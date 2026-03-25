use crate::server::http::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use std::sync::Arc;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
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

    // Process incoming messages and send updates
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
    let mut subscribed_type: Option<String> = None;

    loop {
        tokio::select! {
            Some(msg) = socket.recv() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            let msg_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");

                            match msg_type {
                                "subscribe" => {
                                    if let Some(data) = parsed.get("data") {
                                        subscribed_type = data.get("entity_type")
                                            .and_then(|t| t.as_str())
                                            .map(String::from);

                                        let confirmation = serde_json::json!({
                                            "type": "subscription_created",
                                            "id": parsed.get("id"),
                                            "timestamp": chrono::Utc::now().to_rfc3339(),
                                            "data": {
                                                "entity_type": subscribed_type,
                                            }
                                        });

                                        if socket.send(Message::Text(confirmation.to_string())).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                "unsubscribe" => {
                                    subscribed_type = None;
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
                // Send entity updates for subscribed types
                if let Some(ref entity_type) = subscribed_type {
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
