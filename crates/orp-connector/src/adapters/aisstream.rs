//! AISStream.io WebSocket connector — real-time global AIS ship tracking.
//!
//! Connects to `wss://stream.aisstream.io/v0/stream` and receives live
//! AIS position reports from vessels worldwide.

use crate::traits::{Connector, ConnectorConfig, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const AISSTREAM_URL: &str = "wss://stream.aisstream.io/v0/stream";

/// Configuration for the AISStream connector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AisStreamConfig {
    pub api_key: String,
    pub bounding_boxes: Vec<[[f64; 2]; 2]>,
    pub filter_mmsi: Option<Vec<String>>,
    pub filter_message_types: Option<Vec<String>>,
}

impl Default for AisStreamConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            bounding_boxes: vec![
                [[-90.0, -180.0], [90.0, 180.0]], // worldwide
            ],
            filter_mmsi: None,
            filter_message_types: Some(vec![
                "PositionReport".to_string(),
                "ShipStaticData".to_string(),
                "StandardClassBPositionReport".to_string(),
            ]),
        }
    }
}

/// AISStream.io WebSocket connector.
pub struct AisStreamConnector {
    config: ConnectorConfig,
    ais_config: AisStreamConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl AisStreamConnector {
    pub fn new(api_key: String) -> Self {
        let ais_config = AisStreamConfig {
            api_key,
            ..Default::default()
        };
        Self {
            config: ConnectorConfig {
                connector_id: "aisstream-live".to_string(),
                connector_type: "aisstream".to_string(),
                url: Some(AISSTREAM_URL.to_string()),
                entity_type: "ship".to_string(),
                properties: HashMap::new(),
                enabled: true,
                trust_score: 0.95,
            },
            ais_config,
            running: Arc::new(AtomicBool::new(false)),
            events_processed: Arc::new(AtomicU64::new(0)),
            errors: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_bounding_boxes(mut self, boxes: Vec<[[f64; 2]; 2]>) -> Self {
        self.ais_config.bounding_boxes = boxes;
        self
    }

    fn build_subscription(&self) -> JsonValue {
        let mut sub = json!({
            "APIKey": self.ais_config.api_key,
            "BoundingBoxes": self.ais_config.bounding_boxes,
        });
        if let Some(ref mmsi) = self.ais_config.filter_mmsi {
            sub["FiltersShipMMSI"] = json!(mmsi);
        }
        if let Some(ref types) = self.ais_config.filter_message_types {
            sub["FilterMessageTypes"] = json!(types);
        }
        sub
    }

    fn parse_message(msg_json: &str, events_processed: &AtomicU64, errors: &AtomicU64) -> Option<SourceEvent> {
        let data: JsonValue = match serde_json::from_str(msg_json) {
            Ok(v) => v,
            Err(_) => {
                errors.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };

        // Check for error messages
        if data.get("error").is_some() {
            tracing::error!("AISStream error: {}", data["error"]);
            errors.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        let msg_type = data.get("MessageType")?.as_str()?;
        let metadata = data.get("MetaData")?;
        let message = data.get("Message")?;

        let mmsi = metadata.get("MMSI").and_then(|v| v.as_u64())
            .map(|m| m.to_string())?;
        let ship_name = metadata.get("ShipName")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .trim()
            .to_string();

        match msg_type {
            "PositionReport" | "StandardClassBPositionReport" | "ExtendedClassBPositionReport" => {
                let report = message.get(msg_type)?;
                let lat = report.get("Latitude").and_then(|v| v.as_f64())?;
                let lon = report.get("Longitude").and_then(|v| v.as_f64())?;

                // Filter invalid positions
                if lat.abs() > 90.0 || lon.abs() > 180.0 {
                    return None;
                }

                let sog = report.get("Sog").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let cog = report.get("Cog").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let heading = report.get("TrueHeading").and_then(|v| v.as_u64()).unwrap_or(511);
                let nav_status = report.get("NavigationalStatus").and_then(|v| v.as_u64()).unwrap_or(15);

                let mut properties = HashMap::new();
                properties.insert("speed".to_string(), json!(sog));
                properties.insert("course".to_string(), json!(cog));
                if heading < 360 {
                    properties.insert("heading".to_string(), json!(heading));
                }
                properties.insert("nav_status".to_string(), json!(nav_status));
                properties.insert("mmsi".to_string(), json!(mmsi));
                if !ship_name.is_empty() && ship_name != "Unknown" {
                    properties.insert("name".to_string(), json!(ship_name));
                }

                events_processed.fetch_add(1, Ordering::Relaxed);

                Some(SourceEvent {
                    connector_id: "aisstream-live".to_string(),
                    entity_id: format!("mmsi:{}", mmsi),
                    entity_type: "ship".to_string(),
                    properties,
                    timestamp: chrono::Utc::now(),
                    latitude: Some(lat),
                    longitude: Some(lon),
                })
            }
            "ShipStaticData" => {
                let report = message.get("ShipStaticData")?;
                let imo = report.get("ImoNumber").and_then(|v| v.as_u64()).unwrap_or(0);
                let callsign = report.get("CallSign").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let ship_type = report.get("Type").and_then(|v| v.as_u64()).unwrap_or(0);
                let dest = report.get("Destination").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let dim_a = report.get("Dimension").and_then(|d| d.get("A")).and_then(|v| v.as_u64()).unwrap_or(0);
                let dim_b = report.get("Dimension").and_then(|d| d.get("B")).and_then(|v| v.as_u64()).unwrap_or(0);
                let length = dim_a + dim_b;

                let lat = metadata.get("latitude").and_then(|v| v.as_f64());
                let lon = metadata.get("longitude").and_then(|v| v.as_f64());

                let mut properties = HashMap::new();
                properties.insert("mmsi".to_string(), json!(mmsi));
                properties.insert("name".to_string(), json!(ship_name));
                if imo > 0 { properties.insert("imo".to_string(), json!(imo)); }
                if !callsign.is_empty() { properties.insert("callsign".to_string(), json!(callsign)); }
                if ship_type > 0 { properties.insert("ship_type".to_string(), json!(ship_type)); }
                if !dest.is_empty() { properties.insert("destination".to_string(), json!(dest)); }
                if length > 0 { properties.insert("length".to_string(), json!(length)); }

                events_processed.fetch_add(1, Ordering::Relaxed);

                Some(SourceEvent {
                    connector_id: "aisstream-live".to_string(),
                    entity_id: format!("mmsi:{}", mmsi),
                    entity_type: "ship".to_string(),
                    properties,
                    timestamp: chrono::Utc::now(),
                    latitude: lat,
                    longitude: lon,
                })
            }
            _ => None,
        }
    }
}

#[async_trait]
impl Connector for AisStreamConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(&self, tx: mpsc::Sender<SourceEvent>) -> Result<(), crate::traits::ConnectorError> {
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let events_processed = self.events_processed.clone();
        let errors = self.errors.clone();
        let subscription = self.build_subscription();

        tracing::info!("Connecting to AISStream.io for live global AIS data...");

        let mut backoff_ms: u64 = 1000;

        while running.load(Ordering::SeqCst) {
            match connect_async(AISSTREAM_URL).await {
                Ok((ws_stream, _)) => {
                    tracing::info!("Connected to AISStream.io — receiving live data");
                    backoff_ms = 1000; // reset backoff on success

                    let (mut write, mut read) = ws_stream.split();

                    // Send subscription within 3 seconds
                    let sub_msg = Message::Text(subscription.to_string());
                    if let Err(e) = write.send(sub_msg).await {
                        tracing::error!("Failed to send subscription: {}", e);
                        errors.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }

                    // Read messages
                    while running.load(Ordering::SeqCst) {
                        match read.next().await {
                            Some(Ok(Message::Text(text))) => {
                                if let Some(event) = Self::parse_message(
                                    &text,
                                    &events_processed,
                                    &errors,
                                ) {
                                    if tx.send(event).await.is_err() {
                                        tracing::warn!("AISStream: channel closed, stopping");
                                        running.store(false, Ordering::SeqCst);
                                        break;
                                    }
                                }
                            }
                            Some(Ok(Message::Close(_))) => {
                                tracing::warn!("AISStream: server closed connection");
                                break;
                            }
                            Some(Ok(_)) => {} // ping/pong/binary — ignore
                            Some(Err(e)) => {
                                tracing::error!("AISStream WebSocket error: {}", e);
                                errors.fetch_add(1, Ordering::Relaxed);
                                break;
                            }
                            None => {
                                tracing::warn!("AISStream: stream ended");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to connect to AISStream.io: {}", e);
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            }

            if running.load(Ordering::SeqCst) {
                tracing::info!("AISStream: reconnecting in {}ms...", backoff_ms);
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(60_000);
            }
        }

        Ok(())
    }

    async fn stop(&self) -> Result<(), crate::traits::ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn health_check(&self) -> Result<(), crate::traits::ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(crate::traits::ConnectorError::ConnectionError("Not running".into()))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        ConnectorStats {
            events_processed: self.events_processed.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            last_event_timestamp: None,
            uptime_seconds: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_subscription_worldwide() {
        let conn = AisStreamConnector::new("test-key".to_string());
        let sub = conn.build_subscription();
        assert_eq!(sub["APIKey"], "test-key");
        assert!(sub["BoundingBoxes"].is_array());
    }

    #[test]
    fn test_parse_position_report() {
        let msg = r#"{
            "MessageType": "PositionReport",
            "MetaData": {"MMSI": 259000420, "ShipName": "AUGUSTSON", "latitude": 66.02695, "longitude": 12.253},
            "Message": {"PositionReport": {"UserID": 259000420, "Latitude": 66.02695, "Longitude": 12.253, "Sog": 5.2, "Cog": 180.0, "TrueHeading": 175, "NavigationalStatus": 0, "Timestamp": 31}}
        }"#;
        let events = AtomicU64::new(0);
        let errors = AtomicU64::new(0);
        let result = AisStreamConnector::parse_message(msg, &events, &errors);
        assert!(result.is_some());
        let ev = result.unwrap();
        assert_eq!(ev.entity_id, "mmsi:259000420");
        assert_eq!(ev.entity_type, "ship");
        assert!((ev.latitude.unwrap() - 66.02695).abs() < 0.001);
        assert_eq!(ev.properties["speed"], json!(5.2));
        assert_eq!(ev.properties["name"], json!("AUGUSTSON"));
    }

    #[test]
    fn test_parse_static_data() {
        let msg = r#"{
            "MessageType": "ShipStaticData",
            "MetaData": {"MMSI": 123456789, "ShipName": "TEST VESSEL", "latitude": 51.9, "longitude": 4.2},
            "Message": {"ShipStaticData": {"ImoNumber": 9876543, "CallSign": "ABCD", "Type": 70, "Destination": "ROTTERDAM", "Dimension": {"A": 100, "B": 50, "C": 10, "D": 10}}}
        }"#;
        let events = AtomicU64::new(0);
        let errors = AtomicU64::new(0);
        let result = AisStreamConnector::parse_message(msg, &events, &errors);
        assert!(result.is_some());
        let ev = result.unwrap();
        assert_eq!(ev.properties["imo"], json!(9876543));
        assert_eq!(ev.properties["destination"], json!("ROTTERDAM"));
        assert_eq!(ev.properties["length"], json!(150));
    }

    #[test]
    fn test_parse_invalid_position() {
        let msg = r#"{
            "MessageType": "PositionReport",
            "MetaData": {"MMSI": 123, "ShipName": "BAD"},
            "Message": {"PositionReport": {"UserID": 123, "Latitude": 91.0, "Longitude": 181.0, "Sog": 0, "Cog": 0, "TrueHeading": 511, "NavigationalStatus": 15, "Timestamp": 0}}
        }"#;
        let events = AtomicU64::new(0);
        let errors = AtomicU64::new(0);
        let result = AisStreamConnector::parse_message(msg, &events, &errors);
        assert!(result.is_none()); // sentinel positions rejected
    }

    #[test]
    fn test_parse_error_message() {
        let msg = r#"{"error": "Api Key Is Not Valid"}"#;
        let events = AtomicU64::new(0);
        let errors = AtomicU64::new(0);
        let result = AisStreamConnector::parse_message(msg, &events, &errors);
        assert!(result.is_none());
        assert_eq!(errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_parse_malformed_json() {
        let msg = "not json at all";
        let events = AtomicU64::new(0);
        let errors = AtomicU64::new(0);
        let result = AisStreamConnector::parse_message(msg, &events, &errors);
        assert!(result.is_none());
        assert_eq!(errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_custom_bounding_boxes() {
        let conn = AisStreamConnector::new("key".to_string())
            .with_bounding_boxes(vec![[[51.0, 3.0], [52.0, 5.0]]]);
        let sub = conn.build_subscription();
        let boxes = sub["BoundingBoxes"].as_array().unwrap();
        assert_eq!(boxes.len(), 1);
    }
}
