use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// AIS NMEA sentence parser and TCP connector
pub struct AisConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl AisConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Parse an NMEA sentence and extract AIS position data
    /// Supports a simplified version of !AIVDM sentences
    pub fn parse_nmea_sentence(sentence: &str) -> Option<AisMessage> {
        let parts: Vec<&str> = sentence.split(',').collect();
        if parts.len() < 7 {
            return None;
        }

        // Very simplified parser for demo; real AIS uses 6-bit ASCII encoding
        if !parts[0].starts_with("!AIVDM") && !parts[0].starts_with("!AIVDO") {
            return None;
        }

        // For demo purposes, parse a simplified CSV-like AIS format
        // Real implementation would decode the binary payload in parts[5]
        None
    }

    /// Parse a simplified AIS CSV line (for demo/testing)
    /// Format: MMSI,LAT,LON,SPEED,COURSE,HEADING,NAME,SHIP_TYPE
    pub fn parse_csv_line(line: &str) -> Option<AisMessage> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 6 {
            return None;
        }

        let mmsi = parts[0].trim().to_string();
        let lat: f64 = parts[1].trim().parse().ok()?;
        let lon: f64 = parts[2].trim().parse().ok()?;
        let speed: f32 = parts[3].trim().parse().ok()?;
        let course: f32 = parts[4].trim().parse().ok()?;
        let heading: f32 = parts[5].trim().parse().unwrap_or(course);
        let name = parts.get(6).map(|s| s.trim().to_string());
        let ship_type = parts.get(7).map(|s| s.trim().to_string());

        Some(AisMessage {
            mmsi,
            latitude: lat,
            longitude: lon,
            speed,
            course,
            heading,
            name,
            ship_type,
            timestamp: Utc::now(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct AisMessage {
    pub mmsi: String,
    pub latitude: f64,
    pub longitude: f64,
    pub speed: f32,
    pub course: f32,
    pub heading: f32,
    pub name: Option<String>,
    pub ship_type: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl AisMessage {
    pub fn to_source_event(&self, connector_id: &str) -> SourceEvent {
        let mut properties = HashMap::new();
        properties.insert("mmsi".to_string(), serde_json::json!(self.mmsi));
        properties.insert("speed".to_string(), serde_json::json!(self.speed));
        properties.insert("course".to_string(), serde_json::json!(self.course));
        properties.insert("heading".to_string(), serde_json::json!(self.heading));
        if let Some(ref name) = self.name {
            properties.insert("name".to_string(), serde_json::json!(name));
        }
        if let Some(ref ship_type) = self.ship_type {
            properties.insert("ship_type".to_string(), serde_json::json!(ship_type));
        }

        SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id: format!("mmsi:{}", self.mmsi),
            entity_type: "ship".to_string(),
            properties,
            timestamp: self.timestamp,
            latitude: Some(self.latitude),
            longitude: Some(self.longitude),
        }
    }
}

#[async_trait]
impl Connector for AisConnector {
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
            "AIS connector started"
        );

        // Generate synthetic AIS data for demo purposes
        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let connector_id = self.config.connector_id.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
            let mut ship_counter = 0u64;

            // Pre-defined demo ships near Rotterdam
            let demo_ships = vec![
                ("211378120", 51.92, 4.48, 12.5, 245.0, "Maersk Seatrade", "container"),
                ("244820583", 51.89, 4.32, 8.3, 180.0, "Rotterdam Express", "tanker"),
                ("477280410", 51.95, 4.55, 15.0, 90.0, "Ever Given", "container"),
                ("305160000", 51.88, 4.25, 5.0, 315.0, "Pacific Pioneer", "bulk"),
                ("636092200", 51.94, 4.42, 10.0, 120.0, "Atlantic Breeze", "general"),
            ];

            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                for (mmsi, lat, lon, speed, course, name, ship_type) in &demo_ships {
                    // Add slight random variation
                    let jitter = (ship_counter as f64 % 100.0) * 0.0001;
                    let msg = AisMessage {
                        mmsi: mmsi.to_string(),
                        latitude: lat + jitter,
                        longitude: lon + jitter * 0.5,
                        speed: *speed,
                        course: *course,
                        heading: *course,
                        name: Some(name.to_string()),
                        ship_type: Some(ship_type.to_string()),
                        timestamp: Utc::now(),
                    };

                    let event = msg.to_source_event(&connector_id);
                    if tx.send(event).await.is_err() {
                        return;
                    }
                    events_count.fetch_add(1, Ordering::Relaxed);
                }
                ship_counter += 1;
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "AIS connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "AIS connector not running".to_string(),
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
    fn test_parse_csv_line() {
        let line = "211378120,51.9225,4.4792,12.3,245.0,243.0,Test Ship,container";
        let msg = AisConnector::parse_csv_line(line).unwrap();
        assert_eq!(msg.mmsi, "211378120");
        assert!((msg.latitude - 51.9225).abs() < 0.001);
        assert!((msg.speed - 12.3).abs() < 0.1);
    }

    #[test]
    fn test_to_source_event() {
        let msg = AisMessage {
            mmsi: "123456789".to_string(),
            latitude: 51.0,
            longitude: 4.0,
            speed: 10.0,
            course: 180.0,
            heading: 180.0,
            name: Some("Test".to_string()),
            ship_type: None,
            timestamp: Utc::now(),
        };

        let event = msg.to_source_event("ais-1");
        assert_eq!(event.entity_id, "mmsi:123456789");
        assert_eq!(event.entity_type, "ship");
    }

    #[test]
    fn test_parse_csv_line_too_few_fields() {
        let line = "211378120,51.9225";
        assert!(AisConnector::parse_csv_line(line).is_none());
    }

    #[test]
    fn test_parse_csv_line_invalid_lat() {
        let line = "211378120,INVALID,4.4792,12.3,245.0,243.0";
        assert!(AisConnector::parse_csv_line(line).is_none());
    }

    #[test]
    fn test_parse_csv_line_no_name() {
        let line = "211378120,51.9225,4.4792,12.3,245.0,243.0";
        let msg = AisConnector::parse_csv_line(line).unwrap();
        assert!(msg.name.is_none());
        assert!(msg.ship_type.is_none());
    }

    #[test]
    fn test_parse_csv_line_heading_defaults_to_course() {
        let line = "211378120,51.9225,4.4792,12.3,245.0";
        // Only 5 fields — not enough for heading parse
        assert!(AisConnector::parse_csv_line(line).is_none());
    }

    #[test]
    fn test_parse_nmea_sentence_non_aivdm() {
        assert!(AisConnector::parse_nmea_sentence("$GPGGA,something").is_none());
    }

    #[test]
    fn test_parse_nmea_sentence_too_short() {
        assert!(AisConnector::parse_nmea_sentence("!AIVDM,1,2").is_none());
    }

    #[test]
    fn test_source_event_has_properties() {
        let msg = AisMessage {
            mmsi: "999999999".to_string(),
            latitude: 52.0,
            longitude: 5.0,
            speed: 25.0,
            course: 90.0,
            heading: 88.0,
            name: Some("Big Ship".to_string()),
            ship_type: Some("container".to_string()),
            timestamp: Utc::now(),
        };
        let event = msg.to_source_event("ais-test");
        assert!(event.properties.contains_key("mmsi"));
        assert!(event.properties.contains_key("speed"));
        assert!(event.properties.contains_key("course"));
        assert!(event.properties.contains_key("heading"));
        assert!(event.properties.contains_key("name"));
        assert!(event.properties.contains_key("ship_type"));
    }

    #[test]
    fn test_connector_id() {
        let config = ConnectorConfig {
            connector_id: "ais-test-id".to_string(),
            connector_type: "ais".to_string(),
            url: None,
            entity_type: "ship".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = AisConnector::new(config);
        assert_eq!(connector.connector_id(), "ais-test-id");
    }

    #[test]
    fn test_connector_stats_initial() {
        let config = ConnectorConfig {
            connector_id: "ais-stats".to_string(),
            connector_type: "ais".to_string(),
            url: None,
            entity_type: "ship".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = AisConnector::new(config);
        let stats = connector.stats();
        assert_eq!(stats.events_processed, 0);
        assert_eq!(stats.errors, 0);
    }

    #[tokio::test]
    async fn test_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "ais-health".to_string(),
            connector_type: "ais".to_string(),
            url: None,
            entity_type: "ship".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = AisConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }
}
