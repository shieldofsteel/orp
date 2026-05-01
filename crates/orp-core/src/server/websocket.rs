use crate::server::http::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::HeaderMap,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Events pushed to all WebSocket clients via tokio::broadcast.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub enum BroadcastEvent {
    #[serde(rename = "entity_created")]
    EntityCreated {
        entity_id: String,
        entity_type: String,
        entity_name: Option<String>,
        properties: serde_json::Value,
        geometry: Option<serde_json::Value>,
        timestamp: String,
    },
    #[serde(rename = "entity_update")]
    EntityUpdate {
        entity_id: String,
        entity_type: String,
        changes: serde_json::Value,
        geometry: Option<serde_json::Value>,
        timestamp: String,
    },
    #[serde(rename = "entity_deleted")]
    EntityDeleted {
        entity_id: String,
        entity_type: String,
        timestamp: String,
    },
    #[serde(rename = "relationship_changed")]
    RelationshipChanged {
        relationship_id: String,
        source_id: String,
        target_id: String,
        relationship_type: String,
        event: String, // "created" | "deleted" | "updated"
        timestamp: String,
    },
    #[serde(rename = "alert_triggered")]
    AlertTriggered {
        id: String,
        monitor_id: String,
        monitor_name: String,
        severity: String,
        affected_entities: Vec<serde_json::Value>,
        timestamp: String,
    },
}

/// Query parameters for WebSocket upgrade — auth token is required.
#[derive(Deserialize)]
pub struct WsParams {
    token: Option<String>,
}

/// Per-connection authenticated identity for the WebSocket session.
///
/// Captured from the validated JWT (or `X-API-Key` header) at upgrade time and
/// passed into [`handle_socket`] so that ABAC checks and structured log
/// emission run against the *real* caller — not a synthetic "ws-client".
///
/// Constructed in [`ws_handler`] from one of three sources, in priority order:
///
/// 1. `Authorization: Bearer <jwt>` header or `?token=<jwt>` query param —
///    JWT decoded, [`Claims`](orp_security::Claims) carried through.
/// 2. `X-API-Key: <key>` header — `ApiKeyService::validate_key` invoked, the
///    granted scopes carried through as permissions.
/// 3. Permissive (dev) mode with no credentials — synthetic admin identity.
///
/// Permissive mode with a valid JWT/API key still uses the *token's* identity:
/// a developer may still want to test "this user has only `entities:read`"
/// in a local dev box.
#[derive(Clone, Debug)]
pub struct WsAuth {
    /// Subject identifier — JWT `sub`, API key `key_id`, or `"anonymous-dev"`.
    pub subject: String,
    /// Granted permissions. Used by the per-event ABAC check.
    pub permissions: Vec<String>,
    /// Org identifier from JWT (when present) or API key.
    pub org_id: Option<String>,
    /// How the caller authenticated. Logged once per connection.
    pub method: WsAuthMethod,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WsAuthMethod {
    Jwt,
    ApiKey,
    PermissiveDev,
}

impl WsAuth {
    fn permissive_dev() -> Self {
        Self {
            subject: "anonymous-dev".to_string(),
            permissions: vec!["admin".to_string()],
            org_id: None,
            method: WsAuthMethod::PermissiveDev,
        }
    }

    fn from_claims(claims: orp_security::Claims) -> Self {
        Self {
            subject: claims.sub,
            permissions: claims.permissions,
            org_id: claims.org_id,
            method: WsAuthMethod::Jwt,
        }
    }

