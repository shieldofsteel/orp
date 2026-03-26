use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// DNP3 (Distributed Network Protocol 3) parser
// ---------------------------------------------------------------------------
// DNP3 (IEEE 1815) is the standard SCADA protocol for electric utilities,
// water/wastewater, and oil & gas.
//
// Protocol layers:
//   - Data Link Layer: frames with CRC-16 every 16 bytes
//   - Transport Layer: segmentation/reassembly
//   - Application Layer: function codes + data objects
//
// Frame format (Data Link):
//   0x0564  : start bytes
//   length  : u8 (5–255, excluding CRC)
//   control : u8 (direction, PRM, FCB, FCV, function)
//   dest    : u16 (destination address)
//   source  : u16 (source address)
//   CRC     : u16 (over header)
//   [data blocks + CRC every 16 bytes]
//
// Application Layer:
//   control : u8 (FIR, FIN, CON, UNS, sequence)
//   function: u8 (request/response code)
//   objects : data objects with group/variation/qualifier
//
// Key data objects:
//   Group 1: Binary Inputs (var 1=packed, var 2=with flags)
//   Group 2: Binary Input Events
//   Group 10: Binary Outputs
//   Group 12: Binary Output Commands (CROB)
//   Group 20: Counters
//   Group 22: Counter Events
//   Group 30: Analog Inputs (var 1=32-bit, var 2=16-bit, var 5=float)
//   Group 32: Analog Input Events
//   Group 40: Analog Outputs
//   Group 50: Time and Date

/// DNP3 function codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dnp3Function {
    Confirm,
    Read,
    Write,
    Select,
    Operate,
    DirectOperate,
    DirectOperateNoAck,
    ImmediateFreeze,
    ImmediateFreezeNoAck,
    FreezeAndClear,
    FreezeAndClearNoAck,
    FreezeAtTime,
    FreezeAtTimeNoAck,
    ColdRestart,
    WarmRestart,
    InitializeDataToDefaults,
    InitializeApplication,
    StartApplication,
    StopApplication,
    EnableUnsolicited,
    DisableUnsolicited,
    AssignClass,
    DelayMeasurement,
    Response,
    UnsolicitedResponse,
    Unknown(u8),
}

impl Dnp3Function {
    pub fn from_code(code: u8) -> Self {
        match code {
            0x00 => Dnp3Function::Confirm,
            0x01 => Dnp3Function::Read,
            0x02 => Dnp3Function::Write,
            0x03 => Dnp3Function::Select,
            0x04 => Dnp3Function::Operate,
            0x05 => Dnp3Function::DirectOperate,
            0x06 => Dnp3Function::DirectOperateNoAck,
            0x07 => Dnp3Function::ImmediateFreeze,
            0x08 => Dnp3Function::ImmediateFreezeNoAck,
            0x09 => Dnp3Function::FreezeAndClear,
            0x0A => Dnp3Function::FreezeAndClearNoAck,
            0x0B => Dnp3Function::FreezeAtTime,
            0x0C => Dnp3Function::FreezeAtTimeNoAck,
            0x0D => Dnp3Function::ColdRestart,
            0x0E => Dnp3Function::WarmRestart,
            0x0F => Dnp3Function::InitializeDataToDefaults,
            0x10 => Dnp3Function::InitializeApplication,
            0x11 => Dnp3Function::StartApplication,
            0x12 => Dnp3Function::StopApplication,
            0x14 => Dnp3Function::EnableUnsolicited,
            0x15 => Dnp3Function::DisableUnsolicited,
            0x16 => Dnp3Function::AssignClass,
            0x17 => Dnp3Function::DelayMeasurement,
            0x81 => Dnp3Function::Response,
            0x82 => Dnp3Function::UnsolicitedResponse,
            _ => Dnp3Function::Unknown(code),
        }
    }

