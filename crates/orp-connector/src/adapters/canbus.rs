use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CAN Bus / J1939 parser
// ---------------------------------------------------------------------------
// CAN (Controller Area Network) is the dominant in-vehicle communication bus.
// ISO 11898 defines the physical and data-link layer.
//
// CAN frame layout:
//   - Standard (CAN 2.0A): 11-bit arbitration ID, up to 8 bytes data
//   - Extended (CAN 2.0B): 29-bit arbitration ID, up to 8 bytes data
//   - CAN FD: up to 64 bytes data
//
// SAE J1939 (heavy-duty vehicles) uses extended CAN frames with:
//   - 29-bit ID decomposed as: priority (3) | reserved (1) | data_page (1) |
//     PDU format (8) | PDU specific (8) | source address (8)
//   - PGN (Parameter Group Number) encodes the message type
//   - SPN (Suspect Parameter Number) encodes individual data fields
//
// Since SocketCAN is Linux-only, this module provides:
//   1. CAN frame parsing from binary/log data
//   2. J1939 PGN decoding for common engine/vehicle parameters
//   3. A connector that reads CAN dump files or log streams

/// CAN frame type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanFrameType {
    Standard,  // 11-bit ID
    Extended,  // 29-bit ID
}

/// Parsed CAN frame.
#[derive(Clone, Debug)]
pub struct CanFrame {
    pub frame_type: CanFrameType,
    pub id: u32,
    pub dlc: u8,        // data length code (0–8, or 0–64 for CAN FD)
    pub data: Vec<u8>,
    pub timestamp: Option<f64>, // optional timestamp (seconds)
}

impl CanFrame {
    /// Check if this is a J1939 extended frame.
    pub fn is_j1939(&self) -> bool {
        self.frame_type == CanFrameType::Extended
    }
}

/// J1939 decoded header from a 29-bit extended CAN ID.
#[derive(Clone, Debug)]
pub struct J1939Header {
    pub priority: u8,
    pub reserved: bool,
    pub data_page: bool,
    pub pdu_format: u8,
    pub pdu_specific: u8,
    pub source_address: u8,
    pub pgn: u32,
}

/// Well-known J1939 PGN definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum J1939Pgn {
    EngineSpeed,               // PGN 61444 (0xF004)
    VehicleSpeed,              // PGN 65265 (0xFEF1)
    EngineTemperature,         // PGN 65262 (0xFEEE)
    FuelConsumption,           // PGN 65266 (0xFEF2)
    TransmissionOilTemp,       // PGN 65272 (0xFEF8)
    BrakeSystemPressure,       // PGN 65267 (0xFEF3)
    AmbientConditions,         // PGN 65269 (0xFEF5)
    VehicleElectricalPower,    // PGN 65271 (0xFEF7)
    DashDisplay,               // PGN 65276 (0xFEFC)
    EngineHours,               // PGN 65253 (0xFEE5)
    Unknown(u32),
}

impl J1939Pgn {
    pub fn from_pgn(pgn: u32) -> Self {
        match pgn {
            61444 => J1939Pgn::EngineSpeed,
            65265 => J1939Pgn::VehicleSpeed,
            65262 => J1939Pgn::EngineTemperature,
            65266 => J1939Pgn::FuelConsumption,
            65272 => J1939Pgn::TransmissionOilTemp,
            65267 => J1939Pgn::BrakeSystemPressure,
            65269 => J1939Pgn::AmbientConditions,
            65271 => J1939Pgn::VehicleElectricalPower,
            65276 => J1939Pgn::DashDisplay,
            65253 => J1939Pgn::EngineHours,
            _ => J1939Pgn::Unknown(pgn),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            J1939Pgn::EngineSpeed => "Engine Speed",
            J1939Pgn::VehicleSpeed => "Vehicle Speed",
            J1939Pgn::EngineTemperature => "Engine Temperature",
            J1939Pgn::FuelConsumption => "Fuel Consumption",
            J1939Pgn::TransmissionOilTemp => "Transmission Oil Temperature",
            J1939Pgn::BrakeSystemPressure => "Brake System Pressure",
            J1939Pgn::AmbientConditions => "Ambient Conditions",
            J1939Pgn::VehicleElectricalPower => "Vehicle Electrical Power",
            J1939Pgn::DashDisplay => "Dash Display",
            J1939Pgn::EngineHours => "Engine Hours",
            J1939Pgn::Unknown(_) => "Unknown PGN",
        }
    }
}

