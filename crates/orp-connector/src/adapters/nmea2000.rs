use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// NMEA 2000 (N2K) PGN decoder
// ---------------------------------------------------------------------------
// NMEA 2000 (IEC 61162-3) is the modern marine vessel network protocol,
// based on CAN bus (ISO 11898). It operates at 250 kbps using 29-bit
// extended CAN IDs.
//
// CAN ID structure (29 bits):
//   Priority (3 bits, 26-28) | Reserved (1 bit, 25) | Data Page (1 bit, 24) |
//   PDU Format (8 bits, 16-23) | PDU Specific (8 bits, 8-15) | Source Addr (8 bits, 0-7)
//
// PGN (Parameter Group Number) calculation:
//   - If PDU Format >= 240 (PDU2, broadcast): PGN = (DP << 16) | (PF << 8) | PS
//   - If PDU Format < 240 (PDU1, addressed): PGN = (DP << 16) | (PF << 8)
//
// Key PGNs decoded:
//   127250 — Vessel Heading
//   128259 — Speed (Water Referenced)
//   128267 — Water Depth
//   130306 — Wind Data
//   129025 — Position, Rapid Update
//   129038 — AIS Class A Position Report
//   129039 — AIS Class B Position Report

/// Well-known NMEA 2000 PGN identifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum N2kPgn {
    VesselHeading,       // 127250
    Speed,               // 128259
    WaterDepth,          // 128267
    WindData,            // 130306
    PositionRapid,       // 129025
    AisClassAPosition,   // 129038
    AisClassBPosition,   // 129039
    CogSogRapid,         // 129026
    GnssFix,             // 129029
    Rudder,              // 127245
    Unknown(u32),
}

impl N2kPgn {
    pub fn from_pgn(pgn: u32) -> Self {
        match pgn {
            127250 => N2kPgn::VesselHeading,
            128259 => N2kPgn::Speed,
            128267 => N2kPgn::WaterDepth,
            130306 => N2kPgn::WindData,
            129025 => N2kPgn::PositionRapid,
            129038 => N2kPgn::AisClassAPosition,
            129039 => N2kPgn::AisClassBPosition,
            129026 => N2kPgn::CogSogRapid,
            129029 => N2kPgn::GnssFix,
            127245 => N2kPgn::Rudder,
            _ => N2kPgn::Unknown(pgn),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            N2kPgn::VesselHeading => "Vessel Heading",
            N2kPgn::Speed => "Speed, Water Referenced",
            N2kPgn::WaterDepth => "Water Depth",
            N2kPgn::WindData => "Wind Data",
            N2kPgn::PositionRapid => "Position, Rapid Update",
            N2kPgn::AisClassAPosition => "AIS Class A Position Report",
            N2kPgn::AisClassBPosition => "AIS Class B Position Report",
            N2kPgn::CogSogRapid => "COG & SOG, Rapid Update",
            N2kPgn::GnssFix => "GNSS Position Data",
            N2kPgn::Rudder => "Rudder",
            N2kPgn::Unknown(_) => "Unknown PGN",
        }
    }
}

/// Parsed N2K CAN frame.
#[derive(Clone, Debug)]
pub struct N2kFrame {
    pub priority: u8,
    pub pgn: u32,
    pub source_address: u8,
    pub destination: Option<u8>,
    pub data: Vec<u8>,
}

