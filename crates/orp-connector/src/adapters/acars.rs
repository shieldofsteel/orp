use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ACARS (Aircraft Communications Addressing and Reporting System) parser
// ---------------------------------------------------------------------------
// ACARS is a digital datalink system for transmission of short messages
// between aircraft and ground stations via VHF, HF, or SATCOM.
//
// Standard: ARINC 618, 619, 620, 622, 633
//
// Text format fields:
//   - Mode: single character identifying the transmission mode (2, H, etc.)
//   - Registration: aircraft tail number (up to 7 chars, with leading '.')
//   - Acknowledgement: single char (NAK = 0x15, or '!')
//   - Label: 2-character message type code (H1, SA, _d, Q0, etc.)
//   - Block ID: single character (0-9, A-Z)
//   - Message Number: sequence identifier
//   - Flight ID: airline + flight number
//   - Sublabel: 2-character sublabel (optional)
//   - Message Text: free-form payload
//
// This module supports two input formats:
//   1. Key-value text (MODE:, REG:, LABEL:, etc.)
//   2. Raw ACARS string (mode + reg + ack + label + block_id + ...)

/// Parsed ACARS message.
#[derive(Clone, Debug)]
pub struct AcarsMessage {
    pub mode: char,
    pub registration: String,
    pub label: String,
    pub block_id: Option<char>,
    pub acknowledgement: Option<char>,
    pub flight_id: Option<String>,
    pub message_number: Option<String>,
    pub message_text: String,
    pub sublabel: Option<String>,
}

impl AcarsMessage {
    /// Human-readable label description.
    pub fn label_description(&self) -> &str {
        match self.label.as_str() {
            "H1" => "HF Data Link Message",
            "SA" => "Departure / Arrival",
            "_d" => "Command / Control",
            "Q0" => "Link Test",
            "RA" => "Position Report",
            "5Z" => "Airline Designated Downlink",
            "5U" => "Weather Request",
            "80" => "OOOI (Out/Off/On/In) Report",
            "_\x7f" | "SQ" => "Squawk Message",
            "B6" => "Free Text Uplink",
            "BA" => "Advisory / Information",
            "MA" => "Meteorological Report",
            _ => "Unknown Label",
        }
    }

