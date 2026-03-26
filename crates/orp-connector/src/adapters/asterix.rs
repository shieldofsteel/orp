use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ASTERIX binary protocol parser
// ---------------------------------------------------------------------------
// ASTERIX (All Purpose Structured Eurocontrol Surveillance Information Exchange)
// is a binary protocol for radar/surveillance data exchange.
//
// Structure:
//   Data Block = CAT (1 byte) + LEN (2 bytes, big-endian) + Record(s)
//   Record     = FSPEC (variable length bitmap) + Data Fields
//
// Each category has a User Application Profile (UAP) that maps FSPEC bits
// to field definitions. We implement Cat 048 (monoradar target reports) and
// Cat 062 (system track data) as the most important categories.

/// Parsed ASTERIX data block.
#[derive(Clone, Debug)]
pub struct AsterixDataBlock {
    pub category: u8,
    pub records: Vec<AsterixRecord>,
}

/// A single ASTERIX record with decoded fields.
#[derive(Clone, Debug)]
pub struct AsterixRecord {
    pub category: u8,
    pub fields: HashMap<String, AsterixFieldValue>,
}

/// Field value variants.
#[derive(Clone, Debug, PartialEq)]
pub enum AsterixFieldValue {
    U8(u8),
    U16(u16),
    U32(u32),
    I16(i16),
    F64(f64),
    Text(String),
    Bytes(Vec<u8>),
}

impl AsterixFieldValue {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            AsterixFieldValue::U8(v) => serde_json::json!(*v),
            AsterixFieldValue::U16(v) => serde_json::json!(*v),
            AsterixFieldValue::U32(v) => serde_json::json!(*v),
            AsterixFieldValue::I16(v) => serde_json::json!(*v),
            AsterixFieldValue::F64(v) => serde_json::json!(*v),
            AsterixFieldValue::Text(v) => serde_json::json!(v),
            AsterixFieldValue::Bytes(v) => {
                serde_json::json!(v.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(""))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FSPEC parsing
// ---------------------------------------------------------------------------

/// Parse FSPEC (Field Specification) — variable-length bitmap.
/// Each octet's bit 0 (LSB) indicates whether another FSPEC octet follows.
/// Returns (fspec_bytes, bytes_consumed).
pub fn parse_fspec(data: &[u8]) -> Result<(Vec<u8>, usize), ConnectorError> {
    if data.is_empty() {
        return Err(ConnectorError::ParseError(
            "ASTERIX: empty FSPEC".to_string(),
        ));
    }
    let mut fspec = Vec::new();
    let mut idx = 0;
    loop {
        if idx >= data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX: truncated FSPEC".to_string(),
            ));
        }
        let byte = data[idx];
        fspec.push(byte);
        idx += 1;
        // If bit 0 (LSB) is 0, this is the last FSPEC byte
        if byte & 0x01 == 0 {
            break;
        }
    }
    Ok((fspec, idx))
}

/// Check if a specific field index is present in FSPEC.
/// Field indices are numbered 0-based across all FSPEC bytes, skipping the
/// FX (extension) bits. Each FSPEC byte contributes 7 data bits (bits 7..1).
pub fn fspec_has_field(fspec: &[u8], field_index: usize) -> bool {
    let byte_idx = field_index / 7;
    let bit_idx = field_index % 7;
    if byte_idx >= fspec.len() {
        return false;
    }
    // Bits are numbered 7 (MSB) down to 1, with bit 0 being FX
    let bit_pos = 7 - bit_idx;
    (fspec[byte_idx] >> bit_pos) & 1 == 1
}

// ---------------------------------------------------------------------------
// Category 048 — Monoradar Target Reports
// ---------------------------------------------------------------------------
// UAP for Cat 048 (most common fields):
//   FRN 1: I048/010 Data Source Identifier (2 bytes)
//   FRN 2: I048/140 Time of Day (3 bytes)
//   FRN 3: I048/020 Target Report Descriptor (variable)
//   FRN 4: I048/040 Measured Position in Polar (4 bytes)
//   FRN 5: I048/070 Mode-3/A Code (2 bytes)
//   FRN 6: I048/090 Flight Level (2 bytes)
//   FRN 7: I048/130 Radar Plot Characteristics (variable)
//   FRN 8: I048/220 Aircraft Address (ICAO 24-bit) (3 bytes)
//   FRN 9: I048/240 Aircraft Identification (6 bytes)
//   FRN 10: I048/250 Mode S MB Data (variable)
//   FRN 11: I048/161 Track Number (2 bytes)
//   FRN 12: I048/042 Calculated Position in Cartesian (4 bytes)
//   FRN 13: I048/200 Calculated Track Velocity (4 bytes)