/// Decoded N2K data from a specific PGN.
#[derive(Clone, Debug)]
pub struct N2kDecodedData {
    pub pgn: N2kPgn,
    pub pgn_number: u32,
    pub source_address: u8,
    pub values: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// CAN ID helpers
// ---------------------------------------------------------------------------

/// Extract (priority, pgn, source_address) from a 29-bit CAN ID.
pub fn extract_n2k_pgn(can_id: u32) -> (u8, u32, u8) {
    let priority = ((can_id >> 26) & 0x07) as u8;
    let dp = (can_id >> 24) & 0x01;
    let pf = ((can_id >> 16) & 0xFF) as u8;
    let ps = ((can_id >> 8) & 0xFF) as u8;
    let source_address = (can_id & 0xFF) as u8;

    let pgn = if pf >= 240 {
        (dp << 16) | ((pf as u32) << 8) | (ps as u32)
    } else {
        (dp << 16) | ((pf as u32) << 8)
    };

    (priority, pgn, source_address)
}

/// Build an N2kFrame from a CAN ID and data bytes.
pub fn parse_n2k_frame(can_id: u32, data: &[u8]) -> N2kFrame {
    let (priority, pgn, source_address) = extract_n2k_pgn(can_id);
    let pf = ((can_id >> 16) & 0xFF) as u8;
    let ps = ((can_id >> 8) & 0xFF) as u8;

    let destination = if pf < 240 { Some(ps) } else { None };

    N2kFrame {
        priority,
        pgn,
        source_address,
        destination,
        data: data.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// PGN decoders
// ---------------------------------------------------------------------------

/// Decode PGN 127250 — Vessel Heading.
/// Bytes: SID(1), Heading(2, 0.0001 rad unsigned), Deviation(2), Variation(2), Reference(1)
pub fn decode_pgn_127250(data: &[u8]) -> HashMap<String, serde_json::Value> {
    let mut values = HashMap::new();
    if data.is_empty() {
        return values;
    }

    values.insert("sid".into(), json!(data[0]));

    if data.len() >= 3 {
        let raw = u16::from_le_bytes([data[1], data[2]]);
        if raw != 0xFFFF {
            let heading_rad = raw as f64 * 0.0001;
            let heading_deg = heading_rad.to_degrees();
            values.insert("heading_rad".into(), json!(heading_rad));
            values.insert("heading_deg".into(), json!(heading_deg));
        }
    }

    if data.len() >= 5 {
        let raw = i16::from_le_bytes([data[3], data[4]]);
        if raw != i16::MAX {
            let deviation_rad = raw as f64 * 0.0001;
            values.insert("deviation_rad".into(), json!(deviation_rad));
        }
    }

    if data.len() >= 7 {
        let raw = i16::from_le_bytes([data[5], data[6]]);
        if raw != i16::MAX {
            let variation_rad = raw as f64 * 0.0001;
            values.insert("variation_rad".into(), json!(variation_rad));
        }
    }

    if data.len() >= 8 {
        let reference = data[7] & 0x03;
        let ref_name = match reference {
            0 => "True",
            1 => "Magnetic",
            _ => "Unknown",
        };
        values.insert("reference".into(), json!(ref_name));
    }

    values
}

/// Decode PGN 128259 — Speed (Water Referenced).
/// Bytes: SID(1), SpeedWaterRef(2, 0.01 m/s unsigned), SpeedGroundRef(2), SwrtType(4bits+4bits)
pub fn decode_pgn_128259(data: &[u8]) -> HashMap<String, serde_json::Value> {
    let mut values = HashMap::new();
    if data.is_empty() {
        return values;
    }

    values.insert("sid".into(), json!(data[0]));

    if data.len() >= 3 {
        let raw = u16::from_le_bytes([data[1], data[2]]);
        if raw != 0xFFFF {
            let speed_mps = raw as f64 * 0.01;
            let speed_knots = speed_mps * 1.94384;
            values.insert("speed_water_mps".into(), json!(speed_mps));
            values.insert("speed_water_knots".into(), json!(speed_knots));
        }
    }

    if data.len() >= 5 {
        let raw = u16::from_le_bytes([data[3], data[4]]);
        if raw != 0xFFFF {
            let speed_mps = raw as f64 * 0.01;
            values.insert("speed_ground_mps".into(), json!(speed_mps));
        }
    }

    values
}

/// Decode PGN 128267 — Water Depth.
/// Bytes: SID(1), Depth(4, 0.01m unsigned), Offset(2, 0.001m signed), MaxRange(4)
pub fn decode_pgn_128267(data: &[u8]) -> HashMap<String, serde_json::Value> {
    let mut values = HashMap::new();
    if data.is_empty() {
        return values;
    }

    values.insert("sid".into(), json!(data[0]));

    if data.len() >= 5 {
        let raw = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        if raw != 0xFFFFFFFF {
            let depth_m = raw as f64 * 0.01;
            let depth_ft = depth_m * 3.28084;
            values.insert("depth_m".into(), json!(depth_m));
            values.insert("depth_ft".into(), json!(depth_ft));
        }
    }

    if data.len() >= 7 {
        let raw = i16::from_le_bytes([data[5], data[6]]);
        if raw != i16::MAX {
            let offset_m = raw as f64 * 0.001;
            values.insert("transducer_offset_m".into(), json!(offset_m));
        }
    }

    values
}

/// Decode PGN 130306 — Wind Data.
/// Bytes: SID(1), WindSpeed(2, 0.01 m/s unsigned), WindAngle(2, 0.0001 rad unsigned), Reference(3 bits)
pub fn decode_pgn_130306(data: &[u8]) -> HashMap<String, serde_json::Value> {
    let mut values = HashMap::new();
    if data.is_empty() {
        return values;
    }

    values.insert("sid".into(), json!(data[0]));

    if data.len() >= 3 {
        let raw = u16::from_le_bytes([data[1], data[2]]);
        if raw != 0xFFFF {
            let speed_mps = raw as f64 * 0.01;
            let speed_knots = speed_mps * 1.94384;
            values.insert("wind_speed_mps".into(), json!(speed_mps));
            values.insert("wind_speed_knots".into(), json!(speed_knots));
        }
    }

    if data.len() >= 5 {
        let raw = u16::from_le_bytes([data[3], data[4]]);
        if raw != 0xFFFF {
            let angle_rad = raw as f64 * 0.0001;
            let angle_deg = angle_rad.to_degrees();
            values.insert("wind_angle_rad".into(), json!(angle_rad));
            values.insert("wind_angle_deg".into(), json!(angle_deg));
        }
    }

    if data.len() >= 6 {
        let reference = data[5] & 0x07;
        let ref_name = match reference {
            0 => "True (ground)",
            1 => "Magnetic (ground)",
            2 => "Apparent",
            3 => "True (boat)",
            _ => "Unknown",
        };
        values.insert("wind_reference".into(), json!(ref_name));
    }

    values
}

/// Decode PGN 129025 — Position, Rapid Update.
/// Bytes: Latitude(4, 1e-7 degrees signed), Longitude(4, 1e-7 degrees signed)
pub fn decode_pgn_129025(data: &[u8]) -> HashMap<String, serde_json::Value> {
    let mut values = HashMap::new();

    if data.len() >= 4 {
        let raw = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if raw != i32::MAX {
            let lat = raw as f64 * 1e-7;
            values.insert("latitude".into(), json!(lat));
        }
    }

    if data.len() >= 8 {
        let raw = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        if raw != i32::MAX {
            let lon = raw as f64 * 1e-7;
            values.insert("longitude".into(), json!(lon));
        }
    }

    values
}

/// Decode PGN 129038 — AIS Class A Position Report.
/// Bytes: MessageID+Repeat(1), MMSI(4 bytes LE), Longitude(4, 1e-7 deg signed),
///        Latitude(4, 1e-7 deg signed), ... (many more fields)
pub fn decode_pgn_129038(data: &[u8]) -> HashMap<String, serde_json::Value> {
    let mut values = HashMap::new();

    if data.is_empty() {
        return values;
    }

    let message_id = data[0] & 0x3F;
    let repeat_indicator = (data[0] >> 6) & 0x03;
    values.insert("message_id".into(), json!(message_id));
    values.insert("repeat_indicator".into(), json!(repeat_indicator));

    if data.len() >= 5 {
        let mmsi = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        values.insert("mmsi".into(), json!(mmsi));
    }

    if data.len() >= 9 {
        let raw = i32::from_le_bytes([data[5], data[6], data[7], data[8]]);
        if raw != i32::MAX {
            let lon = raw as f64 * 1e-7;
            values.insert("longitude".into(), json!(lon));
        }
    }

    if data.len() >= 13 {
        let raw = i32::from_le_bytes([data[9], data[10], data[11], data[12]]);
        if raw != i32::MAX {
            let lat = raw as f64 * 1e-7;
            values.insert("latitude".into(), json!(lat));
        }
    }

    // Additional fields if available
    if data.len() >= 15 {
        let raw = u16::from_le_bytes([data[13], data[14]]);
        if raw != 0xFFFF {
            let cog = raw as f64 * 0.0001; // radians
            values.insert("cog_rad".into(), json!(cog));
            values.insert("cog_deg".into(), json!(cog.to_degrees()));
        }
    }

    if data.len() >= 17 {
        let raw = u16::from_le_bytes([data[15], data[16]]);
        if raw != 0xFFFF {
            let sog = raw as f64 * 0.01; // m/s
            values.insert("sog_mps".into(), json!(sog));
            values.insert("sog_knots".into(), json!(sog * 1.94384));
        }
    }

    values
}

/// Decode PGN 129039 — AIS Class B Position Report.
/// Same initial structure as Class A.
pub fn decode_pgn_129039(data: &[u8]) -> HashMap<String, serde_json::Value> {
    // Class B has the same initial layout as Class A for the common fields
    let mut values = decode_pgn_129038(data);
    values.insert("ais_class".into(), json!("B"));
    values
}

/// Master decode dispatcher.
pub fn decode_n2k_data(frame: &N2kFrame) -> N2kDecodedData {
    let pgn_enum = N2kPgn::from_pgn(frame.pgn);
    let values = match pgn_enum {
        N2kPgn::VesselHeading => decode_pgn_127250(&frame.data),
        N2kPgn::Speed => decode_pgn_128259(&frame.data),
        N2kPgn::WaterDepth => decode_pgn_128267(&frame.data),
        N2kPgn::WindData => decode_pgn_130306(&frame.data),
        N2kPgn::PositionRapid => decode_pgn_129025(&frame.data),
        N2kPgn::AisClassAPosition => decode_pgn_129038(&frame.data),
        N2kPgn::AisClassBPosition => decode_pgn_129039(&frame.data),
        _ => {
            let mut v = HashMap::new();
            let hex: String = frame.data.iter().map(|b| format!("{:02X}", b)).collect();
            v.insert("raw_data".into(), json!(hex));
            v
        }
    };

    N2kDecodedData {
        pgn: pgn_enum,
        pgn_number: frame.pgn,
        source_address: frame.source_address,
        values,
    }
}

// ---------------------------------------------------------------------------
// N2K → SourceEvent
// ---------------------------------------------------------------------------

/// Convert decoded N2K data to a SourceEvent.
pub fn n2k_to_source_event(
    decoded: &N2kDecodedData,
    connector_id: &str,
) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert("pgn".into(), json!(decoded.pgn_number));
    properties.insert("pgn_name".into(), json!(decoded.pgn.name()));
    properties.insert("source_address".into(), json!(decoded.source_address));

    for (k, v) in &decoded.values {
        properties.insert(k.clone(), v.clone());
    }

    let (entity_type, entity_id) = match decoded.pgn {
        N2kPgn::AisClassAPosition | N2kPgn::AisClassBPosition => {
            let mmsi = decoded
                .values
                .get("mmsi")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            ("vessel".to_string(), format!("n2k:ais:{}", mmsi))
        }
        _ => (
            "vessel".to_string(),
            format!("n2k:src-{}", decoded.source_address),
        ),
    };

    let lat = decoded.values.get("latitude").and_then(|v| v.as_f64());
    let lon = decoded.values.get("longitude").and_then(|v| v.as_f64());

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type,
        properties,
        timestamp: Utc::now(),
        latitude: lat,
        longitude: lon,
    }
}

/// Parse an Actisense / candump-style log line.
///
/// Format: `timestamp,priority,pgn,source,destination,dlc,data_hex`
/// or: `(timestamp) n2k CAN_ID#DATA_HEX`
pub fn parse_n2k_log_line(line: &str) -> Option<N2kFrame> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // Try CSV format: timestamp,priority,pgn,source,dest,dlc,hex_data
    let parts: Vec<&str> = line.split(',').collect();
    if parts.len() >= 7 {
        let priority = parts[1].trim().parse::<u8>().ok()?;
        let pgn = parts[2].trim().parse::<u32>().ok()?;
        let source = parts[3].trim().parse::<u8>().ok()?;
        let dest_str = parts[4].trim();
        let destination = if dest_str == "255" || dest_str.is_empty() {
            None
        } else {
            dest_str.parse::<u8>().ok()
        };

        let hex_data = parts[6].trim();
        let data: Vec<u8> = (0..hex_data.len())
            .step_by(2)
            .filter_map(|i| {
                if i + 2 <= hex_data.len() {
                    u8::from_str_radix(&hex_data[i..i + 2], 16).ok()
                } else {
                    None
                }
            })
            .collect();

        return Some(N2kFrame {
            priority,
            pgn,
            source_address: source,
            destination,
            data,
        });
    }

