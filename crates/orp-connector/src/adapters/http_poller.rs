use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// HTTP REST polling connector — periodically fetches JSON data from a URL
pub struct HttpPollerConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl HttpPollerConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Extract entities from a JSON array response
    pub fn extract_entities(
        json: &serde_json::Value,
        entity_type: &str,
        connector_id: &str,
        id_field: &str,
        lat_field: &str,
        lon_field: &str,
    ) -> Vec<SourceEvent> {
        let items = if let Some(arr) = json.as_array() {
            arr.clone()
        } else if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
            data.clone()
        } else if let Some(features) = json.get("features").and_then(|f| f.as_array()) {
            // GeoJSON format
            features.clone()
        } else {
            vec![json.clone()]
        };

        items
            .iter()
            .filter_map(|item| {
                let entity_id = item
                    .get(id_field)
                    .and_then(|v| v.as_str().map(String::from).or_else(|| v.as_i64().map(|n| n.to_string())))
                    .unwrap_or_default();
                if entity_id.is_empty() {
                    return None;
                }

                let latitude = item
                    .get(lat_field)
                    .and_then(|v| v.as_f64());
                let longitude = item
                    .get(lon_field)
                    .and_then(|v| v.as_f64());

                let mut properties = HashMap::new();
                if let Some(obj) = item.as_object() {
                    for (k, v) in obj {
                        if k != id_field && k != lat_field && k != lon_field {
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
            })
            .collect()
    }
}

#[async_trait]
impl Connector for HttpPollerConnector {
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
            "HTTP poller connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let config = self.config.clone();
        let connector_id = self.config.connector_id.clone();

        let poll_interval_secs: u64 = config
            .properties
            .get("poll_interval_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        let id_field = config
            .properties
            .get("id_field")
            .and_then(|v| v.as_str())
            .unwrap_or("id")
            .to_string();
        let lat_field = config
            .properties
            .get("lat_field")
            .and_then(|v| v.as_str())
            .unwrap_or("latitude")
            .to_string();
        let lon_field = config
            .properties
            .get("lon_field")
            .and_then(|v| v.as_str())
            .unwrap_or("longitude")
            .to_string();

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(poll_interval_secs));

            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                if let Some(ref url) = config.url {
                    // Try real HTTP request
                    match reqwest::get(url).await {
                        Ok(resp) => match resp.json::<serde_json::Value>().await {
                            Ok(json) => {
                                let events = HttpPollerConnector::extract_entities(
                                    &json,
                                    &config.entity_type,
                                    &connector_id,
                                    &id_field,
                                    &lat_field,
                                    &lon_field,
                                );
                                for event in events {
                                    if tx.send(event).await.is_err() {
                                        return;
                                    }
                                    events_count.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(e) => {
                                tracing::warn!("HTTP poller JSON parse error: {}", e);
                                errors_count.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Err(e) => {
                            tracing::warn!("HTTP poller request error: {}", e);
                            errors_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                } else {
                    // Demo mode: generate synthetic weather data
                    let demo_weather = vec![
                        ("storm-atlantic-1", 45.0, -30.0, "tropical_storm", "warning"),
                        ("low-pressure-north", 58.0, 5.0, "low_pressure", "info"),
                        ("high-pressure-med", 38.0, 15.0, "high_pressure", "info"),
                    ];

                    for (id, lat, lon, sys_type, severity) in &demo_weather {
                        let mut properties = HashMap::new();
                        properties.insert(
                            "system_type".to_string(),
                            serde_json::json!(sys_type),
                        );
                        properties.insert(
                            "severity".to_string(),
                            serde_json::json!(severity),
                        );
                        properties.insert(
                            "radius_km".to_string(),
                            serde_json::json!(200.0),
                        );

                        let event = SourceEvent {
                            connector_id: connector_id.clone(),
                            entity_id: format!("weather:{}", id),
                            entity_type: "weather_system".to_string(),
                            properties,
                            timestamp: Utc::now(),
                            latitude: Some(*lat),
                            longitude: Some(*lon),
                        };

                        if tx.send(event).await.is_err() {
                            return;
                        }
                        events_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "HTTP poller connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "HTTP poller connector not running".to_string(),
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
    fn test_extract_entities_array() {
        let json = serde_json::json!([
            {"id": "vessel-1", "latitude": 51.92, "longitude": 4.48, "name": "Ship A"},
            {"id": "vessel-2", "latitude": 52.00, "longitude": 4.50, "name": "Ship B"},
        ]);

        let events = HttpPollerConnector::extract_entities(
            &json, "ship", "http-1", "id", "latitude", "longitude",
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_id, "ship:vessel-1");
        assert!((events[0].latitude.unwrap() - 51.92).abs() < 0.01);
    }

    #[test]
    fn test_extract_entities_nested() {
        let json = serde_json::json!({
            "data": [
                {"id": "port-1", "latitude": 51.92, "longitude": 4.48},
            ]
        });

        let events = HttpPollerConnector::extract_entities(
            &json, "port", "http-1", "id", "latitude", "longitude",
        );
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_extract_entities_empty() {
        let json = serde_json::json!([]);
        let events = HttpPollerConnector::extract_entities(
            &json, "ship", "http-1", "id", "lat", "lon",
        );
        assert!(events.is_empty());
    }
}