pub fn parse_cat048_record(
    data: &[u8],
) -> Result<(AsterixRecord, usize), ConnectorError> {
    let (fspec, mut offset) = parse_fspec(data)?;
    let mut fields = HashMap::new();

    // FRN 1 — I048/010 Data Source Identifier (SAC/SIC, 2 bytes)
    if fspec_has_field(&fspec, 0) {
        if offset + 2 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/010".to_string(),
            ));
        }
        let sac = data[offset];
        let sic = data[offset + 1];
        fields.insert("sac".into(), AsterixFieldValue::U8(sac));
        fields.insert("sic".into(), AsterixFieldValue::U8(sic));
        offset += 2;
    }

    // FRN 2 — I048/140 Time of Day (3 bytes, 1/128 s resolution)
    if fspec_has_field(&fspec, 1) {
        if offset + 3 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/140".to_string(),
            ));
        }
        let raw = ((data[offset] as u32) << 16)
            | ((data[offset + 1] as u32) << 8)
            | (data[offset + 2] as u32);
        let seconds = raw as f64 / 128.0;
        fields.insert("time_of_day".into(), AsterixFieldValue::F64(seconds));
        offset += 3;
    }

    // FRN 3 — I048/020 Target Report Descriptor (variable, at least 1 byte)
    if fspec_has_field(&fspec, 2) {
        if offset >= data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/020".to_string(),
            ));
        }
        let first = data[offset];
        let typ = (first >> 5) & 0x07;
        fields.insert("target_report_type".into(), AsterixFieldValue::U8(typ));
        offset += 1;
        // Extension bytes
        if first & 0x01 != 0 {
            if offset >= data.len() {
                return Err(ConnectorError::ParseError(
                    "ASTERIX Cat048: truncated I048/020 ext".to_string(),
                ));
            }
            offset += 1; // skip extension byte
            // More extensions possible
            while offset > 0 && data[offset - 1] & 0x01 != 0 {
                if offset >= data.len() {
                    break;
                }
                offset += 1;
            }
        }
    }

    // FRN 4 — I048/040 Measured Position in Polar (4 bytes)
    if fspec_has_field(&fspec, 3) {
        if offset + 4 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/040".to_string(),
            ));
        }
        let rho_raw = ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
        let theta_raw =
            ((data[offset + 2] as u16) << 8) | (data[offset + 3] as u16);
        let rho_nm = rho_raw as f64 / 256.0; // NM
        let theta_deg = theta_raw as f64 * 360.0 / 65536.0; // degrees
        fields.insert("rho_nm".into(), AsterixFieldValue::F64(rho_nm));
        fields.insert("theta_deg".into(), AsterixFieldValue::F64(theta_deg));
        offset += 4;
    }

    // FRN 5 — I048/070 Mode-3/A Code (2 bytes)
    if fspec_has_field(&fspec, 4) {
        if offset + 2 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/070".to_string(),
            ));
        }
        let code_raw =
            ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
        let squawk = code_raw & 0x0FFF;
        fields.insert("mode3a".into(), AsterixFieldValue::U16(squawk));
        offset += 2;
    }

    // FRN 6 — I048/090 Flight Level (2 bytes, 1/4 FL)
    if fspec_has_field(&fspec, 5) {
        if offset + 2 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/090".to_string(),
            ));
        }
        let fl_raw = ((data[offset] as i16) << 8) | (data[offset + 1] as i16);
        let flight_level = (fl_raw & 0x3FFF) as f64 / 4.0;
        fields.insert(
            "flight_level".into(),
            AsterixFieldValue::F64(flight_level),
        );
        offset += 2;
    }

    // FRN 7 — I048/130 Radar Plot Characteristics (variable, compound)
    // Skip for now — variable length compound subfields
    if fspec_has_field(&fspec, 6) {
        if offset >= data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/130".to_string(),
            ));
        }
        // Primary subfield indicator
        let sub_indicator = data[offset];
        offset += 1;
        // Count present subfields (each is 1 byte)
        for bit in (1..=7).rev() {
            if sub_indicator & (1 << bit) != 0 {
                if offset >= data.len() {
                    return Err(ConnectorError::ParseError(
                        "ASTERIX Cat048: truncated I048/130 subfields".to_string(),
                    ));
                }
                offset += 1;
            }
        }
    }

    // FRN 8 — I048/220 Aircraft Address (3 bytes, ICAO 24-bit)
    if fspec_has_field(&fspec, 7) {
        if offset + 3 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/220".to_string(),
            ));
        }
        let addr = ((data[offset] as u32) << 16)
            | ((data[offset + 1] as u32) << 8)
            | (data[offset + 2] as u32);
        fields.insert(
            "icao_address".into(),
            AsterixFieldValue::Text(format!("{:06X}", addr)),
        );
        offset += 3;
    }

    // FRN 9 — I048/240 Aircraft Identification (6 bytes, ICAO 6-bit chars)
    if fspec_has_field(&fspec, 8) {
        if offset + 6 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/240".to_string(),
            ));
        }
        let callsign = decode_icao_6bit(&data[offset..offset + 6]);
        fields.insert(
            "callsign".into(),
            AsterixFieldValue::Text(callsign),
        );
        offset += 6;
    }

    // FRN 10 — I048/250 Mode S MB Data (variable, 1+N*8 bytes)
    if fspec_has_field(&fspec, 9) {
        if offset >= data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/250".to_string(),
            ));
        }
        let rep = data[offset] as usize;
        offset += 1;
        let mb_len = rep * 8;
        if offset + mb_len > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/250 data".to_string(),
            ));
        }
        offset += mb_len;
    }

    // FRN 11 — I048/161 Track Number (2 bytes)
    if fspec_has_field(&fspec, 10) {
        if offset + 2 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/161".to_string(),
            ));
        }
        let track =
            ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
        fields.insert("track_number".into(), AsterixFieldValue::U16(track & 0x0FFF));
        offset += 2;
    }

    // FRN 12 — I048/042 Calculated Position in Cartesian (4 bytes)
    if fspec_has_field(&fspec, 11) {
        if offset + 4 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/042".to_string(),
            ));
        }
        let x_raw = ((data[offset] as i16) << 8) | (data[offset + 1] as i16);
        let y_raw =
            ((data[offset + 2] as i16) << 8) | (data[offset + 3] as i16);
        let x_nm = x_raw as f64 / 128.0;
        let y_nm = y_raw as f64 / 128.0;
        fields.insert("x_nm".into(), AsterixFieldValue::F64(x_nm));
        fields.insert("y_nm".into(), AsterixFieldValue::F64(y_nm));
        offset += 4;
    }

    // FRN 13 — I048/200 Calculated Track Velocity (4 bytes)
    if fspec_has_field(&fspec, 12) {
        if offset + 4 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat048: truncated I048/200".to_string(),
            ));
        }
        let gs_raw =
            ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
        let hdg_raw =
            ((data[offset + 2] as u16) << 8) | (data[offset + 3] as u16);
        let ground_speed_kt = gs_raw as f64 * 3600.0 / 16384.0; // NM/s → knots
        let heading_deg = hdg_raw as f64 * 360.0 / 65536.0;
        fields.insert(
            "ground_speed_kt".into(),
            AsterixFieldValue::F64(ground_speed_kt),
        );
        fields.insert(
            "heading_deg".into(),
            AsterixFieldValue::F64(heading_deg),
        );
        offset += 4;
    }

    Ok((
        AsterixRecord {
            category: 48,
            fields,
        },
        offset,
    ))
}

