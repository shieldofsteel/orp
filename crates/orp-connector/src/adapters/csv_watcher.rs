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

    /// Parse a CSV file with headers, returning SourceEvents.
    ///
    /// Quoted fields containing commas (e.g. `"Doe, John",51.5,-0.1`)
    /// parse correctly via the `csv` crate. The optional `parse_errors`
    /// counter is bumped once per malformed record (invalid UTF-8,
    /// header decode failure, ...) and each failure is logged via
    /// `tracing::warn!`. Callers surface this via `errors_count`.
    pub fn parse_csv_with_headers(
        content: &str,
        entity_type: &str,
        connector_id: &str,
        id_column: &str,
        lat_column: &str,
        lon_column: &str,
        parse_errors: Option<&AtomicU64>,
    ) -> Vec<SourceEvent> {
        Self::parse_csv_bytes_with_headers(
            content.as_bytes(), entity_type, connector_id,
            id_column, lat_column, lon_column, parse_errors,
        )
    }

    /// Byte-oriented variant — the only path that can hit invalid-UTF-8
    /// errors from the `csv` crate.
    pub fn parse_csv_bytes_with_headers(
        bytes: &[u8],
        entity_type: &str,
        connector_id: &str,
        id_column: &str,
        lat_column: &str,
        lon_column: &str,
        parse_errors: Option<&AtomicU64>,
    ) -> Vec<SourceEvent> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .trim(csv::Trim::All)
            .flexible(true) // tolerate ragged rows; we'll filter in-loop.
            .from_reader(bytes);

        let headers: Vec<String> = match reader.headers() {
            Ok(h) => h.iter().map(|s| s.to_string()).collect(),
            Err(e) => {
                tracing::warn!(connector_id = %connector_id, error = %e,
                    "CSV header parse failed; skipping file");
                if let Some(c) = parse_errors { c.fetch_add(1, Ordering::Relaxed); }
                return vec![];
            }
        };

        let id_idx = headers.iter().position(|h| h == id_column);
        let lat_idx = headers.iter().position(|h| h == lat_column);
        let lon_idx = headers.iter().position(|h| h == lon_column);

        let mut events: Vec<SourceEvent> = Vec::new();
        for result in reader.records() {
            let record = match result {
                Ok(r) => r,
                Err(e) => {
                    let line = e.position().map(|p| p.line()).unwrap_or(0);
                    tracing::warn!(connector_id = %connector_id, line, error = %e,
                        "CSV row parse failed; dropping row");
                    if let Some(c) = parse_errors { c.fetch_add(1, Ordering::Relaxed); }
                    continue;
                }
            };

            let entity_id = id_idx
                .and_then(|i| record.get(i))
                .map(|s| s.to_string())
                .unwrap_or_default();
            if entity_id.is_empty() {
                continue;
            }

            let latitude = lat_idx
                .and_then(|i| record.get(i))
                .and_then(|s| s.parse::<f64>().ok());
            let longitude = lon_idx
                .and_then(|i| record.get(i))
                .and_then(|s| s.parse::<f64>().ok());

            let mut properties = HashMap::new();
            for (i, header) in headers.iter().enumerate() {
                if Some(i) != id_idx && Some(i) != lat_idx && Some(i) != lon_idx {
                    if let Some(value) = record.get(i) {
                        // Try number → bool → fall back to string.
                        if let Ok(n) = value.parse::<f64>() {
                            properties.insert(header.clone(), serde_json::json!(n));
                        } else if let Ok(b) = value.parse::<bool>() {
                            properties.insert(header.clone(), serde_json::json!(b));
                        } else {
                            properties.insert(header.clone(), serde_json::json!(value));
                        }
                    }
                }
            }

            events.push(SourceEvent {
                connector_id: connector_id.to_string(),
                entity_id: format!("{}:{}", entity_type, entity_id),
                entity_type: entity_type.to_string(),
                properties,
                timestamp: Utc::now(),
                latitude,
                longitude,
            });
        }
        events
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
                                Some(errors_count.as_ref()),
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
            csv, "ship", "csv-1", "id", "latitude", "longitude", None,
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
            csv, "ship", "csv-1", "id", "lat", "lon", None,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_csv_missing_columns() {
        let csv = "name,value\ntest,42";
        let events = CsvWatcherConnector::parse_csv_with_headers(
            csv, "sensor", "csv-1", "name", "lat", "lon", None,
        );
        assert_eq!(events.len(), 1);
        assert!(events[0].latitude.is_none());
    }

    /// Regression: malformed rows must be logged + counted via the
    /// `parse_errors` parameter, never silently dropped. We trip a real
    /// `csv::Error` with non-UTF-8 bytes in a field (`flexible(true)`
    /// already absorbs ragged rows, so UTF-8 is the realistic trigger).
    #[test]
    fn test_parse_csv_with_headers_logs_malformed_rows() {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"id,latitude,longitude,name\n");
        bytes.extend_from_slice(b"ship-1,51.92,4.48,Good Ship\n");
        bytes.extend_from_slice(b"ship-2,52.00,4.50,");
        bytes.extend_from_slice(&[0xFF, 0xFE]);
        bytes.extend_from_slice(b"\nship-3,53.10,4.60,Another Good Ship\n");

        let counter = AtomicU64::new(0);
        let events = CsvWatcherConnector::parse_csv_bytes_with_headers(
            &bytes, "ship", "csv-malformed", "id", "latitude", "longitude",
            Some(&counter),
        );
        let ids: Vec<&str> = events.iter().map(|e| e.entity_id.as_str()).collect();
        assert!(ids.contains(&"ship:ship-1"),
            "good row before the malformed one was dropped: {:?}", ids);
        assert_eq!(counter.load(Ordering::Relaxed), 1,
            "expected exactly one parse error (events={:?})", ids);
    }
}