    fn from_api_key(result: orp_security::ApiKeyValidationResult) -> Self {
        Self {
            subject: result.key_id,
            permissions: result.scopes,
            org_id: result.org_id,
            method: WsAuthMethod::ApiKey,
        }
    }
}

fn unauthorized_response(reason: &str) -> axum::response::Response {
    let body = format!(r#"{{"error":"{reason}"}}"#);
    axum::http::Response::builder()
        .status(axum::http::StatusCode::UNAUTHORIZED)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap()
        .into_response()
}

/// Resolve the per-connection identity from the request before upgrade.
///
/// Returns `Err(reason)` on auth failure; the caller turns that into a 401.
/// Extracted from [`ws_handler`] so each branch can be unit-tested without
/// spinning up an axum router or a real WebSocket.
pub(crate) async fn resolve_ws_auth(
    auth_state: &orp_security::AuthState,
    bearer_jwt: Option<&str>,
    api_key: Option<&str>,
) -> Result<WsAuth, &'static str> {
    if let Some(token) = bearer_jwt {
        match auth_state.jwt_service.as_ref() {
            Some(jwt_svc) => match jwt_svc.validate_token(token) {
                Ok(claims) => Ok(WsAuth::from_claims(claims)),
                Err(e) => {
                    tracing::warn!(error = %e, "WebSocket JWT rejected");
                    Err("Invalid or expired token")
                }
            },
            None if auth_state.permissive_mode => {
                // Dev-mode safety net: no JWT service configured, but a token
                // was offered. Treat as permissive-dev rather than 500.
                tracing::warn!(
                    "WebSocket received JWT but no jwt_service is configured; \
                     falling through to permissive-dev identity"
                );
                Ok(WsAuth::permissive_dev())
            }
            None => Err("Authentication not configured"),
        }
    } else if let Some(key) = api_key {
        match auth_state.api_key_service.as_ref() {
            Some(svc) => match svc.validate_key(key).await {
                Ok(result) if result.is_revoked => Err("API key has been revoked"),
                Ok(result) if result.is_expired => Err("API key has expired"),
                Ok(result) => Ok(WsAuth::from_api_key(result)),
                Err(e) => {
                    tracing::warn!(error = %e, "WebSocket API key rejected");
                    Err("Invalid API key")
                }
            },
            None if auth_state.permissive_mode => Ok(WsAuth::permissive_dev()),
            None => Err("API key auth not configured"),
        }
    } else if auth_state.permissive_mode {
        tracing::warn!(
            "WebSocket admitted in permissive (dev) mode — synthetic admin identity. \
             Refuse this if you see it in production."
        );
        Ok(WsAuth::permissive_dev())
    } else {
        Err("Missing authentication credentials")
    }
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Priority: Authorization header → ?token query param → X-API-Key header.
    // Browsers can't easily set custom headers on a `new WebSocket()`, so the
    // ?token query path remains the primary JWT carrier; the `Authorization`
    // header is honoured for non-browser clients (CLI, server-to-server).
    let bearer_jwt = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .or_else(|| params.token.filter(|t| !t.is_empty()));

    let api_key = headers
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let auth = match resolve_ws_auth(
        state.auth_state.as_ref(),
        bearer_jwt.as_deref(),
        api_key.as_deref(),
    )
    .await
    {
        Ok(a) => a,
        Err(reason) => return unauthorized_response(reason),
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, auth))
        .into_response()
}

/// Per-client subscription entry.
#[derive(Clone, Debug)]
struct Subscription {
    id: String,
    entity_type: Option<String>,
    entity_id: Option<String>,
}

/// Check whether this client is allowed to see a given entity via ABAC.
fn can_see_entity(
    abac: &orp_security::AbacEngine,
    user_sub: &str,
    user_permissions: &[String],
    user_org_id: Option<&str>,
    entity_type: &str,
    entity_id: &str,
) -> bool {
    let ctx = orp_security::EvaluationContext {
        subject: orp_security::Subject {
            sub: user_sub.to_string(),
            permissions: user_permissions.to_vec(),
            role: if user_permissions.iter().any(|p| p == "admin") {
                Some("admin".to_string())
            } else {
                None
            },
            org_id: user_org_id.map(|s| s.to_string()),
            attributes: std::collections::HashMap::new(),
        },
        action: "entities:read".to_string(),
        resource: orp_security::Resource {
            r#type: entity_type.to_string(),
            id: entity_id.to_string(),
            attributes: std::collections::HashMap::new(),
        },
    };
    abac.evaluate(&ctx).result == orp_security::EvaluationResult::Allow
}