    pub fn code(&self) -> u8 {
        match self {
            Dnp3Function::Confirm => 0x00,
            Dnp3Function::Read => 0x01,
            Dnp3Function::Write => 0x02,
            Dnp3Function::Select => 0x03,
            Dnp3Function::Operate => 0x04,
            Dnp3Function::DirectOperate => 0x05,
            Dnp3Function::DirectOperateNoAck => 0x06,
            Dnp3Function::ImmediateFreeze => 0x07,
            Dnp3Function::ImmediateFreezeNoAck => 0x08,
            Dnp3Function::FreezeAndClear => 0x09,
            Dnp3Function::FreezeAndClearNoAck => 0x0A,
            Dnp3Function::FreezeAtTime => 0x0B,
            Dnp3Function::FreezeAtTimeNoAck => 0x0C,
            Dnp3Function::ColdRestart => 0x0D,
            Dnp3Function::WarmRestart => 0x0E,
            Dnp3Function::InitializeDataToDefaults => 0x0F,
            Dnp3Function::InitializeApplication => 0x10,
            Dnp3Function::StartApplication => 0x11,
            Dnp3Function::StopApplication => 0x12,
            Dnp3Function::EnableUnsolicited => 0x14,
            Dnp3Function::DisableUnsolicited => 0x15,
            Dnp3Function::AssignClass => 0x16,
            Dnp3Function::DelayMeasurement => 0x17,
            Dnp3Function::Response => 0x81,
            Dnp3Function::UnsolicitedResponse => 0x82,
            Dnp3Function::Unknown(c) => *c,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Dnp3Function::Confirm => "Confirm",
            Dnp3Function::Read => "Read",
            Dnp3Function::Write => "Write",
            Dnp3Function::Select => "Select",
            Dnp3Function::Operate => "Operate",
            Dnp3Function::DirectOperate => "DirectOperate",
            Dnp3Function::DirectOperateNoAck => "DirectOperateNoAck",
            Dnp3Function::ImmediateFreeze => "ImmediateFreeze",
            Dnp3Function::ImmediateFreezeNoAck => "ImmediateFreezeNoAck",
            Dnp3Function::FreezeAndClear => "FreezeAndClear",
            Dnp3Function::FreezeAndClearNoAck => "FreezeAndClearNoAck",
            Dnp3Function::FreezeAtTime => "FreezeAtTime",
            Dnp3Function::FreezeAtTimeNoAck => "FreezeAtTimeNoAck",
            Dnp3Function::ColdRestart => "ColdRestart",
            Dnp3Function::WarmRestart => "WarmRestart",
            Dnp3Function::InitializeDataToDefaults => "InitializeDataToDefaults",
            Dnp3Function::InitializeApplication => "InitializeApplication",
            Dnp3Function::StartApplication => "StartApplication",
            Dnp3Function::StopApplication => "StopApplication",
            Dnp3Function::EnableUnsolicited => "EnableUnsolicited",
            Dnp3Function::DisableUnsolicited => "DisableUnsolicited",
            Dnp3Function::AssignClass => "AssignClass",
            Dnp3Function::DelayMeasurement => "DelayMeasurement",
            Dnp3Function::Response => "Response",
            Dnp3Function::UnsolicitedResponse => "UnsolicitedResponse",
            Dnp3Function::Unknown(_) => "Unknown",
        }
    }
}

/// DNP3 data link layer header.
#[derive(Clone, Debug)]
pub struct Dnp3LinkHeader {
    pub length: u8,
    pub control: u8,
    pub destination: u16,
    pub source: u16,
}

impl Dnp3LinkHeader {
    /// Direction bit (1 = master→outstation, 0 = outstation→master).
    pub fn direction(&self) -> bool {
        self.control & 0x80 != 0
    }

    /// Primary station bit.
    pub fn primary(&self) -> bool {
        self.control & 0x40 != 0
    }
}

/// DNP3 application control byte.
#[derive(Clone, Debug)]
pub struct Dnp3AppControl {
    pub fir: bool,  // first fragment
    pub fin: bool,  // final fragment
    pub con: bool,  // confirmation required
    pub uns: bool,  // unsolicited
    pub seq: u8,    // sequence number (0–15)
}