// ---------------------------------------------------------------------------
// Category 062 — System Track Data
// ---------------------------------------------------------------------------
// UAP for Cat 062 (primary fields):
//   FRN 1: I062/010 Data Source Identifier (2 bytes)
//   FRN 2: I062/015 Service Identification (1 byte)
//   FRN 3: I062/070 Time of Track Information (3 bytes)
//   FRN 4: I062/105 Calculated Track Position (WGS-84) (8 bytes)
//   FRN 5: I062/100 Calculated Track Position (Cartesian) (6 bytes)
//   FRN 6: I062/185 Calculated Track Velocity (Cartesian) (4 bytes)
//   FRN 7: I062/210 Calculated Acceleration (Cartesian) (2 bytes)

pub fn parse_cat062_record(
    data: &[u8],
) -> Result<(AsterixRecord, usize), ConnectorError> {
    let (fspec, mut offset) = parse_fspec(data)?;
    let mut fields = HashMap::new();

    // FRN 1 — I062/010 Data Source Identifier (2 bytes)
    if fspec_has_field(&fspec, 0) {
        if offset + 2 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat062: truncated I062/010".to_string(),
            ));
        }
        fields.insert("sac".into(), AsterixFieldValue::U8(data[offset]));
        fields.insert("sic".into(), AsterixFieldValue::U8(data[offset + 1]));
        offset += 2;
    }

    // FRN 2 — I062/015 Service Identification (1 byte)
    if fspec_has_field(&fspec, 1) {
        if offset >= data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat062: truncated I062/015".to_string(),
            ));
        }
        fields.insert("service_id".into(), AsterixFieldValue::U8(data[offset]));
        offset += 1;
    }

    // FRN 3 — I062/070 Time of Track Information (3 bytes, 1/128 s)
    if fspec_has_field(&fspec, 2) {
        if offset + 3 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat062: truncated I062/070".to_string(),
            ));
        }
        let raw = ((data[offset] as u32) << 16)
            | ((data[offset + 1] as u32) << 8)
            | (data[offset + 2] as u32);
        let seconds = raw as f64 / 128.0;
        fields.insert("time_of_track".into(), AsterixFieldValue::F64(seconds));
        offset += 3;
    }

    // FRN 4 — I062/105 Calculated Track Position WGS-84 (8 bytes)
    if fspec_has_field(&fspec, 3) {
        if offset + 8 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat062: truncated I062/105".to_string(),
            ));
        }
        let lat_raw = i32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        let lon_raw = i32::from_be_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]);
        let lat = lat_raw as f64 * (180.0 / (1i64 << 25) as f64);
        let lon = lon_raw as f64 * (180.0 / (1i64 << 25) as f64);
        fields.insert("latitude".into(), AsterixFieldValue::F64(lat));
        fields.insert("longitude".into(), AsterixFieldValue::F64(lon));
        offset += 8;
    }

    // FRN 5 — I062/100 Calculated Track Position Cartesian (6 bytes)
    if fspec_has_field(&fspec, 4) {
        if offset + 6 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat062: truncated I062/100".to_string(),
            ));
        }
        // 3 bytes X, 3 bytes Y (signed, 0.5m resolution)
        let x_raw = i32::from_be_bytes([
            if data[offset] & 0x80 != 0 { 0xFF } else { 0x00 },
            data[offset],
            data[offset + 1],
            data[offset + 2],
        ]);
        let y_raw = i32::from_be_bytes([
            if data[offset + 3] & 0x80 != 0 { 0xFF } else { 0x00 },
            data[offset + 3],
            data[offset + 4],
            data[offset + 5],
        ]);
        let x_m = x_raw as f64 * 0.5;
        let y_m = y_raw as f64 * 0.5;
        fields.insert("cart_x_m".into(), AsterixFieldValue::F64(x_m));
        fields.insert("cart_y_m".into(), AsterixFieldValue::F64(y_m));
        offset += 6;
    }

    // FRN 6 — I062/185 Calculated Track Velocity Cartesian (4 bytes)
    if fspec_has_field(&fspec, 5) {
        if offset + 4 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat062: truncated I062/185".to_string(),
            ));
        }
        let vx_raw =
            i16::from_be_bytes([data[offset], data[offset + 1]]);
        let vy_raw =
            i16::from_be_bytes([data[offset + 2], data[offset + 3]]);
        let vx_ms = vx_raw as f64 * 0.25; // 0.25 m/s resolution
        let vy_ms = vy_raw as f64 * 0.25;
        fields.insert("vx_ms".into(), AsterixFieldValue::F64(vx_ms));
        fields.insert("vy_ms".into(), AsterixFieldValue::F64(vy_ms));
        offset += 4;
    }

    // FRN 7 — I062/210 Calculated Acceleration Cartesian (2 bytes)
    if fspec_has_field(&fspec, 6) {
        if offset + 2 > data.len() {
            return Err(ConnectorError::ParseError(
                "ASTERIX Cat062: truncated I062/210".to_string(),
            ));
        }
        let ax = data[offset] as i8;
        let ay = data[offset + 1] as i8;
        fields.insert(
            "ax_ms2".into(),
            AsterixFieldValue::F64(ax as f64 * 0.25),
        );
        fields.insert(
            "ay_ms2".into(),
            AsterixFieldValue::F64(ay as f64 * 0.25),
        );
        offset += 2;
    }

    Ok((
        AsterixRecord {
            category: 62,
            fields,
        },
        offset,
    ))
}