/// Send a message with a 5-second timeout. Returns `Err(())` if the send
/// failed *or* timed out — caller breaks the connection in either case.
///
/// The audit flagged the unbounded `socket.send().await` as Medium severity:
/// a half-stuck client that ACKs the TCP connection but never drains its
/// receive buffer would otherwise pin the broadcast loop indefinitely,
/// holding a `broadcast_rx` slot and blocking heartbeat ticks.
async fn send_with_timeout(socket: &mut WebSocket, msg: Message) -> Result<(), ()> {
    match tokio::time::timeout(Duration::from_secs(5), socket.send(msg)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "WebSocket send error — closing connection");
            Err(())
        }
        Err(_) => {
            tracing::warn!("WebSocket send timed out after 5s — closing connection");
            Err(())
        }
    }
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>, auth: WsAuth) {
    tracing::info!(
        subject = %auth.subject,
        method = ?auth.method,
        permissions = ?auth.permissions,
        org_id = ?auth.org_id,
        "WebSocket client connected"
    );

    // Send initial heartbeat
    let heartbeat = serde_json::json!({
        "type": "heartbeat",
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    if send_with_timeout(&mut socket, Message::Text(heartbeat.to_string()))
        .await
        .is_err()
    {
        return;
    }

    // Subscribe to the broadcast channel for real-time push
    let mut broadcast_rx = state.broadcast_tx.subscribe();
    let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(15));
    let mut subscriptions: Vec<Subscription> = Vec::new();

    // ABAC identity for *this* connection — taken straight from the validated
    // JWT/API-key claims. Previously the WS path ignored the token's claims
    // and granted hardcoded permissions; that was the v0.3.0 audit's critical
    // finding. With these bound, a JWT carrying `permissions: []` correctly
    // produces zero deliveries instead of full admin replay.
    let ws_user_sub = auth.subject.clone();
    let ws_user_permissions = auth.permissions.clone();
    let ws_user_org_id = auth.org_id.clone();

    loop {
        tokio::select! {
            Some(msg) = socket.recv() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            let msg_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");

                            match msg_type {
                                "subscribe" => {
                                    let sub_id = parsed.get("id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("default")
                                        .to_string();
                                    let data = parsed.get("data");
                                    let entity_type = data
                                        .and_then(|d| d.get("entity_type"))
                                        .and_then(|t| t.as_str())
                                        .map(String::from);
                                    let entity_id = data
                                        .and_then(|d| d.get("entity_id"))
                                        .and_then(|t| t.as_str())
                                        .map(String::from);

                                    subscriptions.push(Subscription {
                                        id: sub_id.clone(),
                                        entity_type: entity_type.clone(),
                                        entity_id: entity_id.clone(),
                                    });

                                    let confirmation = serde_json::json!({
                                        "type": "subscription_created",
                                        "id": sub_id,
                                        "timestamp": chrono::Utc::now().to_rfc3339(),
                                        "data": {
                                            "subscription_id": sub_id,
                                            "entity_type": entity_type,
                                        }
                                    });

                                    if send_with_timeout(&mut socket, Message::Text(confirmation.to_string())).await.is_err() {
                                        break;
                                    }
                                }
                                "unsubscribe" => {
                                    if let Some(data) = parsed.get("data") {
                                        let sub_id = data.get("subscription_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        subscriptions.retain(|s| s.id != sub_id);
                                    }
                                }
                                "heartbeat_ack" => {
                                    // Client acknowledged heartbeat
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
            // Receive broadcast events from mutations in handlers
            result = broadcast_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Check if this event matches any subscription
                        let (event_entity_type, event_entity_id) = match &event {
                            BroadcastEvent::EntityCreated { entity_type, entity_id, .. } => (entity_type.clone(), entity_id.clone()),
                            BroadcastEvent::EntityUpdate { entity_type, entity_id, .. } => (entity_type.clone(), entity_id.clone()),
                            BroadcastEvent::EntityDeleted { entity_type, entity_id, .. } => (entity_type.clone(), entity_id.clone()),
                            BroadcastEvent::RelationshipChanged { source_id, .. } => ("relationship".to_string(), source_id.clone()),
                            BroadcastEvent::AlertTriggered { .. } => ("alert".to_string(), String::new()),
                        };

                        let matches_sub = subscriptions.iter().any(|sub| {
                            if let Some(ref st) = sub.entity_type {
                                if st.eq_ignore_ascii_case(&event_entity_type) {
                                    return true;
                                }
                            }
                            if let Some(ref sid) = sub.entity_id {
                                if *sid == event_entity_id {
                                    return true;
                                }
                            }
                            // Alert events match any subscription
                            matches!(&event, BroadcastEvent::AlertTriggered { .. })
                        });

                        if matches_sub {
                            // ABAC check: can THIS client (real JWT/API-key
                            // identity) see this entity?
                            if !can_see_entity(
                                &state.abac_engine,
                                &ws_user_sub,
                                &ws_user_permissions,
                                ws_user_org_id.as_deref(),
                                &event_entity_type,
                                &event_entity_id,
                            ) {
                                continue;
                            }

                            if let Ok(json) = serde_json::to_string(&event) {
                                if send_with_timeout(&mut socket, Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            subject = %ws_user_sub,
                            "WebSocket broadcast lagged by {} messages",
                            n
                        );
                    }
                    Err(_) => break,
                }
            }
            _ = heartbeat_interval.tick() => {
                // Send heartbeat
                let heartbeat = serde_json::json!({
                    "type": "heartbeat",
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });

                if send_with_timeout(&mut socket, Message::Text(heartbeat.to_string())).await.is_err() {
                    break;
                }
            }
        }
    }

    tracing::info!(subject = %ws_user_sub, "WebSocket client disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use orp_security::{
        AbacEngine, ApiKeyService, AuthState, CreateApiKeyRequest, JwtConfig, JwtService,
    };

    fn make_jwt_service() -> JwtService {
        let config = JwtConfig {
            hs256_secret: Some(b"test-secret-do-not-use-in-prod-32bytes!!".to_vec()),
            ..JwtConfig::default()
        };
        JwtService::new(config).expect("jwt service")
    }

    #[tokio::test]
    async fn test_ws_uses_jwt_subject() {
        let jwt_svc = make_jwt_service();
        let token = jwt_svc
            .issue_token(
                "alice",
                Some("alice@example.com"),
                None,
                Some("org-acme"),
                vec!["entities:read".to_string()],
            )
            .unwrap();

        let auth_state = AuthState {
            jwt_service: Some(Arc::new(jwt_svc)),
            api_key_service: None,
            permissive_mode: false,
        };

        let auth = resolve_ws_auth(&auth_state, Some(&token), None)
            .await
            .expect("auth resolves");

        assert_eq!(auth.subject, "alice");
        assert_eq!(auth.permissions, vec!["entities:read".to_string()]);
        assert_eq!(auth.org_id.as_deref(), Some("org-acme"));
        assert_eq!(auth.method, WsAuthMethod::Jwt);
    }

    #[tokio::test]
    async fn test_ws_respects_jwt_permissions() {
        // JWT with empty permissions → ABAC denies non-admin reads.
        let jwt_svc = make_jwt_service();
        let token = jwt_svc
            .issue_token("bob", None, None, None, vec![])
            .unwrap();

        let auth_state = AuthState {
            jwt_service: Some(Arc::new(jwt_svc)),
            api_key_service: None,
            permissive_mode: false,
        };
        let auth = resolve_ws_auth(&auth_state, Some(&token), None)
            .await
            .unwrap();

        assert_eq!(auth.subject, "bob");
        assert!(auth.permissions.is_empty());

        // Default-deny ABAC engine — empty permissions must produce a deny.
        let abac = AbacEngine::new();
        let allowed = can_see_entity(&abac, &auth.subject, &auth.permissions, None, "ship", "v-1");
        assert!(
            !allowed,
            "JWT with empty permissions must NOT see entities under default-deny ABAC"
        );
    }

    #[tokio::test]
    async fn test_ws_admin_jwt() {
        let jwt_svc = make_jwt_service();
        let token = jwt_svc
            .issue_token("carol", None, None, None, vec!["admin".to_string()])
            .unwrap();

        let auth_state = AuthState {
            jwt_service: Some(Arc::new(jwt_svc)),
            api_key_service: None,
            permissive_mode: false,
        };
        let auth = resolve_ws_auth(&auth_state, Some(&token), None)
            .await
            .unwrap();

        assert_eq!(auth.subject, "carol");
        assert_eq!(auth.permissions, vec!["admin".to_string()]);

        // permissive ABAC → admin definitely sees entities.
        let abac = AbacEngine::default_permissive();
        let allowed = can_see_entity(&abac, &auth.subject, &auth.permissions, None, "ship", "v-1");
        assert!(allowed, "admin JWT must see entities under permissive ABAC");
    }

    #[tokio::test]
    async fn test_ws_rejects_invalid_jwt() {
        let jwt_svc = make_jwt_service();
        let auth_state = AuthState {
            jwt_service: Some(Arc::new(jwt_svc)),
            api_key_service: None,
            permissive_mode: false,
        };
        let err = resolve_ws_auth(&auth_state, Some("not.a.jwt"), None)
            .await
            .unwrap_err();
        assert_eq!(err, "Invalid or expired token");
    }

    #[tokio::test]
    async fn test_ws_rejects_missing_credentials_in_strict_mode() {
        let auth_state = AuthState {
            jwt_service: Some(Arc::new(make_jwt_service())),
            api_key_service: None,
            permissive_mode: false,
        };
        let err = resolve_ws_auth(&auth_state, None, None).await.unwrap_err();
        assert_eq!(err, "Missing authentication credentials");
    }

    #[tokio::test]
    async fn test_ws_permissive_dev_admits_anonymous() {
        let auth_state = AuthState {
            jwt_service: None,
            api_key_service: None,
            permissive_mode: true,
        };
        let auth = resolve_ws_auth(&auth_state, None, None).await.unwrap();
        assert_eq!(auth.method, WsAuthMethod::PermissiveDev);
        assert_eq!(auth.subject, "anonymous-dev");
    }

    #[tokio::test]
    async fn test_ws_api_key_uses_key_identity() {
        let svc = ApiKeyService::new();
        let resp = svc
            .create_key(CreateApiKeyRequest {
                name: "test".to_string(),
                scopes: vec!["entities:read".to_string()],
                rate_limit: Some(0),
                expires_in: None,
                org_id: Some("org-acme".to_string()),
            })
            .unwrap();

        let auth_state = AuthState {
            jwt_service: None,
            api_key_service: Some(Arc::new(svc)),
            permissive_mode: false,
        };
        let auth = resolve_ws_auth(&auth_state, None, Some(&resp.api_key))
            .await
            .unwrap();

        assert_eq!(auth.subject, resp.id);
        assert_eq!(auth.permissions, vec!["entities:read".to_string()]);
        assert_eq!(auth.org_id.as_deref(), Some("org-acme"));
        assert_eq!(auth.method, WsAuthMethod::ApiKey);
    }

    #[tokio::test]
    async fn test_ws_rejects_revoked_api_key() {
        let svc = ApiKeyService::new();
        let resp = svc
            .create_key(CreateApiKeyRequest {
                name: "test".to_string(),
                scopes: vec!["entities:read".to_string()],
                rate_limit: Some(0),
                expires_in: None,
                org_id: None,
            })
            .unwrap();
        svc.revoke_key(&resp.id).unwrap();

        let auth_state = AuthState {
            jwt_service: None,
            api_key_service: Some(Arc::new(svc)),
            permissive_mode: false,
        };
        let err = resolve_ws_auth(&auth_state, None, Some(&resp.api_key))
            .await
            .unwrap_err();
        // Revoked keys come back as `ApiKeyError::Revoked` from validate_key,
        // which falls through to the generic "Invalid API key" branch.
        assert_eq!(err, "Invalid API key");
    }
}