/// DNP3 data object header.
#[derive(Clone, Debug)]
pub struct Dnp3ObjectHeader {
    pub group: u8,
    pub variation: u8,
    pub qualifier: u8,
}

impl Dnp3ObjectHeader {
    pub fn group_name(&self) -> &'static str {
        match self.group {
            1 => "Binary Input",
            2 => "Binary Input Event",
            10 => "Binary Output",
            12 => "Binary Output Command (CROB)",
            20 => "Counter",
            22 => "Counter Event",
            30 => "Analog Input",
            32 => "Analog Input Event",
            40 => "Analog Output",
            41 => "Analog Output Command",
            50 => "Time and Date",
            60 => "Class Data",
            80 => "Internal Indications",
            _ => "Unknown Group",
        }
    }
}

/// Parsed DNP3 data object value.
#[derive(Clone, Debug)]
pub enum Dnp3DataValue {
    BinaryInput { index: u16, value: bool, flags: u8 },
    BinaryOutput { index: u16, value: bool, flags: u8 },
    Counter { index: u16, value: u32, flags: u8 },
    AnalogInput { index: u16, value: f64, flags: u8 },
    AnalogOutput { index: u16, value: f64, flags: u8 },
    TimeAndDate { timestamp_ms: u64 },
    Raw { group: u8, variation: u8, data: Vec<u8> },
}

impl Dnp3DataValue {
    pub fn to_json(&self) -> JsonValue {
        match self {
            Dnp3DataValue::BinaryInput { index, value, flags } => json!({
                "type": "binary_input",
                "index": index,
                "value": value,
                "flags": flags,
                "online": flags & 0x01 != 0,
            }),
            Dnp3DataValue::BinaryOutput { index, value, flags } => json!({
                "type": "binary_output",
                "index": index,
                "value": value,
                "flags": flags,
            }),
            Dnp3DataValue::Counter { index, value, flags } => json!({
                "type": "counter",
                "index": index,
                "value": value,
                "flags": flags,
            }),
            Dnp3DataValue::AnalogInput { index, value, flags } => json!({
                "type": "analog_input",
                "index": index,
                "value": value,
                "flags": flags,
                "online": flags & 0x01 != 0,
            }),
            Dnp3DataValue::AnalogOutput { index, value, flags } => json!({
                "type": "analog_output",
                "index": index,
                "value": value,
                "flags": flags,
            }),
            Dnp3DataValue::TimeAndDate { timestamp_ms } => json!({
                "type": "time_and_date",
                "timestamp_ms": timestamp_ms,
            }),
            Dnp3DataValue::Raw { group, variation, data } => {
                let hex: String = data.iter().map(|b| format!("{:02X}", b)).collect();
                json!({
                    "type": "raw",
                    "group": group,
                    "variation": variation,
                    "data_hex": hex,
                })
            }
        }
    }
}

/// Parsed DNP3 frame (application layer).
#[derive(Clone, Debug)]
pub struct Dnp3Frame {
    pub link_header: Dnp3LinkHeader,
    pub app_control: Option<Dnp3AppControl>,
    pub function: Option<Dnp3Function>,
    pub objects: Vec<Dnp3DataValue>,
}

// ---------------------------------------------------------------------------
// CRC-16 (DNP3 uses CRC-16-DNP)
// ---------------------------------------------------------------------------

const DNP3_CRC_TABLE: [u16; 256] = {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u16;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA6BC;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

pub fn compute_dnp3_crc(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        let idx = ((crc ^ byte as u16) & 0xFF) as usize;
        crc = (crc >> 8) ^ DNP3_CRC_TABLE[idx];
    }
    !crc
}