    /// Determine a mode description.
    pub fn mode_description(&self) -> &str {
        match self.mode {
            '2' => "VHF ACARS",
            'H' | 'C' | 'P' | 'Y' => "HF Data Link (HFDL)",
            'I' => "Inmarsat",
            'V' => "VDL Mode 2",
            _ => "Unknown Mode",
        }
    }
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse ACARS message from key-value text format.
///
/// Expected format (one field per line):
/// ```text
/// MODE: 2
/// REG: .N12345
/// ACK: !
/// LABEL: H1
/// BLOCK_ID: 3
/// MSGNO: M42A
/// FLIGHT: AA1234
/// SUBLABEL: DF
/// MSG: POSITION REPORT LAT 40.123 LON -73.456
/// ```
pub fn parse_acars_text(data: &str) -> Result<AcarsMessage, ConnectorError> {
    let mut mode: Option<char> = None;
    let mut registration: Option<String> = None;
    let mut label: Option<String> = None;
    let mut block_id: Option<char> = None;
    let mut ack: Option<char> = None;
    let mut flight_id: Option<String> = None;
    let mut message_number: Option<String> = None;
    let mut message_text = String::new();
    let mut sublabel: Option<String> = None;
    let mut in_msg = false;

    for line in data.lines() {
        let line = line.trim();
        if in_msg {
            if !message_text.is_empty() {
                message_text.push('\n');
            }
            message_text.push_str(line);
            continue;
        }

        if let Some(val) = line.strip_prefix("MODE:") {
            let val = val.trim();
            mode = val.chars().next();
        } else if let Some(val) = line.strip_prefix("REG:") {
            let val = val.trim().trim_start_matches('.');
            registration = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("ACK:") {
            let val = val.trim();
            ack = val.chars().next();
        } else if let Some(val) = line.strip_prefix("LABEL:") {
            label = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("BLOCK_ID:") {
            let val = val.trim();
            block_id = val.chars().next();
        } else if let Some(val) = line.strip_prefix("MSGNO:") {
            let val = val.trim();
            if !val.is_empty() {
                message_number = Some(val.to_string());
            }
        } else if let Some(val) = line.strip_prefix("FLIGHT:") {
            let val = val.trim();
            if !val.is_empty() {
                flight_id = Some(val.to_string());
            }
        } else if let Some(val) = line.strip_prefix("SUBLABEL:") {
            let val = val.trim();
            if !val.is_empty() {
                sublabel = Some(val.to_string());
            }
        } else if let Some(val) = line.strip_prefix("MSG:") {
            message_text = val.trim().to_string();
            in_msg = true;
        }
    }

    let mode = mode.ok_or_else(|| {
        ConnectorError::ParseError("ACARS: missing MODE field".into())
    })?;
    let registration = registration.ok_or_else(|| {
        ConnectorError::ParseError("ACARS: missing REG field".into())
    })?;
    let label = label.ok_or_else(|| {
        ConnectorError::ParseError("ACARS: missing LABEL field".into())
    })?;

    Ok(AcarsMessage {
        mode,
        registration,
        label,
        block_id,
        acknowledgement: ack,
        flight_id,
        message_number,
        message_text,
        sublabel,
    })
}

/// Parse raw ACARS string.
///
/// Raw format: `<mode><reg (7 chars, padded)><ack><label (2 chars)><block_id><msgno (4 chars)><flight (6 chars)><text>`
///
/// Example: `2.N12345 !H13M42AAA1234POSITION REPORT`
pub fn parse_acars_raw(data: &str) -> Option<AcarsMessage> {
    let data = data.trim();
    if data.len() < 13 {
        return None;
    }

    let chars: Vec<char> = data.chars().collect();
    let mode = chars[0];

    // Registration is chars 1..8 (7 chars, may have leading '.')
    let reg_raw: String = chars[1..8].iter().collect();
    let registration = reg_raw.trim().trim_start_matches('.').to_string();
    if registration.is_empty() {
        return None;
    }

    let ack = chars[8];
    let label: String = chars[9..11].iter().collect();
    let block_id = chars[11];

    // Message number: next 4 chars (optional, may be spaces)
    let msgno_raw: String = if chars.len() > 15 {
        chars[12..16].iter().collect()
    } else {
        String::new()
    };
    let message_number = if msgno_raw.trim().is_empty() {
        None
    } else {
        Some(msgno_raw.trim().to_string())
    };

    // Flight ID: next 6 chars (optional)
    let flight_raw: String = if chars.len() > 21 {
        chars[16..22].iter().collect()
    } else if chars.len() > 16 {
        chars[16..].iter().collect()
    } else {
        String::new()
    };
    let flight_id = if flight_raw.trim().is_empty() {
        None
    } else {
        Some(flight_raw.trim().to_string())
    };

    // Message text: everything after flight
    let text_start = 22.min(chars.len());
    let message_text: String = chars[text_start..].iter().collect();

    Some(AcarsMessage {
        mode,
        registration,
        label,
        block_id: Some(block_id),
        acknowledgement: Some(ack),
        flight_id,
        message_number,
        message_text: message_text.trim().to_string(),
        sublabel: None,
    })
}

// ---------------------------------------------------------------------------
// ACARS → SourceEvent
// ---------------------------------------------------------------------------

/// Convert an ACARS message to a SourceEvent.
pub fn acars_to_source_event(
    msg: &AcarsMessage,
    connector_id: &str,
) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert("mode".into(), json!(msg.mode.to_string()));
    properties.insert("mode_description".into(), json!(msg.mode_description()));
    properties.insert("registration".into(), json!(msg.registration));
    properties.insert("label".into(), json!(msg.label));
    properties.insert("label_description".into(), json!(msg.label_description()));
    properties.insert("message_text".into(), json!(msg.message_text));

    if let Some(bid) = msg.block_id {
        properties.insert("block_id".into(), json!(bid.to_string()));
    }
    if let Some(ref ack) = msg.acknowledgement {
        properties.insert("acknowledgement".into(), json!(ack.to_string()));
    }
    if let Some(ref fid) = msg.flight_id {
        properties.insert("flight_id".into(), json!(fid));
    }
    if let Some(ref mno) = msg.message_number {
        properties.insert("message_number".into(), json!(mno));
    }
    if let Some(ref sub) = msg.sublabel {
        properties.insert("sublabel".into(), json!(sub));
    }

    let entity_id = format!("acars:{}", msg.registration);

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "aircraft".to_string(),
        properties,
        timestamp: Utc::now(),
        latitude: None,
        longitude: None,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct AcarsConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl AcarsConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_processed: Arc::new(AtomicU64::new(0)),
            errors: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl Connector for AcarsConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        let path = self
            .config
            .url
            .as_deref()
            .ok_or_else(|| {
                ConnectorError::ConfigError("ACARS: url (file path) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        // Split on double-newlines (each ACARS message is a block)
        let blocks: Vec<&str> = content.split("\n\n").collect();
        for block in blocks {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let block = block.trim();
            if block.is_empty() {
                continue;
            }

            match parse_acars_text(block) {
                Ok(msg) => {
                    let event = acars_to_source_event(&msg, &connector_id);
                    if tx.send(event).await.is_err() {
                        break;
                    }
                    events_processed.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    // Try raw format
                    if let Some(msg) = parse_acars_raw(block) {
                        let event = acars_to_source_event(&msg, &connector_id);
                        if tx.send(event).await.is_err() {
                            break;
                        }
                        events_processed.fetch_add(1, Ordering::Relaxed);
                    } else {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }

        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ConnectorError::ConnectionError(
                "ACARS connector is not running".into(),
            ));
        }
        Ok(())
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_acars_text_basic() {
        let data = "MODE: 2\nREG: .N12345\nLABEL: H1\nBLOCK_ID: 3\nMSG: TEST MESSAGE";
        let msg = parse_acars_text(data).unwrap();
        assert_eq!(msg.mode, '2');
        assert_eq!(msg.registration, "N12345");
        assert_eq!(msg.label, "H1");
        assert_eq!(msg.block_id, Some('3'));
        assert_eq!(msg.message_text, "TEST MESSAGE");
    }

    #[test]
    fn test_parse_acars_text_position() {
        let data = "MODE: 2\nREG: .N67890\nLABEL: RA\nFLIGHT: UA456\nMSG: POS N40123W073456 FL350";
        let msg = parse_acars_text(data).unwrap();
        assert_eq!(msg.label, "RA");
        assert_eq!(msg.flight_id, Some("UA456".to_string()));
        assert!(msg.message_text.contains("POS"));
    }

    #[test]
    fn test_parse_acars_text_missing_optional() {
        let data = "MODE: H\nREG: .GABCD\nLABEL: SA\nMSG: DEPARTURE";
        let msg = parse_acars_text(data).unwrap();
        assert_eq!(msg.mode, 'H');
        assert_eq!(msg.registration, "GABCD");
        assert!(msg.flight_id.is_none());
        assert!(msg.block_id.is_none());
        assert!(msg.message_number.is_none());
    }

    #[test]
    fn test_parse_acars_raw_basic() {
        // mode='2', reg='.N12345' (7 chars at positions 1-7), ack='!' (pos 8),
        // label='H1' (pos 9-10), block='3' (pos 11), msgno='M42A', flight='AA1234', text
        let raw = "2.N12345!H13M42AAA1234POSITION REPORT";
        let msg = parse_acars_raw(raw).unwrap();
        assert_eq!(msg.mode, '2');
        assert_eq!(msg.registration, "N12345");
        assert_eq!(msg.label, "H1");
        assert_eq!(msg.block_id, Some('3'));
        assert_eq!(msg.acknowledgement, Some('!'));
    }

    #[test]
    fn test_parse_acars_raw_short() {
        // Too short to parse
        let raw = "2.N12";
        assert!(parse_acars_raw(raw).is_none());
    }

    #[test]
    fn test_acars_to_source_event() {
        let msg = AcarsMessage {
            mode: '2',
            registration: "N12345".into(),
            label: "H1".into(),
            block_id: Some('3'),
            acknowledgement: Some('!'),
            flight_id: Some("AA100".into()),
            message_number: Some("M42A".into()),
            message_text: "TEST".into(),
            sublabel: None,
        };
        let event = acars_to_source_event(&msg, "acars-test");
        assert_eq!(event.entity_id, "acars:N12345");
        assert_eq!(event.connector_id, "acars-test");
        assert_eq!(event.properties["flight_id"], json!("AA100"));
    }

    #[test]
    fn test_acars_entity_type() {
        let msg = AcarsMessage {
            mode: '2',
            registration: "N99999".into(),
            label: "H1".into(),
            block_id: None,
            acknowledgement: None,
            flight_id: None,
            message_number: None,
            message_text: "".into(),
            sublabel: None,
        };
        let event = acars_to_source_event(&msg, "test");
        assert_eq!(event.entity_type, "aircraft");
    }

    #[test]
    fn test_acars_mode_types() {
        let msg_vhf = AcarsMessage {
            mode: '2',
            registration: "N1".into(),
            label: "H1".into(),
            block_id: None,
            acknowledgement: None,
            flight_id: None,
            message_number: None,
            message_text: "".into(),
            sublabel: None,
        };
        assert_eq!(msg_vhf.mode_description(), "VHF ACARS");

        let msg_hf = AcarsMessage {
            mode: 'H',
            registration: "N2".into(),
            label: "H1".into(),
            block_id: None,
            acknowledgement: None,
            flight_id: None,
            message_number: None,
            message_text: "".into(),
            sublabel: None,
        };
        assert_eq!(msg_hf.mode_description(), "HF Data Link (HFDL)");

        let msg_inm = AcarsMessage {
            mode: 'I',
            registration: "N3".into(),
            label: "H1".into(),
            block_id: None,
            acknowledgement: None,
            flight_id: None,
            message_number: None,
            message_text: "".into(),
            sublabel: None,
        };
        assert_eq!(msg_inm.mode_description(), "Inmarsat");
    }

    #[test]
    fn test_parse_empty_message() {
        assert!(parse_acars_text("").is_err());
        assert!(parse_acars_raw("").is_none());
    }

    #[test]
    fn test_acars_label_types() {
        let labels = vec![
            ("H1", "HF Data Link Message"),
            ("SA", "Departure / Arrival"),
            ("_d", "Command / Control"),
            ("Q0", "Link Test"),
            ("RA", "Position Report"),
            ("80", "OOOI (Out/Off/On/In) Report"),
            ("ZZ", "Unknown Label"),
        ];

        for (label, expected) in labels {
            let msg = AcarsMessage {
                mode: '2',
                registration: "N1".into(),
                label: label.into(),
                block_id: None,
                acknowledgement: None,
                flight_id: None,
                message_number: None,
                message_text: "".into(),
                sublabel: None,
            };
            assert_eq!(msg.label_description(), expected);
        }
    }

    #[test]
    fn test_acars_connector_id() {
        let config = ConnectorConfig {
            connector_id: "acars-1".to_string(),
            connector_type: "acars".to_string(),
            url: None,
            entity_type: "aircraft".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = AcarsConnector::new(config);
        assert_eq!(connector.connector_id(), "acars-1");
    }

    #[tokio::test]
    async fn test_acars_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "acars-h".to_string(),
            connector_type: "acars".to_string(),
            url: None,
            entity_type: "aircraft".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = AcarsConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_parse_acars_text_with_sublabel() {
        let data = "MODE: 2\nREG: .N55555\nLABEL: H1\nSUBLABEL: DF\nMSG: ADS-C REPORT";
        let msg = parse_acars_text(data).unwrap();
        assert_eq!(msg.sublabel, Some("DF".to_string()));
    }
}
