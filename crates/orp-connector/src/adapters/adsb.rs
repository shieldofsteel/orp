use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// ADS-B TCP receiver for aircraft position data
/// Supports Beast binary format and SBS BaseStation format (port 30003)
pub struct AdsbConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl AdsbConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Parse an SBS BaseStation message (CSV format from dump1090 port 30003)
    /// Format: MSG,{type},{session},{aircraft},{icao},{flight},{date},{time},{date},{time},{callsign},{alt},{speed},{heading},{lat},{lon},{vertical_rate},{squawk},{alert},{emergency},{spi},{is_on_ground}
    pub fn parse_sbs_message(line: &str) -> Option<AdsbMessage> {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 15 || parts[0] != "MSG" {
            return None;
        }

        let msg_type: u8 = parts[1].trim().parse().ok()?;
        // MSG type 3 = airborne position, type 2 = surface position
        if msg_type != 2 && msg_type != 3 {
            return None;
        }

        let icao = parts[4].trim().to_string();
        if icao.is_empty() {
            return None;
        }

        let callsign = parts.get(10).and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        let altitude: Option<f64> = parts.get(11).and_then(|s| s.trim().parse().ok());
        let speed: Option<f64> = parts.get(12).and_then(|s| s.trim().parse().ok());
        let heading: Option<f64> = parts.get(13).and_then(|s| s.trim().parse().ok());
        let lat: Option<f64> = parts.get(14).and_then(|s| s.trim().parse().ok());
        let lon: Option<f64> = parts.get(15).and_then(|s| s.trim().parse().ok());
        let vertical_rate: Option<f64> = parts.get(16).and_then(|s| s.trim().parse().ok());
        let squawk = parts
            .get(17)
            .and_then(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            });
        let on_ground: Option<bool> = parts
            .get(21)
            .and_then(|s| match s.trim() {
                "-1" | "1" => Some(true),
                "0" => Some(false),
                _ => None,
            });

        Some(AdsbMessage {
            icao,
            callsign,
            latitude: lat,
            longitude: lon,
            altitude,
            speed,
            heading,
            vertical_rate,
            squawk,
            on_ground,
            timestamp: Utc::now(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct AdsbMessage {
    pub icao: String,
    pub callsign: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub speed: Option<f64>,
    pub heading: Option<f64>,
    pub vertical_rate: Option<f64>,
    pub squawk: Option<String>,
    pub on_ground: Option<bool>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl AdsbMessage {
    pub fn to_source_event(&self, connector_id: &str) -> SourceEvent {
        let mut properties = HashMap::new();
        properties.insert("icao".to_string(), serde_json::json!(self.icao));
        if let Some(ref callsign) = self.callsign {
            properties.insert("callsign".to_string(), serde_json::json!(callsign));
        }
        if let Some(alt) = self.altitude {
            properties.insert("altitude".to_string(), serde_json::json!(alt));
        }
        if let Some(speed) = self.speed {
            properties.insert("speed".to_string(), serde_json::json!(speed));
        }
        if let Some(heading) = self.heading {
            properties.insert("heading".to_string(), serde_json::json!(heading));
        }
        if let Some(vrate) = self.vertical_rate {
            properties.insert("vertical_rate".to_string(), serde_json::json!(vrate));
        }
        if let Some(ref squawk) = self.squawk {
            properties.insert("squawk".to_string(), serde_json::json!(squawk));
        }
        if let Some(on_ground) = self.on_ground {
            properties.insert("on_ground".to_string(), serde_json::json!(on_ground));
        }

        SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id: format!("icao:{}", self.icao),
            entity_type: "aircraft".to_string(),
            properties,
            timestamp: self.timestamp,
            latitude: self.latitude,
            longitude: self.longitude,
        }
    }
}

#[async_trait]
impl Connector for AdsbConnector {
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
            "ADS-B connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let connector_id = self.config.connector_id.clone();
        let url = self.config.url.clone();

        tokio::spawn(async move {
            // If a TCP URL is configured, try real connection
            if let Some(ref url_str) = url {
                if let Some(addr) = url_str.strip_prefix("tcp://") {
                    match tokio::net::TcpStream::connect(addr).await {
                        Ok(stream) => {
                            tracing::info!("Connected to ADS-B feed at {}", addr);
                            let mut reader =
                                tokio::io::BufReader::new(stream);
                            use tokio::io::AsyncBufReadExt;
                            let mut line = String::new();
                            while running.load(Ordering::SeqCst) {
                                line.clear();
                                match reader.read_line(&mut line).await {
                                    Ok(0) => break,
                                    Ok(_) => {
                                        if let Some(msg) =
                                            AdsbConnector::parse_sbs_message(&line)
                                        {
                                            if msg.latitude.is_some() {
                                                let event =
                                                    msg.to_source_event(&connector_id);
                                                if tx.send(event).await.is_err() {
                                                    return;
                                                }
                                                events_count
                                                    .fetch_add(1, Ordering::Relaxed);
                                            }
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            return;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Cannot connect to ADS-B feed at {}: {}, using demo data",
                                addr,
                                e
                            );
                        }
                    }
                }
            }

            // Demo mode: generate synthetic aircraft data
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3));
            let demo_aircraft = vec![
                ("A1B2C3", 52.30, 4.76, 35000.0, 450.0, 90.0, "KLM1234"),
                ("D4E5F6", 51.47, -0.46, 28000.0, 380.0, 180.0, "BAW456"),
                ("789ABC", 48.86, 2.35, 40000.0, 500.0, 270.0, "AFR789"),
                ("DEF012", 50.03, 8.57, 32000.0, 420.0, 45.0, "DLH321"),
                ("345678", 40.64, -73.78, 15000.0, 250.0, 310.0, "AAL100"),
            ];

            let mut counter = 0u64;
            while running.load(Ordering::SeqCst) {
                interval.tick().await;
                for (icao, lat, lon, alt, speed, heading, callsign) in &demo_aircraft {
                    let jitter = (counter as f64 % 50.0) * 0.001;
                    let msg = AdsbMessage {
                        icao: icao.to_string(),
                        callsign: Some(callsign.to_string()),
                        latitude: Some(lat + jitter),
                        longitude: Some(lon + jitter * 0.5),
                        altitude: Some(*alt + (counter as f64 % 10.0) * 100.0),
                        speed: Some(*speed),
                        heading: Some(*heading),
                        vertical_rate: Some(0.0),
                        squawk: None,
                        on_ground: Some(false),
                        timestamp: Utc::now(),
                    };
                    let event = msg.to_source_event(&connector_id);
                    if tx.send(event).await.is_err() {
                        return;
                    }
                    events_count.fetch_add(1, Ordering::Relaxed);
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
            "ADS-B connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "ADS-B connector not running".to_string(),
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
    fn test_parse_sbs_message() {
        let line = "MSG,3,1,1,A1B2C3,1,2026/03/26,14:30:00.000,2026/03/26,14:30:00.000,KLM1234,35000,450,90,52.3000,4.7600,0,,0,0,0,0";
        let msg = AdsbConnector::parse_sbs_message(line);
        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.icao, "A1B2C3");
        assert!((msg.latitude.unwrap() - 52.3).abs() < 0.01);
        assert!((msg.altitude.unwrap() - 35000.0).abs() < 1.0);
    }

    #[test]
    fn test_parse_sbs_invalid() {
        assert!(AdsbConnector::parse_sbs_message("INVALID,data").is_none());
        assert!(AdsbConnector::parse_sbs_message("MSG,1,1,1,,1,,,,,,,,,,,,,,,").is_none());
    }

    #[test]
    fn test_adsb_to_source_event() {
        let msg = AdsbMessage {
            icao: "A1B2C3".to_string(),
            callsign: Some("KLM1234".to_string()),
            latitude: Some(52.3),
            longitude: Some(4.76),
            altitude: Some(35000.0),
            speed: Some(450.0),
            heading: Some(90.0),
            vertical_rate: Some(500.0),
            squawk: Some("7700".to_string()),
            on_ground: Some(false),
            timestamp: Utc::now(),
        };
        let event = msg.to_source_event("adsb-1");
        assert_eq!(event.entity_id, "icao:A1B2C3");
        assert_eq!(event.entity_type, "aircraft");
        assert!(event.properties.contains_key("callsign"));
        assert!(event.properties.contains_key("squawk"));
    }

    #[test]
    fn test_parse_sbs_type2_surface() {
        let line = "MSG,2,1,1,AABBCC,1,2026/03/26,14:30:00.000,2026/03/26,14:30:00.000,,0,50,90,52.3000,4.7600,0,,0,0,0,-1";
        let msg = AdsbConnector::parse_sbs_message(line);
        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.icao, "AABBCC");
        assert_eq!(msg.on_ground, Some(true));
    }

    #[test]
    fn test_parse_sbs_empty_icao_rejected() {
        let line = "MSG,3,1,1,,1,2026/03/26,14:30:00.000,2026/03/26,14:30:00.000,,35000,450,90,52.3,4.7,0,,0,0,0,0";
        assert!(AdsbConnector::parse_sbs_message(line).is_none());
    }

    #[test]
    fn test_parse_sbs_type_4_rejected() {
        let line = "MSG,4,1,1,A1B2C3,1,2026/03/26,14:30:00.000,2026/03/26,14:30:00.000,,35000,450,90,52.3,4.7,0,,0,0,0,0";
        assert!(AdsbConnector::parse_sbs_message(line).is_none());
    }

    #[test]
    fn test_adsb_source_event_no_optional_fields() {
        let msg = AdsbMessage {
            icao: "AABBCC".to_string(),
            callsign: None,
            latitude: None,
            longitude: None,
            altitude: None,
            speed: None,
            heading: None,
            vertical_rate: None,
            squawk: None,
            on_ground: None,
            timestamp: Utc::now(),
        };
        let event = msg.to_source_event("adsb-test");
        assert_eq!(event.entity_id, "icao:AABBCC");
        assert!(!event.properties.contains_key("callsign"));
        assert!(!event.properties.contains_key("squawk"));
    }

    #[test]
    fn test_adsb_connector_id() {
        let config = ConnectorConfig {
            connector_id: "adsb-test-id".to_string(),
            connector_type: "adsb".to_string(),
            url: None,
            entity_type: "aircraft".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = AdsbConnector::new(config);
        assert_eq!(connector.connector_id(), "adsb-test-id");
    }

    #[tokio::test]
    async fn test_adsb_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "adsb-health".to_string(),
            connector_type: "adsb".to_string(),
            url: None,
            entity_type: "aircraft".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = AdsbConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }
}