/// Verify CRC of a DNP3 data block.
pub fn verify_dnp3_crc(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }
    let payload = &data[..data.len() - 2];
    let crc_bytes = &data[data.len() - 2..];
    let expected = u16::from_le_bytes([crc_bytes[0], crc_bytes[1]]);
    compute_dnp3_crc(payload) == expected
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse DNP3 data link header (10 bytes: 0x05 0x64 + length + control + dest + src + CRC).
pub fn parse_dnp3_link_header(data: &[u8]) -> Result<Dnp3LinkHeader, ConnectorError> {
    if data.len() < 10 {
        return Err(ConnectorError::ParseError(
            "DNP3: frame too short for link header (need 10 bytes)".into(),
        ));
    }
    // Check start bytes
    if data[0] != 0x05 || data[1] != 0x64 {
        return Err(ConnectorError::ParseError(format!(
            "DNP3: invalid start bytes 0x{:02X}{:02X} (expected 0x0564)",
            data[0], data[1]
        )));
    }

    let length = data[2];
    let control = data[3];
    let destination = u16::from_le_bytes([data[4], data[5]]);
    let source = u16::from_le_bytes([data[6], data[7]]);

    // Verify header CRC
    let header_crc = u16::from_le_bytes([data[8], data[9]]);
    let computed_crc = compute_dnp3_crc(&data[0..8]);
    if header_crc != computed_crc {
        return Err(ConnectorError::ParseError(format!(
            "DNP3: header CRC mismatch (got 0x{:04X}, expected 0x{:04X})",
            header_crc, computed_crc
        )));
    }

    Ok(Dnp3LinkHeader {
        length,
        control,
        destination,
        source,
    })
}

/// Parse DNP3 application control byte.
pub fn parse_app_control(byte: u8) -> Dnp3AppControl {
    Dnp3AppControl {
        fir: byte & 0x80 != 0,
        fin: byte & 0x40 != 0,
        con: byte & 0x20 != 0,
        uns: byte & 0x10 != 0,
        seq: byte & 0x0F,
    }
}

/// Parse application layer data from reassembled transport payload.
pub fn parse_dnp3_application(data: &[u8]) -> Result<(Dnp3AppControl, Dnp3Function, Vec<Dnp3DataValue>), ConnectorError> {
    if data.len() < 2 {
        return Err(ConnectorError::ParseError(
            "DNP3: application data too short".into(),
        ));
    }

    let app_control = parse_app_control(data[0]);
    let function = Dnp3Function::from_code(data[1]);
    let mut objects = Vec::new();

    // Parse object headers and data
    let mut offset = 2;
    while offset + 3 <= data.len() {
        let group = data[offset];
        let variation = data[offset + 1];
        let qualifier = data[offset + 2];
        offset += 3;

        // Parse based on qualifier code (simplified)
        let range_spec = qualifier & 0x0F;
        let count = match range_spec {
            // 0x00–0x05: various range specifiers
            0x07 => {
                // Single value count (1 byte)
                if offset < data.len() {
                    let c = data[offset] as usize;
                    offset += 1;
                    c
                } else {
                    0
                }
            }
            0x08 => {
                // Two-byte count
                if offset + 1 < data.len() {
                    let c = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
                    offset += 2;
                    c
                } else {
                    0
                }
            }
            _ => {
                // Unsupported qualifier, try to parse one object
                if offset < data.len() {
                    1
                } else {
                    0
                }
            }
        };

        for i in 0..count {
            let idx = i as u16;
            match (group, variation) {
                (1, 1) => {
                    // Binary Input packed
                    if offset < data.len() {
                        let val = data[offset] & 0x01 != 0;
                        objects.push(Dnp3DataValue::BinaryInput {
                            index: idx,
                            value: val,
                            flags: data[offset],
                        });
                        offset += 1;
                    }
                }
                (1, 2) => {
                    // Binary Input with flags
                    if offset < data.len() {
                        let flags = data[offset];
                        let val = flags & 0x80 != 0;
                        objects.push(Dnp3DataValue::BinaryInput {
                            index: idx,
                            value: val,
                            flags,
                        });
                        offset += 1;
                    }
                }
                (20, 1) => {
                    // Counter 32-bit with flags
                    if offset + 4 < data.len() {
                        let flags = data[offset];
                        let value = u32::from_le_bytes([
                            data[offset + 1],
                            data[offset + 2],
                            data[offset + 3],
                            data[offset + 4],
                        ]);
                        objects.push(Dnp3DataValue::Counter {
                            index: idx,
                            value,
                            flags,
                        });
                        offset += 5;
                    }
                }
                (30, 1) => {
                    // Analog Input 32-bit with flags
                    if offset + 4 < data.len() {
                        let flags = data[offset];
                        let value = i32::from_le_bytes([
                            data[offset + 1],
                            data[offset + 2],
                            data[offset + 3],
                            data[offset + 4],
                        ]);
                        objects.push(Dnp3DataValue::AnalogInput {
                            index: idx,
                            value: value as f64,
                            flags,
                        });
                        offset += 5;
                    }
                }
                (30, 2) => {
                    // Analog Input 16-bit with flags
                    if offset + 2 < data.len() {
                        let flags = data[offset];
                        let value = i16::from_le_bytes([data[offset + 1], data[offset + 2]]);
                        objects.push(Dnp3DataValue::AnalogInput {
                            index: idx,
                            value: value as f64,
                            flags,
                        });
                        offset += 3;
                    }
                }
                (30, 5) => {
                    // Analog Input float with flags
                    if offset + 4 < data.len() {
                        let flags = data[offset];
                        let value = f32::from_le_bytes([
                            data[offset + 1],
                            data[offset + 2],
                            data[offset + 3],
                            data[offset + 4],
                        ]);
                        objects.push(Dnp3DataValue::AnalogInput {
                            index: idx,
                            value: value as f64,
                            flags,
                        });
                        offset += 5;
                    }
                }
                (50, 1) => {
                    // Time and Date (6 bytes: 48-bit ms since epoch)
                    if offset + 6 <= data.len() {
                        let mut bytes = [0u8; 8];
                        bytes[0..6].copy_from_slice(&data[offset..offset + 6]);
                        let ts_ms = u64::from_le_bytes(bytes);
                        objects.push(Dnp3DataValue::TimeAndDate { timestamp_ms: ts_ms });
                        offset += 6;
                    }
                }
                _ => {
                    // Unknown group/variation — store raw
                    let remaining = data.len() - offset;
                    let take = remaining.min(8);
                    objects.push(Dnp3DataValue::Raw {
                        group,
                        variation,
                        data: data[offset..offset + take].to_vec(),
                    });
                    offset += take;
                    break; // Can't parse further without knowing layout
                }
            }
        }
    }

    Ok((app_control, function, objects))
}

