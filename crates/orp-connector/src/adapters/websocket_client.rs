use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// WebSocket client connector — connects to a WebSocket server and receives events
pub struct WebSocketClientConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl WebSocketClientConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Parse a WebSocket JSON message into a SourceEvent
    pub fn parse_ws_message(
        message: &str,
        entity_type: &str,
        connector_id: &str,
    ) -> Option<SourceEvent> {
        let json: serde_json::Value = serde_json::from_str(message).ok()?;

        let entity_id = json
            .get("id")
            .or_else(|| json.get("entity_id"))
            .or_else(|| json.get("device_id"))
            .and_then(|v| v.as_str().map(String::from).or_else(|| v.as_i64().map(|n| n.to_string())))?;

        let latitude = json
            .get("latitude")
            .or_else(|| json.get("lat"))
            .and_then(|v| v.as_f64());
        let longitude = json
            .get("longitude")
            .or_else(|| json.get("lon"))
            .or_else(|| json.get("lng"))
            .and_then(|v| v.as_f64());

        let mut properties = HashMap::new();
        if let Some(obj) = json.as_object() {
            for (k, v) in obj {
                if k != "id"
                    && k != "entity_id"
                    && k != "latitude"
                    && k != "longitude"
                    && k != "lat"
                    && k != "lon"
                    && k != "lng"
                {
                    properties.insert(k.clone(), v.clone());
                }
            }
        }

        Some(SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id: format!("{}:{}", entity_type, entity_id),
            entity_type: entity_type.to_string(),
            properties,
            timestamp: Utc::now(),
            latitude,
            longitude,
        })
    }
}

#[async_trait]
impl Connector for WebSocketClientConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "WebSocket client connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let connector_id = self.config.connector_id.clone();
        let entity_type = self.config.entity_type.clone();

        // Note: Real WebSocket client would use tokio-tungstenite.
        // For now, generate synthetic streaming data.
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(4));
            let demo_streams = vec![
                ("vessel-tracker-1", 51.90, 4.50, "ship", 14.0, 200.0),
                ("vessel-tracker-2", 52.10, 4.30, "ship", 9.5, 150.0),
                ("vessel-tracker-3", 51.85, 4.60, "ship", 18.0, 320.0),
            ];

            let mut counter = 0u64;
            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                for (id, lat, lon, _etype, speed, course) in &demo_streams {
                    let jitter = (counter as f64 % 30.0) * 0.0005;
                    let msg = serde_json::json!({
                        "id": id,
                        "latitude": lat + jitter,
                        "longitude": lon + jitter * 0.5,
                        "speed": speed + (counter as f64 % 5.0) * 0.2,
                        "course": course,
                        "status": "underway",
                    });

                    if let Some(event) = WebSocketClientConnector::parse_ws_message(
                        &msg.to_string(),
                        &entity_type,
                        &connector_id,
                    ) {
                        if tx.send(event).await.is_err() {
                            return;
                        }
                        events_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
                counter += 1;
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "WebSocket client connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "WebSocket client connector not running".to_string(),
            ))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
            last_event_timestamp: Some(Utc::now()),
            uptime_seconds: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ws_message() {
        let msg = r#"{"id": "vessel-1", "latitude": 51.92, "longitude": 4.48, "speed": 12.5, "status": "underway"}"#;
        let event =
            WebSocketClientConnector::parse_ws_message(msg, "ship", "ws-1");
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.entity_id, "ship:vessel-1");
        assert!(event.properties.contains_key("speed"));
        assert!(event.properties.contains_key("status"));
    }

    #[test]
    fn test_parse_ws_message_alt_fields() {
        let msg = r#"{"entity_id": "device-42", "lat": 52.0, "lon": 4.5}"#;
        let event =
            WebSocketClientConnector::parse_ws_message(msg, "sensor", "ws-1");
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.entity_id, "sensor:device-42");
    }

    #[test]
    fn test_parse_ws_invalid() {
        assert!(WebSocketClientConnector::parse_ws_message("not json", "s", "ws-1").is_none());
        // Missing id
        assert!(
            WebSocketClientConnector::parse_ws_message(r#"{"value": 42}"#, "s", "ws-1").is_none()
        );
    }
}