/// Decoded J1939 data values from a specific PGN.
#[derive(Clone, Debug)]
pub struct J1939DecodedData {
    pub pgn: J1939Pgn,
    pub pgn_number: u32,
    pub source_address: u8,
    pub values: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Decode J1939 header from a 29-bit CAN ID.
pub fn decode_j1939_id(can_id: u32) -> J1939Header {
    let priority = ((can_id >> 26) & 0x07) as u8;
    let reserved = (can_id >> 25) & 0x01 != 0;
    let data_page = (can_id >> 24) & 0x01 != 0;
    let pdu_format = ((can_id >> 16) & 0xFF) as u8;
    let pdu_specific = ((can_id >> 8) & 0xFF) as u8;
    let source_address = (can_id & 0xFF) as u8;

    // PGN calculation
    let pgn = if pdu_format < 240 {
        // PDU1 (peer-to-peer): PGN does not include PS (destination address)
        let dp = if data_page { 1u32 } else { 0 };
        let rp = if reserved { 1u32 } else { 0 };
        (rp << 17) | (dp << 16) | ((pdu_format as u32) << 8)
    } else {
        // PDU2 (broadcast): PGN includes PS (group extension)
        let dp = if data_page { 1u32 } else { 0 };
        let rp = if reserved { 1u32 } else { 0 };
        (rp << 17) | (dp << 16) | ((pdu_format as u32) << 8) | (pdu_specific as u32)
    };

    J1939Header {
        priority,
        reserved,
        data_page,
        pdu_format,
        pdu_specific,
        source_address,
        pgn,
    }
}

/// Decode J1939 PGN data into human-readable values.
pub fn decode_j1939_data(header: &J1939Header, data: &[u8]) -> J1939DecodedData {
    let pgn_enum = J1939Pgn::from_pgn(header.pgn);
    let mut values = HashMap::new();

    match pgn_enum {
        J1939Pgn::EngineSpeed => {
            // PGN 61444 — EEC1 (Electronic Engine Controller 1)
            // SPN 190: Engine Speed — bytes 3-4, resolution 0.125 RPM
            if data.len() >= 4 {
                let raw = if data.len() > 4 {
                    u16::from_le_bytes([data[3], data[4]])
                } else {
                    u16::from_le_bytes([data[3], 0])
                };
                let rpm = raw as f64 * 0.125;
                values.insert("engine_speed_rpm".into(), json!(rpm));
            }
            // SPN 899: Engine Torque Mode — byte 0 bits 0-3
            if !data.is_empty() {
                values.insert("torque_mode".into(), json!(data[0] & 0x0F));
            }
        }
        J1939Pgn::VehicleSpeed => {
            // PGN 65265 — CCVS (Cruise Control/Vehicle Speed)
            // SPN 84: Wheel-Based Vehicle Speed — bytes 1-2, resolution 1/256 km/h
            if data.len() >= 3 {
                let raw = u16::from_le_bytes([data[1], data[2]]);
                let speed_kmh = raw as f64 / 256.0;
                values.insert("vehicle_speed_kmh".into(), json!(speed_kmh));
            }
        }
        J1939Pgn::EngineTemperature => {
            // PGN 65262 — ET1 (Engine Temperature 1)
            // SPN 110: Engine Coolant Temperature — byte 0, offset -40°C
            if !data.is_empty() {
                let temp_c = data[0] as i16 - 40;
                values.insert("coolant_temp_c".into(), json!(temp_c));
            }
            // SPN 175: Engine Oil Temperature — bytes 2-3, offset -273°C, resolution 0.03125
            if data.len() >= 4 {
                let raw = u16::from_le_bytes([data[2], data[3]]);
                let oil_temp_c = raw as f64 * 0.03125 - 273.0;
                values.insert("oil_temp_c".into(), json!(oil_temp_c));
            }
        }
        J1939Pgn::FuelConsumption => {
            // PGN 65266 — LFE (Fuel Economy)
            // SPN 183: Fuel Rate — bytes 0-1, resolution 0.05 L/h
            if data.len() >= 2 {
                let raw = u16::from_le_bytes([data[0], data[1]]);
                let fuel_rate_lph = raw as f64 * 0.05;
                values.insert("fuel_rate_lph".into(), json!(fuel_rate_lph));
            }
        }
        J1939Pgn::AmbientConditions => {
            // PGN 65269 — AMB (Ambient Conditions)
            // SPN 171: Ambient Air Temperature — bytes 3-4, offset -273°C, resolution 0.03125
            if data.len() >= 5 {
                let raw = u16::from_le_bytes([data[3], data[4]]);
                let temp_c = raw as f64 * 0.03125 - 273.0;
                values.insert("ambient_temp_c".into(), json!(temp_c));
            }
        }
        J1939Pgn::EngineHours => {
            // PGN 65253 — HOURS (Engine Hours, Revolutions)
            // SPN 247: Engine Total Hours — bytes 0-3, resolution 0.05 hrs
            if data.len() >= 4 {
                let raw = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                let hours = raw as f64 * 0.05;
                values.insert("engine_total_hours".into(), json!(hours));
            }
        }
        _ => {
            // Unknown PGN: store raw hex data
            let hex: String = data.iter().map(|b| format!("{:02X}", b)).collect();
            values.insert("raw_data".into(), json!(hex));
        }
    }

    values.insert("pgn_name".into(), json!(pgn_enum.name()));

    J1939DecodedData {
        pgn: pgn_enum,
        pgn_number: header.pgn,
        source_address: header.source_address,
        values,
    }
}

/// Parse a candump log line.
/// Format: `(timestamp) interface CAN_ID#DATA_HEX`
/// Example: `(1700000000.123456) can0 0CF00400#FF7D7DFFFFFFF000`
pub fn parse_candump_line(line: &str) -> Option<CanFrame> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    // Timestamp
    let timestamp = parts[0]
        .trim_start_matches('(')
        .trim_end_matches(')')
        .parse::<f64>()
        .ok();

