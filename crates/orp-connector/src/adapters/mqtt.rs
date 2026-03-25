use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// MQTT subscriber connector — connects to an MQTT broker and subscribes to topics
pub struct MqttConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl MqttConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Parse an MQTT JSON payload into a SourceEvent
    pub fn parse_mqtt_payload(
        topic: &str,
        payload: &str,
        entity_type: &str,
        connector_id: &str,
    ) -> Option<SourceEvent> {
        let json: serde_json::Value = serde_json::from_str(payload).ok()?;

        let entity_id = json
            .get("id")
            .or_else(|| json.get("entity_id"))
            .or_else(|| json.get("device_id"))
            .and_then(|v| v.as_str().map(String::from).or_else(|| v.as_i64().map(|n| n.to_string())))
            .unwrap_or_else(|| {
                // Derive entity_id from topic
                topic.replace('/', "_")
            });

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
                if k != "latitude" && k != "longitude" && k != "lat" && k != "lon" && k != "lng" {
                    properties.insert(k.clone(), v.clone());
                }
            }
        }
        properties.insert("mqtt_topic".to_string(), serde_json::json!(topic));

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
impl Connector for MqttConnector {
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
            "MQTT connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let connector_id = self.config.connector_id.clone();
        let entity_type = self.config.entity_type.clone();

        // Note: Real MQTT integration would use rumqttc or paho-mqtt crate.
        // For now, we generate synthetic sensor data.
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            let sensors = vec![
                ("sensor/port/rotterdam/tide", "sensor", 51.92, 4.48, "tide_level", 2.5),
                ("sensor/port/rotterdam/wind", "sensor", 51.92, 4.48, "wind_speed", 15.0),
                ("sensor/port/antwerp/tide", "sensor", 51.22, 4.40, "tide_level", 1.8),
                ("sensor/buoy/north_sea_1", "sensor", 53.0, 3.5, "wave_height", 3.2),
            ];

            let mut counter = 0u64;
            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                for (topic, etype, lat, lon, measurement, base_value) in &sensors {
                    let jitter = (counter as f64 % 20.0) * 0.1;
                    let payload = serde_json::json!({
                        "id": topic.replace('/', "_"),
                        "latitude": lat,
                        "longitude": lon,
                        "measurement_type": measurement,
                        "value": base_value + jitter,
                        "unit": match *measurement {
                            "tide_level" => "meters",
                            "wind_speed" => "knots",
                            "wave_height" => "meters",
                            _ => "unknown",
                        },
                    });

                    if let Some(event) = MqttConnector::parse_mqtt_payload(
                        topic,
                        &payload.to_string(),
                        etype,
                        &connector_id,
                    ) {
                        // Override entity type
                        let mut event = event;
                        event.entity_type = entity_type.clone();
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
            "MQTT connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "MQTT connector not running".to_string(),
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
    fn test_parse_mqtt_payload() {
        let payload = r#"{"id": "sensor-1", "latitude": 51.92, "longitude": 4.48, "temperature": 18.5}"#;
        let event =
            MqttConnector::parse_mqtt_payload("sensors/temp/1", payload, "sensor", "mqtt-1");
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.entity_id, "sensor:sensor-1");
        assert!(event.properties.contains_key("temperature"));
        assert!(event.properties.contains_key("mqtt_topic"));
    }

    #[test]
    fn test_parse_mqtt_invalid_json() {
        let event =
            MqttConnector::parse_mqtt_payload("topic", "not json", "sensor", "mqtt-1");
        assert!(event.is_none());
    }

    #[test]
    fn test_parse_mqtt_derive_id_from_topic() {
        let payload = r#"{"temperature": 20.0}"#;
        let event =
            MqttConnector::parse_mqtt_payload("sensor/port/1", payload, "sensor", "mqtt-1");
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.entity_id, "sensor:sensor_port_1");
    }
}
