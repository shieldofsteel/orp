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

    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
    let mut subscriptions: Vec<Subscription> = Vec::new();

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
            _ = interval.tick() => {
                // Push entity updates for each subscription
                for sub in &subscriptions {
                    if let Some(ref entity_type) = sub.entity_type {
                        if let Ok(entities) = state.storage.get_entities_by_type(entity_type, 50, 0).await {
                            for entity in entities.iter().take(10) {
                                let update = serde_json::json!({
                                    "type": "entity_update",
                                    "timestamp": chrono::Utc::now().to_rfc3339(),
                                    "data": {
                                        "entity_id": entity.entity_id,
                                        "entity_type": entity.entity_type,
                                        "changes": entity.properties,
                                        "geometry": entity.geometry.as_ref().map(|g| {
                                            serde_json::json!({
                                                "type": "Point",
                                                "coordinates": [g.lon, g.lat]
                                            })
                                        }),
                                    }
                                });

                                if socket.send(Message::Text(update.to_string())).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }

                    if let Some(ref eid) = sub.entity_id {
                        if let Ok(Some(entity)) = state.storage.get_entity(eid).await {
                            let update = serde_json::json!({
                                "type": "entity_update",
                                "timestamp": chrono::Utc::now().to_rfc3339(),
                                "data": {
                                    "entity_id": entity.entity_id,
                                    "entity_type": entity.entity_type,
                                    "changes": entity.properties,
                                    "geometry": entity.geometry.as_ref().map(|g| {
                                        serde_json::json!({
                                            "type": "Point",
                                            "coordinates": [g.lon, g.lat]
                                        })
                                    }),
                                }
                            });

                            if socket.send(Message::Text(update.to_string())).await.is_err() {
                                return;
                            }
                        }
                    }
                }

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