    // CAN_ID#DATA
    let frame_part = parts[2]; // e.g., "0CF00400#FF7D7DFFFFFFF000"
    let hash_pos = frame_part.find('#')?;
    let id_str = &frame_part[..hash_pos];
    let data_str = &frame_part[hash_pos + 1..];

    let id = u32::from_str_radix(id_str, 16).ok()?;
    let frame_type = if id > 0x7FF {
        CanFrameType::Extended
    } else {
        CanFrameType::Standard
    };

    let data: Vec<u8> = (0..data_str.len())
        .step_by(2)
        .filter_map(|i| {
            if i + 2 <= data_str.len() {
                u8::from_str_radix(&data_str[i..i + 2], 16).ok()
            } else {
                None
            }
        })
        .collect();

    Some(CanFrame {
        frame_type,
        id,
        dlc: data.len() as u8,
        data,
        timestamp,
    })
}

/// Parse raw CAN frame bytes (16 bytes: 4 ID + 1 DLC + 3 pad + 8 data).
pub fn parse_can_frame_bytes(data: &[u8]) -> Option<CanFrame> {
    if data.len() < 16 {
        return None;
    }
    let id = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let is_extended = id & 0x80000000 != 0;
    let clean_id = id & 0x1FFFFFFF;
    let dlc = data[4].min(8);
    let frame_data = data[8..8 + dlc as usize].to_vec();

    Some(CanFrame {
        frame_type: if is_extended {
            CanFrameType::Extended
        } else {
            CanFrameType::Standard
        },
        id: clean_id,
        dlc,
        data: frame_data,
        timestamp: None,
    })
}