    // Try candump format: (timestamp) interface CAN_ID#DATA
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() >= 3 {
        let frame_part = words[2];
        if let Some(hash_pos) = frame_part.find('#') {
            let id_str = &frame_part[..hash_pos];
            let data_str = &frame_part[hash_pos + 1..];

            let can_id = u32::from_str_radix(id_str, 16).ok()?;
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

            return Some(parse_n2k_frame(can_id, &data));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct Nmea2000Connector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl Nmea2000Connector {
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
impl Connector for Nmea2000Connector {
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
                ConnectorError::ConfigError("NMEA 2000: url (log file path) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let _errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        for line in content.lines() {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            if let Some(frame) = parse_n2k_log_line(line) {
                let decoded = decode_n2k_data(&frame);
                let event = n2k_to_source_event(&decoded, &connector_id);
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
                "NMEA 2000 connector is not running".into(),
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
    fn test_extract_pgn_heading() {
        // PGN 127250 = 0x1F112, DP=1, PF=0xF1 (241, >= 240 → broadcast), PS=0x12 (18)
        // CAN ID = priority(3) | R(1) | DP(1) | PF(8) | PS(8) | SA(8)
        // = (2 << 26) | (1 << 24) | (0xF1 << 16) | (0x12 << 8) | 3
        let can_id = (2u32 << 26) | (1u32 << 24) | (0xF1u32 << 16) | (0x12u32 << 8) | 3;
        let (priority, pgn, sa) = extract_n2k_pgn(can_id);
        assert_eq!(priority, 2);
        assert_eq!(pgn, 127250); // (1 << 16) | (0xF1 << 8) | 0x12 = 65536 + 61696 + 18
        assert_eq!(sa, 3);
    }

    #[test]
    fn test_extract_pgn_position() {
        // PGN 129025 = 0x1F801, DP=1, PF=0xF8 (248), PS=0x01
        let can_id = (2u32 << 26) | (1u32 << 24) | (0xF8u32 << 16) | (0x01u32 << 8) | 5;
        let (_, pgn, sa) = extract_n2k_pgn(can_id);
        assert_eq!(pgn, 129025); // (1 << 16) | (0xF8 << 8) | 0x01
        assert_eq!(sa, 5);
    }

    #[test]
    fn test_decode_heading() {
        // SID=0, heading = 1.5708 rad (90°) → raw = 1.5708 / 0.0001 = 15708
        let heading_raw = 15708u16;
        let data = vec![
            0x00, // SID
            heading_raw.to_le_bytes()[0],
            heading_raw.to_le_bytes()[1],
            0xFF, 0x7F, // deviation: max (not available)
            0xFF, 0x7F, // variation: max
            0x00, // reference: True
        ];
        let values = decode_pgn_127250(&data);
        assert!(values.contains_key("heading_rad"));
        let heading = values["heading_deg"].as_f64().unwrap();
        assert!((heading - 90.0).abs() < 0.1);
        assert_eq!(values["reference"], json!("True"));
    }

    #[test]
    fn test_decode_speed() {
        // Speed = 5.14 m/s (10 knots) → raw = 514
        let speed_raw = 514u16;
        let data = vec![
            0x00, // SID
            speed_raw.to_le_bytes()[0],
            speed_raw.to_le_bytes()[1],
            0xFF, 0xFF, // ground speed: not available
        ];
        let values = decode_pgn_128259(&data);
        let speed = values["speed_water_mps"].as_f64().unwrap();
        assert!((speed - 5.14).abs() < 0.01);
        assert!(values.contains_key("speed_water_knots"));
    }

    #[test]
    fn test_decode_depth() {
        // Depth = 25.5 m → raw = 2550
        let depth_raw = 2550u32;
        let data = vec![
            0x00, // SID
            depth_raw.to_le_bytes()[0],
            depth_raw.to_le_bytes()[1],
            depth_raw.to_le_bytes()[2],
            depth_raw.to_le_bytes()[3],
            0x00, 0x00, // offset
        ];
        let values = decode_pgn_128267(&data);
        let depth = values["depth_m"].as_f64().unwrap();
        assert!((depth - 25.5).abs() < 0.01);
        assert!(values.contains_key("depth_ft"));
    }

    #[test]
    fn test_decode_wind() {
        // Wind speed = 10.28 m/s (20 knots) → raw = 1028
        // Wind angle = 0.7854 rad (45°) → raw = 7854
        let speed_raw = 1028u16;
        let angle_raw = 7854u16;
        let data = vec![
            0x00, // SID
            speed_raw.to_le_bytes()[0],
            speed_raw.to_le_bytes()[1],
            angle_raw.to_le_bytes()[0],
            angle_raw.to_le_bytes()[1],
            0x02, // reference: Apparent
        ];
        let values = decode_pgn_130306(&data);
        let speed = values["wind_speed_mps"].as_f64().unwrap();
        assert!((speed - 10.28).abs() < 0.01);
        let angle = values["wind_angle_deg"].as_f64().unwrap();
        assert!((angle - 45.0).abs() < 0.1);
        assert_eq!(values["wind_reference"], json!("Apparent"));
    }

    #[test]
    fn test_decode_position() {
        // Lat = 51.5074° → raw = 515074000
        // Lon = -0.1278° → raw = -1278000
        let lat_raw = 515074000i32;
        let lon_raw = -1278000i32;
        let mut data = vec![0u8; 8];
        data[0..4].copy_from_slice(&lat_raw.to_le_bytes());
        data[4..8].copy_from_slice(&lon_raw.to_le_bytes());

        let values = decode_pgn_129025(&data);
        let lat = values["latitude"].as_f64().unwrap();
        let lon = values["longitude"].as_f64().unwrap();
        assert!((lat - 51.5074).abs() < 0.0001);
        assert!((lon - (-0.1278)).abs() < 0.0001);
    }

    #[test]
    fn test_decode_ais_class_a() {
        let mmsi = 244820000u32;
        let lon_raw = 41234567i32; // ~4.1235°
        let lat_raw = 519876543i32; // ~51.9877°
        let mut data = vec![0u8; 17];
        data[0] = 0x01; // message_id=1, repeat=0
        data[1..5].copy_from_slice(&mmsi.to_le_bytes());
        data[5..9].copy_from_slice(&lon_raw.to_le_bytes());
        data[9..13].copy_from_slice(&lat_raw.to_le_bytes());
        // COG raw (unused for now)
        data[13..15].copy_from_slice(&0xFFFFu16.to_le_bytes());
        // SOG raw
        data[15..17].copy_from_slice(&514u16.to_le_bytes()); // 5.14 m/s

        let values = decode_pgn_129038(&data);
        assert_eq!(values["mmsi"], json!(244820000));
        assert!(values.contains_key("latitude"));
        assert!(values.contains_key("longitude"));
        assert_eq!(values["message_id"], json!(1));
    }

    #[test]
    fn test_decode_ais_class_b() {
        let mmsi = 123456789u32;
        let mut data = vec![0u8; 13];
        data[0] = 0x12; // message_id=18, repeat=0
        data[1..5].copy_from_slice(&mmsi.to_le_bytes());
        data[5..9].copy_from_slice(&0i32.to_le_bytes());
        data[9..13].copy_from_slice(&0i32.to_le_bytes());

        let values = decode_pgn_129039(&data);
        assert_eq!(values["mmsi"], json!(123456789));
        assert_eq!(values["ais_class"], json!("B"));
    }

    #[test]
    fn test_n2k_to_source_event_vessel() {
        let frame = N2kFrame {
            priority: 2,
            pgn: 127250,
            source_address: 3,
            destination: None,
            data: vec![0x00, 0x00, 0x00, 0xFF, 0x7F, 0xFF, 0x7F, 0x00],
        };
        let decoded = decode_n2k_data(&frame);
        let event = n2k_to_source_event(&decoded, "n2k-test");
        assert_eq!(event.entity_type, "vessel");
        assert_eq!(event.entity_id, "n2k:src-3");
        assert_eq!(event.properties["pgn_name"], json!("Vessel Heading"));
    }

    #[test]
    fn test_n2k_to_source_event_ais() {
        let mmsi = 244820000u32;
        let mut data = vec![0u8; 13];
        data[0] = 0x01;
        data[1..5].copy_from_slice(&mmsi.to_le_bytes());
        data[5..9].copy_from_slice(&0i32.to_le_bytes());
        data[9..13].copy_from_slice(&0i32.to_le_bytes());

        let frame = N2kFrame {
            priority: 4,
            pgn: 129038,
            source_address: 1,
            destination: None,
            data,
        };
        let decoded = decode_n2k_data(&frame);
        let event = n2k_to_source_event(&decoded, "n2k-test");
        assert_eq!(event.entity_type, "vessel");
        assert_eq!(event.entity_id, "n2k:ais:244820000");
    }

    #[test]
    fn test_parse_n2k_log_line_csv() {
        let line = "1700000000.123,2,127250,3,255,8,001E9300FF7FFF00";
        let frame = parse_n2k_log_line(line).unwrap();
        assert_eq!(frame.priority, 2);
        assert_eq!(frame.pgn, 127250);
        assert_eq!(frame.source_address, 3);
        assert_eq!(frame.data.len(), 8);
    }

    #[test]
    fn test_n2k_connector_id() {
        let config = ConnectorConfig {
            connector_id: "n2k-1".to_string(),
            connector_type: "nmea2000".to_string(),
            url: None,
            entity_type: "vessel".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = Nmea2000Connector::new(config);
        assert_eq!(connector.connector_id(), "n2k-1");
    }

    #[tokio::test]
    async fn test_n2k_health_check() {
        let config = ConnectorConfig {
            connector_id: "n2k-h".to_string(),
            connector_type: "nmea2000".to_string(),
            url: None,
            entity_type: "vessel".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = Nmea2000Connector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_n2k_pgn_names() {
        assert_eq!(N2kPgn::from_pgn(127250).name(), "Vessel Heading");
        assert_eq!(N2kPgn::from_pgn(128259).name(), "Speed, Water Referenced");
        assert_eq!(N2kPgn::from_pgn(128267).name(), "Water Depth");
        assert_eq!(N2kPgn::from_pgn(130306).name(), "Wind Data");
        assert_eq!(N2kPgn::from_pgn(129025).name(), "Position, Rapid Update");
        assert_eq!(N2kPgn::from_pgn(99999).name(), "Unknown PGN");
    }
}
