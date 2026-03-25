use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// CSV file watcher connector — reads CSV files and emits events
pub struct CsvWatcherConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl CsvWatcherConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Parse a CSV file with headers, returning SourceEvents
    pub fn parse_csv_with_headers(
        content: &str,
        entity_type: &str,
        connector_id: &str,
        id_column: &str,
        lat_column: &str,
        lon_column: &str,
    ) -> Vec<SourceEvent> {
        let mut lines = content.lines();
        let headers: Vec<&str> = match lines.next() {
            Some(header_line) => header_line.split(',').map(|h| h.trim()).collect(),
            None => return vec![],
        };

        let id_idx = headers.iter().position(|h| *h == id_column);
        let lat_idx = headers.iter().position(|h| *h == lat_column);
        let lon_idx = headers.iter().position(|h| *h == lon_column);

        lines
            .filter_map(|line| {
                let fields: Vec<&str> = line.split(',').map(|f| f.trim()).collect();
                if fields.len() < headers.len() {
                    return None;
                }

                let entity_id = id_idx
                    .and_then(|i| fields.get(i))
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if entity_id.is_empty() {
                    return None;
                }

                let latitude = lat_idx
                    .and_then(|i| fields.get(i))
                    .and_then(|s| s.parse::<f64>().ok());
                let longitude = lon_idx
                    .and_then(|i| fields.get(i))
                    .and_then(|s| s.parse::<f64>().ok());

                let mut properties = HashMap::new();
                for (i, header) in headers.iter().enumerate() {
                    if Some(i) != id_idx && Some(i) != lat_idx && Some(i) != lon_idx {
                        if let Some(value) = fields.get(i) {
                            // Try to parse as number, else string
                            if let Ok(n) = value.parse::<f64>() {
                                properties.insert(header.to_string(), serde_json::json!(n));
                            } else if let Ok(b) = value.parse::<bool>() {
                                properties.insert(header.to_string(), serde_json::json!(b));
                            } else {
                                properties
                                    .insert(header.to_string(), serde_json::json!(value));
                            }
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
impl Connector for CsvWatcherConnector {
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
            "CSV watcher connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let connector_id = self.config.connector_id.clone();
        let entity_type = self.config.entity_type.clone();
        let config = self.config.clone();

        let watch_path = config
            .properties
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("./data")
            .to_string();

        let id_column = config
            .properties
            .get("id_column")
            .and_then(|v| v.as_str())
            .unwrap_or("id")
            .to_string();
        let lat_column = config
            .properties
            .get("lat_column")
            .and_then(|v| v.as_str())
            .unwrap_or("latitude")
            .to_string();
        let lon_column = config
            .properties
            .get("lon_column")
            .and_then(|v| v.as_str())
            .unwrap_or("longitude")
            .to_string();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
            let mut processed_files: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                // Scan directory for CSV files
                let entries = match std::fs::read_dir(&watch_path) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };

                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("csv") {
                        continue;
                    }
                    let path_str = path.to_string_lossy().to_string();
                    if processed_files.contains(&path_str) {
                        continue;
                    }

                    match std::fs::read_to_string(&path) {
                        Ok(content) => {
                            let events = CsvWatcherConnector::parse_csv_with_headers(
                                &content,
                                &entity_type,
                                &connector_id,
                                &id_column,
                                &lat_column,
                                &lon_column,
                            );
                            for event in events {
                                if tx.send(event).await.is_err() {
                                    return;
                                }
                                events_count.fetch_add(1, Ordering::Relaxed);
                            }
                            processed_files.insert(path_str);
                        }
                        Err(e) => {
                            tracing::warn!("CSV read error for {:?}: {}", path, e);
                            errors_count.fetch_add(1, Ordering::Relaxed);
                        }
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
            "CSV watcher connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "CSV watcher connector not running".to_string(),
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
    fn test_parse_csv_with_headers() {
        let csv = "id,latitude,longitude,name,speed\n\
                    ship-1,51.92,4.48,Test Ship,12.5\n\
                    ship-2,52.00,4.50,Other Ship,8.0";

        let events = CsvWatcherConnector::parse_csv_with_headers(
            csv, "ship", "csv-1", "id", "latitude", "longitude",
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_id, "ship:ship-1");
        assert!((events[0].latitude.unwrap() - 51.92).abs() < 0.01);
        assert_eq!(
            events[0].properties.get("name"),
            Some(&serde_json::json!("Test Ship"))
        );
        assert_eq!(
            events[0].properties.get("speed"),
            Some(&serde_json::json!(12.5))
        );
    }

    #[test]
    fn test_parse_csv_empty() {
        let csv = "id,lat,lon\n";
        let events = CsvWatcherConnector::parse_csv_with_headers(
            csv, "ship", "csv-1", "id", "lat", "lon",
        );
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_csv_missing_columns() {
        let csv = "name,value\ntest,42";
        let events = CsvWatcherConnector::parse_csv_with_headers(
            csv, "sensor", "csv-1", "name", "lat", "lon",
        );
        assert_eq!(events.len(), 1);
        assert!(events[0].latitude.is_none());
    }
}