// ---------------------------------------------------------------------------
// CAN frame → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a CAN frame (with optional J1939 decoding) to a SourceEvent.
pub fn can_frame_to_source_event(
    frame: &CanFrame,
    connector_id: &str,
) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert("can_id".into(), json!(format!("{:08X}", frame.id)));
    properties.insert("dlc".into(), json!(frame.dlc));

    let hex: String = frame.data.iter().map(|b| format!("{:02X}", b)).collect();
    properties.insert("raw_data".into(), json!(hex));

    let (entity_type, entity_id) = if frame.is_j1939() {
        let header = decode_j1939_id(frame.id);
        let decoded = decode_j1939_data(&header, &frame.data);

        properties.insert("pgn".into(), json!(header.pgn));
        properties.insert("pgn_name".into(), json!(decoded.pgn.name()));
        properties.insert("source_address".into(), json!(header.source_address));
        properties.insert("priority".into(), json!(header.priority));

        for (k, v) in &decoded.values {
            if k != "pgn_name" {
                properties.insert(k.clone(), v.clone());
            }
        }

        let etype = match decoded.pgn {
            J1939Pgn::EngineSpeed
            | J1939Pgn::EngineTemperature
            | J1939Pgn::EngineHours
            | J1939Pgn::FuelConsumption => "engine",
            _ => "vehicle",
        };
        (
            etype.to_string(),
            format!("canbus:j1939:sa-{}", header.source_address),
        )
    } else {
        (
            "vehicle".to_string(),
            format!("canbus:{:03X}", frame.id),
        )
    };

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type,
        properties,
        timestamp: frame
            .timestamp
            .and_then(|t| {
                chrono::DateTime::from_timestamp(t.trunc() as i64, (t.fract() * 1e9) as u32)
            })
            .unwrap_or_else(Utc::now),
        latitude: None,
        longitude: None,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct CanBusConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl CanBusConnector {
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
impl Connector for CanBusConnector {
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
                ConnectorError::ConfigError("CAN Bus: url (log file path) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let running = Arc::clone(&self.running);

        for line in content.lines() {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            if let Some(frame) = parse_candump_line(line) {
                let event = can_frame_to_source_event(&frame, &connector_id);
                if tx.send(event).await.is_err() {
                    break;
                }
                events_processed.fetch_add(1, Ordering::Relaxed);
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
                "CAN Bus connector is not running".into(),
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
    fn test_parse_candump_line_extended() {
        let line = "(1700000000.123456) can0 0CF00400#FF7D7DFFFFFFF000";
        let frame = parse_candump_line(line).unwrap();
        assert_eq!(frame.frame_type, CanFrameType::Extended);
        assert_eq!(frame.id, 0x0CF00400);
        assert_eq!(frame.dlc, 8);
        assert_eq!(frame.data.len(), 8);
        assert!((frame.timestamp.unwrap() - 1700000000.123456).abs() < 0.001);
    }

    #[test]
    fn test_parse_candump_line_standard() {
        let line = "(1700000001.000000) can0 1A3#AABB";
        let frame = parse_candump_line(line).unwrap();
        assert_eq!(frame.frame_type, CanFrameType::Standard);
        assert_eq!(frame.id, 0x1A3);
        assert_eq!(frame.dlc, 2);
        assert_eq!(frame.data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_parse_candump_empty() {
        assert!(parse_candump_line("").is_none());
        assert!(parse_candump_line("# comment").is_none());
    }

    #[test]
    fn test_decode_j1939_id_engine_speed() {
        // PGN 61444 = 0xF004, priority 3, SA 0
        let can_id: u32 = 0x0CF00400; // priority=3, PGN=F004, SA=0
        let header = decode_j1939_id(can_id);
        assert_eq!(header.priority, 3);
        assert_eq!(header.pgn, 61444);
        assert_eq!(header.source_address, 0);
    }

    #[test]
    fn test_decode_j1939_id_vehicle_speed() {
        // PGN 65265 = 0xFEF1, priority 6, SA 0
        let can_id: u32 = 0x18FEF100;
        let header = decode_j1939_id(can_id);
        assert_eq!(header.pgn, 65265);
        assert_eq!(header.source_address, 0);
    }

    #[test]
    fn test_decode_engine_speed() {
        let header = J1939Header {
            priority: 3,
            reserved: false,
            data_page: false,
            pdu_format: 0xF0,
            pdu_specific: 0x04,
            source_address: 0,
            pgn: 61444,
        };
        // RPM = 1600 → raw = 1600 / 0.125 = 12800 = 0x3200
        let mut data = vec![0x00u8; 8];
        data[3] = 0x00; // low byte of RPM raw
        data[4] = 0x32; // high byte: 0x3200 = 12800
        // Actually: bytes [3..5] = 0x0032 → that's 0x3200 LE = 12800
        data[3] = 0x00;
        data[4] = 0x32;
        let decoded = decode_j1939_data(&header, &data);
        assert_eq!(decoded.pgn, J1939Pgn::EngineSpeed);
        assert!(decoded.values.contains_key("engine_speed_rpm"));
    }

    #[test]
    fn test_decode_engine_temperature() {
        let header = J1939Header {
            priority: 6,
            reserved: false,
            data_page: false,
            pdu_format: 0xFE,
            pdu_specific: 0xEE,
            source_address: 0,
            pgn: 65262,
        };
        let mut data = vec![0u8; 8];
        data[0] = 120; // coolant temp = 120 - 40 = 80°C
        let decoded = decode_j1939_data(&header, &data);
        assert_eq!(decoded.pgn, J1939Pgn::EngineTemperature);
        assert_eq!(decoded.values["coolant_temp_c"], json!(80));
    }

    #[test]
    fn test_j1939_pgn_names() {
        assert_eq!(J1939Pgn::from_pgn(61444).name(), "Engine Speed");
        assert_eq!(J1939Pgn::from_pgn(65265).name(), "Vehicle Speed");
        assert_eq!(J1939Pgn::from_pgn(65262).name(), "Engine Temperature");
        assert_eq!(J1939Pgn::from_pgn(99999).name(), "Unknown PGN");
    }

    #[test]
    fn test_can_frame_to_source_event_j1939() {
        let frame = CanFrame {
            frame_type: CanFrameType::Extended,
            id: 0x0CF00400,
            dlc: 8,
            data: vec![0x00, 0x00, 0x00, 0x00, 0x32, 0xFF, 0xFF, 0x00],
            timestamp: Some(1700000000.0),
        };
        let event = can_frame_to_source_event(&frame, "can-test");
        assert_eq!(event.entity_type, "engine");
        assert!(event.entity_id.starts_with("canbus:j1939:"));
        assert!(event.properties.contains_key("pgn"));
        assert!(event.properties.contains_key("pgn_name"));
    }

    #[test]
    fn test_can_frame_to_source_event_standard() {
        let frame = CanFrame {
            frame_type: CanFrameType::Standard,
            id: 0x123,
            dlc: 3,
            data: vec![0xAA, 0xBB, 0xCC],
            timestamp: None,
        };
        let event = can_frame_to_source_event(&frame, "can-test");
        assert_eq!(event.entity_type, "vehicle");
        assert_eq!(event.entity_id, "canbus:123");
    }

    #[test]
    fn test_parse_can_frame_bytes() {
        let mut data = vec![0u8; 16];
        // ID = 0x123 (standard), with EFF flag = 0
        data[0..4].copy_from_slice(&0x00000123u32.to_le_bytes());
        data[4] = 3; // DLC
        data[8] = 0xAA;
        data[9] = 0xBB;
        data[10] = 0xCC;

        let frame = parse_can_frame_bytes(&data).unwrap();
        assert_eq!(frame.frame_type, CanFrameType::Standard);
        assert_eq!(frame.id, 0x123);
        assert_eq!(frame.data, vec![0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn test_can_bus_connector_id() {
        let config = ConnectorConfig {
            connector_id: "can-1".to_string(),
            connector_type: "canbus".to_string(),
            url: None,
            entity_type: "vehicle".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = CanBusConnector::new(config);
        assert_eq!(connector.connector_id(), "can-1");
    }

    #[tokio::test]
    async fn test_can_bus_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "can-h".to_string(),
            connector_type: "canbus".to_string(),
            url: None,
            entity_type: "vehicle".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = CanBusConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }
}