// ---------------------------------------------------------------------------
// DNP3 → SourceEvent
// ---------------------------------------------------------------------------

/// Convert DNP3 data values from a frame into SourceEvents.
pub fn dnp3_frame_to_events(
    frame: &Dnp3Frame,
    connector_id: &str,
) -> Vec<SourceEvent> {
    let src = frame.link_header.source;
    let dst = frame.link_header.destination;
    let function_name = frame
        .function
        .as_ref()
        .map(|f| f.as_str())
        .unwrap_or("unknown");

    frame
        .objects
        .iter()
        .map(|obj| {
            let (entity_id, properties) = match obj {
                Dnp3DataValue::BinaryInput { index, value, flags } => {
                    let id = format!("dnp3:{}:bi:{}", src, index);
                    let mut props = HashMap::new();
                    props.insert("point_type".into(), json!("binary_input"));
                    props.insert("index".into(), json!(index));
                    props.insert("value".into(), json!(value));
                    props.insert("flags".into(), json!(flags));
                    props.insert("online".into(), json!(flags & 0x01 != 0));
                    (id, props)
                }
                Dnp3DataValue::BinaryOutput { index, value, flags } => {
                    let id = format!("dnp3:{}:bo:{}", src, index);
                    let mut props = HashMap::new();
                    props.insert("point_type".into(), json!("binary_output"));
                    props.insert("index".into(), json!(index));
                    props.insert("value".into(), json!(value));
                    props.insert("flags".into(), json!(flags));
                    (id, props)
                }
                Dnp3DataValue::Counter { index, value, flags } => {
                    let id = format!("dnp3:{}:counter:{}", src, index);
                    let mut props = HashMap::new();
                    props.insert("point_type".into(), json!("counter"));
                    props.insert("index".into(), json!(index));
                    props.insert("value".into(), json!(value));
                    props.insert("flags".into(), json!(flags));
                    (id, props)
                }
                Dnp3DataValue::AnalogInput { index, value, flags } => {
                    let id = format!("dnp3:{}:ai:{}", src, index);
                    let mut props = HashMap::new();
                    props.insert("point_type".into(), json!("analog_input"));
                    props.insert("index".into(), json!(index));
                    props.insert("value".into(), json!(value));
                    props.insert("flags".into(), json!(flags));
                    props.insert("online".into(), json!(flags & 0x01 != 0));
                    (id, props)
                }
                Dnp3DataValue::AnalogOutput { index, value, flags } => {
                    let id = format!("dnp3:{}:ao:{}", src, index);
                    let mut props = HashMap::new();
                    props.insert("point_type".into(), json!("analog_output"));
                    props.insert("index".into(), json!(index));
                    props.insert("value".into(), json!(value));
                    props.insert("flags".into(), json!(flags));
                    (id, props)
                }
                Dnp3DataValue::TimeAndDate { timestamp_ms } => {
                    let id = format!("dnp3:{}:time", src);
                    let mut props = HashMap::new();
                    props.insert("point_type".into(), json!("time_and_date"));
                    props.insert("timestamp_ms".into(), json!(timestamp_ms));
                    (id, props)
                }
                Dnp3DataValue::Raw { group, variation, data } => {
                    let id = format!("dnp3:{}:raw:g{}v{}", src, group, variation);
                    let hex: String = data.iter().map(|b| format!("{:02X}", b)).collect();
                    let mut props = HashMap::new();
                    props.insert("point_type".into(), json!("raw"));
                    props.insert("group".into(), json!(group));
                    props.insert("variation".into(), json!(variation));
                    props.insert("data_hex".into(), json!(hex));
                    (id, props)
                }
            };

            let mut all_props = properties;
            all_props.insert("source_address".into(), json!(src));
            all_props.insert("destination_address".into(), json!(dst));
            all_props.insert("function".into(), json!(function_name));

            SourceEvent {
                connector_id: connector_id.to_string(),
                entity_id,
                entity_type: "sensor".into(),
                properties: all_props,
                timestamp: Utc::now(),
                latitude: None,
                longitude: None,
            }
        })
        .collect()
}