// ---------------------------------------------------------------------------
// Data Block parser
// ---------------------------------------------------------------------------

/// Parse an ASTERIX data block (CAT + LEN + records).
pub fn parse_asterix_data_block(
    data: &[u8],
) -> Result<AsterixDataBlock, ConnectorError> {
    if data.len() < 3 {
        return Err(ConnectorError::ParseError(
            "ASTERIX: data block too short (< 3 bytes)".to_string(),
        ));
    }

    let category = data[0];
    let length = ((data[1] as usize) << 8) | (data[2] as usize);

    if length > data.len() {
        return Err(ConnectorError::ParseError(format!(
            "ASTERIX: declared length {} exceeds available data {}",
            length,
            data.len()
        )));
    }

    let record_data = &data[3..length];
    let mut records = Vec::new();
    let mut offset = 0;

    while offset < record_data.len() {
        let (record, consumed) = match category {
            48 => parse_cat048_record(&record_data[offset..])?,
            62 => parse_cat062_record(&record_data[offset..])?,
            _ => {
                // Unknown category — store raw bytes as single record
                let remaining = record_data[offset..].to_vec();
                let mut fields = HashMap::new();
                fields.insert(
                    "raw_data".into(),
                    AsterixFieldValue::Bytes(remaining.clone()),
                );
                (
                    AsterixRecord {
                        category,
                        fields,
                    },
                    remaining.len(),
                )
            }
        };
        records.push(record);
        offset += consumed;
    }

    Ok(AsterixDataBlock { category, records })
}

