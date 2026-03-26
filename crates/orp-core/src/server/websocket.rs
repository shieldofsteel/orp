use crate::server::http::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Validate auth token from query param before upgrade
    let token = match params.token {
        Some(t) if !t.is_empty() => t,
        _ => {
            if state.auth_state.permissive_mode {
                // In dev mode allow connections without token
                String::new()
            } else {
                return axum::http::Response::builder()
                    .status(axum::http::StatusCode::UNAUTHORIZED)
                    .body(axum::body::Body::from(
                        r#"{"error":"Missing token query parameter"}"#,
                    ))
                    .unwrap()
                    .into_response();
            }
        }
    };

    // If a token was provided, validate it
    if !token.is_empty() {
        if let Some(ref jwt_svc) = state.auth_state.jwt_service {
            if let Err(_e) = jwt_svc.validate_token(&token) {
                return axum::http::Response::builder()
                    .status(axum::http::StatusCode::UNAUTHORIZED)
                    .body(axum::body::Body::from(
                        r#"{"error":"Invalid or expired token"}"#,
                    ))
                    .unwrap()
                    .into_response();
            }
        }
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state))
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
            org_id: None,
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

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    tracing::info!("WebSocket client connected");

    // Send initial heartbeat
    let heartbeat = serde_json::json!({
        "type": "heartbeat",
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    if socket
        .send(Message::Text(heartbeat.to_string()))
        .await
        .is_err()
    {
        return;
    }

    // Subscribe to the broadcast channel for real-time push
    let mut broadcast_rx = state.broadcast_tx.subscribe();
    let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(15));
    let mut subscriptions: Vec<Subscription> = Vec::new();

    // For ABAC checks on WS events — use permissive defaults for dev mode.
    // In production, the token was validated in ws_handler; use its claims.
    let ws_user_sub = "ws-client".to_string();
    let ws_user_permissions: Vec<String> = if state.auth_state.permissive_mode {
        vec!["admin".to_string()]
    } else {
        vec!["entities:read".to_string()]
    };

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

                                    if socket.send(Message::Text(confirmation.to_string())).await.is_err() {
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
                            // ABAC check: can this client see this entity?
                            if !can_see_entity(
                                &state.abac_engine,
                                &ws_user_sub,
                                &ws_user_permissions,
                                &event_entity_type,
                                &event_entity_id,
                            ) {
                                continue;
                            }

                            if let Ok(json) = serde_json::to_string(&event) {
                                if socket.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WebSocket broadcast lagged by {} messages", n);
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

                if socket.send(Message::Text(heartbeat.to_string())).await.is_err() {
                    break;
                }
            }
        }
    }

    tracing::info!("WebSocket client disconnected");
}