/// Convenience: parse a DNP3 JSON representation (for log-file ingestion).
/// Expected format: {"source": 10, "destination": 1, "function": "Response",
///   "objects": [{"type": "analog_input", "index": 0, "value": 42.5, "flags": 1}, ...]}
pub fn parse_dnp3_json(data: &str) -> Result<Vec<Dnp3DataValue>, ConnectorError> {
    let value: JsonValue = serde_json::from_str(data).map_err(|e| {
        ConnectorError::ParseError(format!("DNP3: invalid JSON: {}", e))
    })?;

    let objects = value
        .get("objects")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ConnectorError::ParseError("DNP3 JSON: missing 'objects' array".into()))?;

    let mut result = Vec::new();
    for obj in objects {
        let obj_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let index = obj.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let flags = obj.get("flags").and_then(|v| v.as_u64()).unwrap_or(0) as u8;

        match obj_type {
            "binary_input" => {
                let value = obj.get("value").and_then(|v| v.as_bool()).unwrap_or(false);
                result.push(Dnp3DataValue::BinaryInput { index, value, flags });
            }
            "binary_output" => {
                let value = obj.get("value").and_then(|v| v.as_bool()).unwrap_or(false);
                result.push(Dnp3DataValue::BinaryOutput { index, value, flags });
            }
            "counter" => {
                let value = obj.get("value").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                result.push(Dnp3DataValue::Counter { index, value, flags });
            }
            "analog_input" => {
                let value = obj.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
                result.push(Dnp3DataValue::AnalogInput { index, value, flags });
            }
            "analog_output" => {
                let value = obj.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
                result.push(Dnp3DataValue::AnalogOutput { index, value, flags });
            }
            _ => {
                // Store as raw
                let data_hex = obj
                    .get("data_hex")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let bytes: Vec<u8> = (0..data_hex.len())
                    .step_by(2)
                    .filter_map(|i| {
                        if i + 2 <= data_hex.len() {
                            u8::from_str_radix(&data_hex[i..i + 2], 16).ok()
                        } else {
                            None
                        }
                    })
                    .collect();
                result.push(Dnp3DataValue::Raw {
                    group: obj.get("group").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
                    variation: obj
                        .get("variation")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u8,
                    data: bytes,
                });
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct Dnp3Connector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl Dnp3Connector {
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
impl Connector for Dnp3Connector {
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
                ConnectorError::ConfigError("DNP3: url (file path or endpoint) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        // Parse each line as a JSON DNP3 record
        for line in content.lines() {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match parse_dnp3_json(line) {
                Ok(data_values) => {
                    let value: JsonValue = serde_json::from_str(line).unwrap_or_default();
                    let src = value.get("source").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                    let dst = value
                        .get("destination")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u16;
                    let func_str = value
                        .get("function")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Response");

                    let frame = Dnp3Frame {
                        link_header: Dnp3LinkHeader {
                            length: 0,
                            control: 0,
                            destination: dst,
                            source: src,
                        },
                        app_control: None,
                        function: Some(Dnp3Function::from_code(match func_str {
                            "Read" => 0x01,
                            "Response" => 0x81,
                            "Write" => 0x02,
                            _ => 0x81,
                        })),
                        objects: data_values,
                    };

                    let events = dnp3_frame_to_events(&frame, &connector_id);
                    for event in events {
                        if tx.send(event).await.is_err() {
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                        events_processed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
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
                "DNP3 connector is not running".into(),
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
    fn test_dnp3_crc() {
        // Build a valid header and verify CRC
        let data = [0x05, 0x64, 0x05, 0xC0, 0x01, 0x00, 0x00, 0x00];
        let crc = compute_dnp3_crc(&data);
        let mut frame = data.to_vec();
        frame.extend_from_slice(&crc.to_le_bytes());
        assert!(verify_dnp3_crc(&frame));
    }

    #[test]
    fn test_dnp3_crc_invalid() {
        let frame = [0x05, 0x64, 0x05, 0xC0, 0x01, 0x00, 0x00, 0x00, 0xFF, 0xFF];
        assert!(!verify_dnp3_crc(&frame));
    }

    #[test]
    fn test_parse_link_header() {
        let mut data = [0x05u8, 0x64, 0x05, 0xC0, 0x01, 0x00, 0x0A, 0x00, 0x00, 0x00];
        let crc = compute_dnp3_crc(&data[0..8]);
        data[8] = crc as u8;
        data[9] = (crc >> 8) as u8;
        let header = parse_dnp3_link_header(&data).unwrap();
        assert_eq!(header.length, 5);
        assert_eq!(header.destination, 1);
        assert_eq!(header.source, 10);
        assert!(header.direction());
    }

    #[test]
    fn test_parse_link_header_invalid_start() {
        let data = [0x00u8, 0x00, 0x05, 0xC0, 0x01, 0x00, 0x0A, 0x00, 0x00, 0x00];
        assert!(parse_dnp3_link_header(&data).is_err());
    }

    #[test]
    fn test_function_codes() {
        assert_eq!(Dnp3Function::from_code(0x01), Dnp3Function::Read);
        assert_eq!(Dnp3Function::from_code(0x81), Dnp3Function::Response);
        assert_eq!(Dnp3Function::from_code(0x82), Dnp3Function::UnsolicitedResponse);
        assert_eq!(Dnp3Function::Read.as_str(), "Read");
        assert_eq!(Dnp3Function::Response.code(), 0x81);
    }

    #[test]
    fn test_app_control() {
        let ctrl = parse_app_control(0xC0); // FIR=1, FIN=1, CON=0, UNS=0, SEQ=0
        assert!(ctrl.fir);
        assert!(ctrl.fin);
        assert!(!ctrl.con);
        assert!(!ctrl.uns);
        assert_eq!(ctrl.seq, 0);
    }

    #[test]
    fn test_object_header_names() {
        let hdr = Dnp3ObjectHeader { group: 30, variation: 1, qualifier: 0 };
        assert_eq!(hdr.group_name(), "Analog Input");
        let hdr2 = Dnp3ObjectHeader { group: 1, variation: 2, qualifier: 0 };
        assert_eq!(hdr2.group_name(), "Binary Input");
    }

    #[test]
    fn test_parse_dnp3_json_analog() {
        let json = r#"{"source": 10, "destination": 1, "function": "Response",
            "objects": [
                {"type": "analog_input", "index": 0, "value": 42.5, "flags": 1},
                {"type": "analog_input", "index": 1, "value": -10.0, "flags": 1}
            ]}"#;
        let values = parse_dnp3_json(json).unwrap();
        assert_eq!(values.len(), 2);
        if let Dnp3DataValue::AnalogInput { index, value, flags } = &values[0] {
            assert_eq!(*index, 0);
            assert!((value - 42.5).abs() < 0.01);
            assert_eq!(*flags, 1);
        } else {
            panic!("Expected AnalogInput");
        }
    }

    #[test]
    fn test_parse_dnp3_json_binary() {
        let json = r#"{"source": 5, "destination": 1, "function": "Response",
            "objects": [
                {"type": "binary_input", "index": 0, "value": true, "flags": 1},
                {"type": "binary_input", "index": 1, "value": false, "flags": 0}
            ]}"#;
        let values = parse_dnp3_json(json).unwrap();
        assert_eq!(values.len(), 2);
        if let Dnp3DataValue::BinaryInput { value, .. } = &values[0] {
            assert!(*value);
        } else {
            panic!("Expected BinaryInput");
        }
    }

    #[test]
    fn test_parse_dnp3_json_counter() {
        let json = r#"{"source": 10, "destination": 1, "function": "Response",
            "objects": [{"type": "counter", "index": 0, "value": 123456, "flags": 1}]}"#;
        let values = parse_dnp3_json(json).unwrap();
        if let Dnp3DataValue::Counter { value, .. } = &values[0] {
            assert_eq!(*value, 123456);
        } else {
            panic!("Expected Counter");
        }
    }

    #[test]
    fn test_dnp3_frame_to_events() {
        let frame = Dnp3Frame {
            link_header: Dnp3LinkHeader {
                length: 5,
                control: 0xC0,
                destination: 1,
                source: 10,
            },
            app_control: Some(parse_app_control(0xC0)),
            function: Some(Dnp3Function::Response),
            objects: vec![
                Dnp3DataValue::AnalogInput { index: 0, value: 100.5, flags: 1 },
                Dnp3DataValue::BinaryInput { index: 0, value: true, flags: 1 },
            ],
        };
        let events = dnp3_frame_to_events(&frame, "dnp3-test");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_type, "sensor");
        assert_eq!(events[0].entity_id, "dnp3:10:ai:0");
        assert_eq!(events[0].properties["value"], json!(100.5));
        assert_eq!(events[1].entity_id, "dnp3:10:bi:0");
    }

    #[test]
    fn test_data_value_to_json() {
        let ai = Dnp3DataValue::AnalogInput { index: 5, value: 42.0, flags: 1 };
        let j = ai.to_json();
        assert_eq!(j["type"], json!("analog_input"));
        assert_eq!(j["index"], json!(5));
        assert_eq!(j["value"], json!(42.0));
        assert_eq!(j["online"], json!(true));
    }

    #[test]
    fn test_dnp3_connector_id() {
        let config = ConnectorConfig {
            connector_id: "dnp3-1".to_string(),
            connector_type: "dnp3".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = Dnp3Connector::new(config);
        assert_eq!(connector.connector_id(), "dnp3-1");
    }

    #[tokio::test]
    async fn test_dnp3_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "dnp3-h".to_string(),
            connector_type: "dnp3".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = Dnp3Connector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_parse_invalid_dnp3_json() {
        assert!(parse_dnp3_json("{invalid}").is_err());
    }
}