// ---------------------------------------------------------------------------
// ICAO 6-bit character decoding
// ---------------------------------------------------------------------------

/// Decode ICAO 6-bit encoded characters from 6 bytes → 8 characters.
pub fn decode_icao_6bit(data: &[u8]) -> String {
    if data.len() < 6 {
        return String::new();
    }
    // 6 bytes = 48 bits → 8 characters at 6 bits each
    let bits: u64 = ((data[0] as u64) << 40)
        | ((data[1] as u64) << 32)
        | ((data[2] as u64) << 24)
        | ((data[3] as u64) << 16)
        | ((data[4] as u64) << 8)
        | (data[5] as u64);

    let charset = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ     0123456789      ";
    let mut result = String::with_capacity(8);
    for i in (0..8).rev() {
        let idx = ((bits >> (i * 6)) & 0x3F) as usize;
        let ch = if idx < charset.len() {
            charset[idx] as char
        } else {
            ' '
        };
        result.push(ch);
    }
    result.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Record → SourceEvent mapping
// ---------------------------------------------------------------------------

impl AsterixRecord {
    /// Convert to ORP SourceEvent.
    pub fn to_source_event(&self, connector_id: &str) -> SourceEvent {
        let mut properties: HashMap<String, serde_json::Value> = HashMap::new();

        for (k, v) in &self.fields {
            properties.insert(k.clone(), v.to_json());
        }

        properties.insert(
            "asterix_category".into(),
            serde_json::json!(self.category),
        );

        // Determine entity_id from available identifiers
        let entity_id = if let Some(AsterixFieldValue::Text(icao)) =
            self.fields.get("icao_address")
        {
            format!("icao:{}", icao)
        } else if let Some(AsterixFieldValue::U16(track)) =
            self.fields.get("track_number")
        {
            format!("asterix_track:{}", track)
        } else {
            format!("asterix:{}:{}", self.category, uuid::Uuid::new_v4())
        };

        // Extract position (lat/lon from Cat 062, or polar from Cat 048)
        let (lat, lon) = match (
            self.fields.get("latitude"),
            self.fields.get("longitude"),
        ) {
            (Some(AsterixFieldValue::F64(la)), Some(AsterixFieldValue::F64(lo))) => {
                (Some(*la), Some(*lo))
            }
            _ => (None, None),
        };

        // Auto‑detect entity type
        let entity_type = match self.category {
            48 => "radar_target",
            62 => "system_track",
            21 => "adsb_target",
            10 => "mlat_target",
            1 => "radar_plot",
            _ => "asterix_record",
        };

        SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id,
            entity_type: entity_type.to_string(),
            properties,
            timestamp: Utc::now(),
            latitude: lat,
            longitude: lon,
        }
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// ASTERIX connector — receives Eurocontrol ASTERIX binary data over UDP.
pub struct AsterixConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl AsterixConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl Connector for AsterixConnector {
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
            "ASTERIX connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let connector_id = self.config.connector_id.clone();
        let url = self.config.url.clone();

        tokio::spawn(async move {
            if let Some(ref url_str) = url {
                if let Some(addr) = url_str.strip_prefix("udp://") {
                    match tokio::net::UdpSocket::bind(addr).await {
                        Ok(socket) => {
                            tracing::info!("ASTERIX listening on UDP {}", addr);
                            let mut buf = vec![0u8; 65535];
                            while running.load(Ordering::SeqCst) {
                                match socket.recv_from(&mut buf).await {
                                    Ok((n, _)) => {
                                        match parse_asterix_data_block(&buf[..n]) {
                                            Ok(block) => {
                                                for rec in &block.records {
                                                    let event = rec.to_source_event(
                                                        &connector_id,
                                                    );
                                                    if tx.send(event).await.is_err() {
                                                        return;
                                                    }
                                                    events_count.fetch_add(
                                                        1,
                                                        Ordering::Relaxed,
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "ASTERIX parse error: {}",
                                                    e
                                                );
                                                errors_count
                                                    .fetch_add(1, Ordering::Relaxed);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("ASTERIX UDP error: {}", e);
                                        errors_count
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                            return;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Cannot bind ASTERIX UDP {}: {}",
                                addr,
                                e
                            );
                        }
                    }
                }
            }

            // Demo mode — not practical for binary protocol, just idle
            while running.load(Ordering::SeqCst) {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "ASTERIX connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "ASTERIX connector not running".to_string(),
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a Cat 048 data block with one record.
    // FSPEC = 0xFE 0x00 (FRN 1-7 present, no FRN 8+)
    fn build_cat048_block() -> Vec<u8> {
        let mut data: Vec<u8> = Vec::new();
        // FSPEC: FRN 1-7 present → byte 0 = 1111111_0 = 0xFE, no extension
        // Wait, FSPEC bit layout: bits 7..1 = FRN 1..7, bit 0 = FX
        // FRN 1-6 present, FRN 7 absent: 1111110_0 = 0xFC
        data.push(0xFC); // FSPEC: FRN1-6 present, no extension

        // FRN 1: I048/010 SAC=0x01, SIC=0x02
        data.push(0x01);
        data.push(0x02);

        // FRN 2: I048/140 Time of Day = 43200 seconds (12:00:00)
        // 43200 * 128 = 5529600 = 0x545A00 → wait, that won't fit in 3 bytes?
        // 43200 * 128 = 5529600 → 0x545A00 (3 bytes, fits in u24)
        let tod: u32 = 43200 * 128;
        data.push(((tod >> 16) & 0xFF) as u8);
        data.push(((tod >> 8) & 0xFF) as u8);
        data.push((tod & 0xFF) as u8);

        // FRN 3: I048/020 Target Report Descriptor, 1 byte (no ext)
        // Type = SSR (001) → bits 7-5 = 001, rest 0, FX=0
        data.push(0x20);

        // FRN 4: I048/040 Measured Position in Polar
        // Rho = 100 NM → 100 * 256 = 25600 = 0x6400
        data.push(0x64);
        data.push(0x00);
        // Theta = 90° → 90/360 * 65536 = 16384 = 0x4000
        data.push(0x40);
        data.push(0x00);

        // FRN 5: I048/070 Mode-3/A Code = squawk 1200 (octal) = 0x0280
        // Actually, raw value: 1200 in octal → decimal 640
        // But ASTERIX stores it as the raw squawk code in bits 11..0
        data.push(0x02);
        data.push(0x80);

        // FRN 6: I048/090 Flight Level = 350 (FL350 → 350*4 = 1400 = 0x0578)
        let fl: u16 = 350 * 4;
        data.push((fl >> 8) as u8);
        data.push((fl & 0xFF) as u8);

        // Wrap in data block: CAT=48, LEN = 3 + data.len()
        let total_len = 3 + data.len();
        let mut block = Vec::with_capacity(total_len);
        block.push(48); // CAT
        block.push((total_len >> 8) as u8);
        block.push((total_len & 0xFF) as u8);
        block.extend_from_slice(&data);
        block
    }

    #[test]
    fn test_fspec_single_byte() {
        let data = [0xFC]; // 11111100 — no extension
        let (fspec, consumed) = parse_fspec(&data).unwrap();
        assert_eq!(fspec.len(), 1);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_fspec_two_bytes() {
        let data = [0xFD, 0x80]; // first byte FX=1 → extension; second byte FX=0
        let (fspec, consumed) = parse_fspec(&data).unwrap();
        assert_eq!(fspec.len(), 2);
        assert_eq!(consumed, 2);
    }

    #[test]
    fn test_fspec_has_field() {
        let fspec = vec![0xFC]; // 11111100
        assert!(fspec_has_field(&fspec, 0)); // bit 7
        assert!(fspec_has_field(&fspec, 1)); // bit 6
        assert!(fspec_has_field(&fspec, 5)); // bit 2
        assert!(!fspec_has_field(&fspec, 6)); // bit 1 = 0 (this is the FX-adjacent)
    }

    #[test]
    fn test_fspec_empty_error() {
        assert!(parse_fspec(&[]).is_err());
    }

    #[test]
    fn test_parse_cat048_block() {
        let block = build_cat048_block();
        let result = parse_asterix_data_block(&block).unwrap();
        assert_eq!(result.category, 48);
        assert_eq!(result.records.len(), 1);

        let rec = &result.records[0];
        assert_eq!(rec.fields.get("sac"), Some(&AsterixFieldValue::U8(1)));
        assert_eq!(rec.fields.get("sic"), Some(&AsterixFieldValue::U8(2)));

        if let Some(AsterixFieldValue::F64(tod)) = rec.fields.get("time_of_day") {
            assert!((tod - 43200.0).abs() < 0.1);
        } else {
            panic!("time_of_day not found");
        }
    }

    #[test]
    fn test_cat048_rho_theta() {
        let block = build_cat048_block();
        let result = parse_asterix_data_block(&block).unwrap();
        let rec = &result.records[0];

        if let Some(AsterixFieldValue::F64(rho)) = rec.fields.get("rho_nm") {
            assert!((rho - 100.0).abs() < 0.1);
        } else {
            panic!("rho_nm not found");
        }

        if let Some(AsterixFieldValue::F64(theta)) = rec.fields.get("theta_deg") {
            assert!((theta - 90.0).abs() < 0.1);
        } else {
            panic!("theta_deg not found");
        }
    }

    #[test]
    fn test_cat048_flight_level() {
        let block = build_cat048_block();
        let result = parse_asterix_data_block(&block).unwrap();
        let rec = &result.records[0];

        if let Some(AsterixFieldValue::F64(fl)) = rec.fields.get("flight_level") {
            assert!((fl - 350.0).abs() < 0.1);
        } else {
            panic!("flight_level not found");
        }
    }

    #[test]
    fn test_cat048_mode3a() {
        let block = build_cat048_block();
        let result = parse_asterix_data_block(&block).unwrap();
        let rec = &result.records[0];

        if let Some(AsterixFieldValue::U16(code)) = rec.fields.get("mode3a") {
            assert_eq!(*code, 0x0280);
        } else {
            panic!("mode3a not found");
        }
    }

    #[test]
    fn test_cat062_wgs84_position() {
        // Build a Cat 062 block with WGS-84 position
        let mut record_data: Vec<u8> = Vec::new();
        // FSPEC: FRN 1,3,4 present → bits: 1_0_1_1_0_0_0_0 = 0xB0 (no FX)
        // Actually let's be precise:
        // FRN 1 = bit 7, FRN 2 = bit 6, FRN 3 = bit 5, FRN 4 = bit 4
        // We want FRN 1,3,4 → bit7=1 bit6=0 bit5=1 bit4=1 bit3-1=0 bit0(FX)=0
        // = 10110000 = 0xB0
        record_data.push(0xB0);

        // FRN 1: I062/010 SAC=10, SIC=20
        record_data.push(10);
        record_data.push(20);

        // FRN 3: I062/070 Time of Track (3 bytes)
        let tod: u32 = 36000 * 128; // 10:00:00
        record_data.push(((tod >> 16) & 0xFF) as u8);
        record_data.push(((tod >> 8) & 0xFF) as u8);
        record_data.push((tod & 0xFF) as u8);

        // FRN 4: I062/105 WGS-84 position (8 bytes)
        // lat = 52.0° → 52.0 / (180.0 / 2^25) = 52.0 * 2^25 / 180.0
        let lat_raw = (52.0 * (1i64 << 25) as f64 / 180.0) as i32;
        let lon_raw = (4.5 * (1i64 << 25) as f64 / 180.0) as i32;
        record_data.extend_from_slice(&lat_raw.to_be_bytes());
        record_data.extend_from_slice(&lon_raw.to_be_bytes());

        let total_len = 3 + record_data.len();
        let mut block = Vec::with_capacity(total_len);
        block.push(62); // CAT
        block.push((total_len >> 8) as u8);
        block.push((total_len & 0xFF) as u8);
        block.extend_from_slice(&record_data);

        let result = parse_asterix_data_block(&block).unwrap();
        assert_eq!(result.category, 62);
        assert_eq!(result.records.len(), 1);

        let rec = &result.records[0];
        if let Some(AsterixFieldValue::F64(lat)) = rec.fields.get("latitude") {
            assert!((lat - 52.0).abs() < 0.001);
        } else {
            panic!("latitude not found");
        }
        if let Some(AsterixFieldValue::F64(lon)) = rec.fields.get("longitude") {
            assert!((lon - 4.5).abs() < 0.001);
        } else {
            panic!("longitude not found");
        }
    }

    #[test]
    fn test_cat062_to_source_event() {
        let mut fields = HashMap::new();
        fields.insert("latitude".into(), AsterixFieldValue::F64(52.0));
        fields.insert("longitude".into(), AsterixFieldValue::F64(4.5));
        fields.insert("sac".into(), AsterixFieldValue::U8(10));

        let rec = AsterixRecord {
            category: 62,
            fields,
        };
        let event = rec.to_source_event("asterix-test");
        assert_eq!(event.entity_type, "system_track");
        assert_eq!(event.latitude, Some(52.0));
        assert_eq!(event.longitude, Some(4.5));
    }

    #[test]
    fn test_cat048_to_source_event_with_icao() {
        let mut fields = HashMap::new();
        fields.insert(
            "icao_address".into(),
            AsterixFieldValue::Text("A1B2C3".into()),
        );

        let rec = AsterixRecord {
            category: 48,
            fields,
        };
        let event = rec.to_source_event("asterix-test");
        assert_eq!(event.entity_id, "icao:A1B2C3");
        assert_eq!(event.entity_type, "radar_target");
    }

    #[test]
    fn test_decode_icao_6bit() {
        // "KLM1234 " encoded in 6-bit ICAO characters
        // K=11, L=12, M=13, 1=33, 2=34, 3=35, 4=36, space=0
        // Each char = 6 bits, 8 chars = 48 bits = 6 bytes
        let encoded: u64 = (11u64 << 42)
            | (12u64 << 36)
            | (13u64 << 30)
            | (33u64 << 24)
            | (34u64 << 18)
            | (35u64 << 12)
            | (36u64 << 6);
        let bytes = encoded.to_be_bytes();
        let result = decode_icao_6bit(&bytes[2..8]);
        assert_eq!(result, "KLM1234");
    }

    #[test]
    fn test_too_short_data_block() {
        assert!(parse_asterix_data_block(&[48]).is_err());
        assert!(parse_asterix_data_block(&[48, 0]).is_err());
    }

    #[test]
    fn test_length_exceeds_data() {
        assert!(parse_asterix_data_block(&[48, 0, 100]).is_err());
    }

    #[test]
    fn test_unknown_category() {
        // Cat 255 with minimal data
        let block = vec![255, 0, 5, 0xAA, 0xBB];
        let result = parse_asterix_data_block(&block).unwrap();
        assert_eq!(result.category, 255);
        assert_eq!(result.records.len(), 1);
        assert!(result.records[0].fields.contains_key("raw_data"));
    }

    #[test]
    fn test_asterix_connector_id() {
        let config = ConnectorConfig {
            connector_id: "asterix-1".to_string(),
            connector_type: "asterix".to_string(),
            url: None,
            entity_type: "radar_target".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = AsterixConnector::new(config);
        assert_eq!(connector.connector_id(), "asterix-1");
    }

    #[tokio::test]
    async fn test_asterix_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "asterix-health".to_string(),
            connector_type: "asterix".to_string(),
            url: None,
            entity_type: "radar_target".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = AsterixConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_cat048_with_icao_and_callsign() {
        // Build a record with FSPEC that includes FRN 1, 8 (ICAO), 9 (callsign)
        // FRN 1 = bit7, FRN 8 = bit7 of second byte, FRN 9 = bit6 of second byte
        // First byte: FRN 1 present, FX=1 → 10000001 = 0x81
        // Second byte: FRN 8,9 present, FX=0 → 11000000 = 0xC0
        let mut record_data: Vec<u8> = vec![0x81, 0xC0];

        // FRN 1: SAC=1, SIC=2
        record_data.push(0x01);
        record_data.push(0x02);

        // FRN 8: ICAO address = 0xABCDEF
        record_data.push(0xAB);
        record_data.push(0xCD);
        record_data.push(0xEF);

        // FRN 9: Aircraft ID (6 bytes) — encode "TEST    "
        // T=20, E=5, S=19, T=20, spaces
        let encoded: u64 = (20u64 << 42)
            | (5u64 << 36)
            | (19u64 << 30)
            | (20u64 << 24);
        let bytes = encoded.to_be_bytes();
        record_data.extend_from_slice(&bytes[2..8]);

        let total_len = 3 + record_data.len();
        let mut block = Vec::with_capacity(total_len);
        block.push(48);
        block.push((total_len >> 8) as u8);
        block.push((total_len & 0xFF) as u8);
        block.extend_from_slice(&record_data);

        let result = parse_asterix_data_block(&block).unwrap();
        let rec = &result.records[0];
        assert_eq!(
            rec.fields.get("icao_address"),
            Some(&AsterixFieldValue::Text("ABCDEF".into()))
        );
        assert!(rec.fields.contains_key("callsign"));
    }

    #[test]
    fn test_asterix_field_value_to_json() {
        assert_eq!(AsterixFieldValue::U8(42).to_json(), serde_json::json!(42));
        assert_eq!(
            AsterixFieldValue::Text("hello".into()).to_json(),
            serde_json::json!("hello")
        );
        assert_eq!(
            AsterixFieldValue::F64(3.125).to_json(),
            serde_json::json!(3.125)
        );
        assert_eq!(
            AsterixFieldValue::Bytes(vec![0xAB, 0xCD]).to_json(),
            serde_json::json!("ABCD")
        );
    }
}
