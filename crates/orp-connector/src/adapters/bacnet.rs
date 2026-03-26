use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// BACnet (Building Automation and Control Networks) parser
// ---------------------------------------------------------------------------
// BACnet (ASHRAE 135, ISO 16484-5) is the dominant protocol for building
// management systems: HVAC, fire safety, access control, lighting.
//
// BACnet/IP uses UDP port 47808 (0xBAC0) with:
//   - BVLC (BACnet Virtual Link Control) header
//   - NPDU (Network Protocol Data Unit)
//   - APDU (Application Protocol Data Unit)
//
// BVLC header (4 bytes):
//   type    : u8 (always 0x81 for BACnet/IP)
//   function: u8 (0x0A = Original-Unicast-NPDU, 0x0B = Original-Broadcast-NPDU, etc.)
//   length  : u16 (big-endian, total frame length including BVLC)
//
// NPDU (variable):
//   version : u8 (always 0x01)
//   control : u8 (bits: 7=has_DNET, 5=has_SNET, 3=expecting_reply, 2=priority_high,
//                       1=priority_low)
//
// APDU PDU types (upper 4 bits of first byte):
//   0x0 = BACnet-Confirmed-Request
//   0x1 = BACnet-Unconfirmed-Request
//   0x2 = BACnet-SimpleACK
//   0x3 = BACnet-ComplexACK
//   0x4 = BACnet-SegmentACK
//   0x5 = BACnet-Error
//   0x6 = BACnet-Reject
//   0x7 = BACnet-Abort
//
// Key service choices:
//   Confirmed: ReadProperty(12), WriteProperty(15), SubscribeCOV(5),
//              ConfirmedCOVNotification(1)
//   Unconfirmed: I-Am(0), Who-Is(8), UnconfirmedCOVNotification(2),
//                UnconfirmedPrivateTransfer(4)
//
// Object types (10-bit code in object identifier):
//   0 = analog-input, 1 = analog-output, 2 = analog-value,
//   3 = binary-input, 4 = binary-output, 5 = binary-value,
//   8 = device, 13 = multi-state-input, 14 = multi-state-output

/// BACnet/IP BVLC header.
#[derive(Clone, Debug)]
pub struct BacnetBvlcHeader {
    pub type_byte: u8,
    pub function: u8,
    pub length: u16,
}

impl BacnetBvlcHeader {
    pub fn function_name(&self) -> &'static str {
        match self.function {
            0x00 => "BVLC-Result",
            0x01 => "Write-Broadcast-Distribution-Table",
            0x02 => "Read-Broadcast-Distribution-Table",
            0x03 => "Read-Broadcast-Distribution-Table-Ack",
            0x04 => "Forwarded-NPDU",
            0x05 => "Register-Foreign-Device",
            0x06 => "Read-Foreign-Device-Table",
            0x07 => "Read-Foreign-Device-Table-Ack",
            0x08 => "Delete-Foreign-Device-Table-Entry",
            0x09 => "Distribute-Broadcast-To-Network",
            0x0A => "Original-Unicast-NPDU",
            0x0B => "Original-Broadcast-NPDU",
            _ => "Unknown",
        }
    }
}

/// BACnet NPDU header.
#[derive(Clone, Debug)]
pub struct BacnetNpdu {
    pub version: u8,
    pub control: u8,
    pub hop_count: Option<u8>,
}

impl BacnetNpdu {
    pub fn has_dnet(&self) -> bool {
        self.control & 0x20 != 0
    }

    pub fn has_snet(&self) -> bool {
        self.control & 0x08 != 0
    }

    pub fn expecting_reply(&self) -> bool {
        self.control & 0x04 != 0
    }

    pub fn is_network_message(&self) -> bool {
        self.control & 0x80 != 0
    }
}

/// BACnet APDU type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BacnetApduType {
    ConfirmedRequest,
    UnconfirmedRequest,
    SimpleAck,
    ComplexAck,
    SegmentAck,
    Error,
    Reject,
    Abort,
    Unknown(u8),
}

impl BacnetApduType {
    pub fn from_byte(b: u8) -> Self {
        match (b >> 4) & 0x0F {
            0x0 => BacnetApduType::ConfirmedRequest,
            0x1 => BacnetApduType::UnconfirmedRequest,
            0x2 => BacnetApduType::SimpleAck,
            0x3 => BacnetApduType::ComplexAck,
            0x4 => BacnetApduType::SegmentAck,
            0x5 => BacnetApduType::Error,
            0x6 => BacnetApduType::Reject,
            0x7 => BacnetApduType::Abort,
            x => BacnetApduType::Unknown(x),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            BacnetApduType::ConfirmedRequest => "ConfirmedRequest",
            BacnetApduType::UnconfirmedRequest => "UnconfirmedRequest",
            BacnetApduType::SimpleAck => "SimpleAck",
            BacnetApduType::ComplexAck => "ComplexAck",
            BacnetApduType::SegmentAck => "SegmentAck",
            BacnetApduType::Error => "Error",
            BacnetApduType::Reject => "Reject",
            BacnetApduType::Abort => "Abort",
            BacnetApduType::Unknown(_) => "Unknown",
        }
    }
}

/// BACnet service choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BacnetService {
    // Confirmed services
    ConfirmedCovNotification,
    ReadProperty,
    WriteProperty,
    SubscribeCov,
    // Unconfirmed services
    IAm,
    WhoIs,
    UnconfirmedCovNotification,
    UnconfirmedPrivateTransfer,
    Unknown(u8),
}

impl BacnetService {
    pub fn from_confirmed(code: u8) -> Self {
        match code {
            1 => BacnetService::ConfirmedCovNotification,
            5 => BacnetService::SubscribeCov,
            12 => BacnetService::ReadProperty,
            15 => BacnetService::WriteProperty,
            _ => BacnetService::Unknown(code),
        }
    }

    pub fn from_unconfirmed(code: u8) -> Self {
        match code {
            0 => BacnetService::IAm,
            2 => BacnetService::UnconfirmedCovNotification,
            4 => BacnetService::UnconfirmedPrivateTransfer,
            8 => BacnetService::WhoIs,
            _ => BacnetService::Unknown(code),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            BacnetService::ConfirmedCovNotification => "ConfirmedCOVNotification",
            BacnetService::ReadProperty => "ReadProperty",
            BacnetService::WriteProperty => "WriteProperty",
            BacnetService::SubscribeCov => "SubscribeCOV",
            BacnetService::IAm => "I-Am",
            BacnetService::WhoIs => "Who-Is",
            BacnetService::UnconfirmedCovNotification => "UnconfirmedCOVNotification",
            BacnetService::UnconfirmedPrivateTransfer => "UnconfirmedPrivateTransfer",
            BacnetService::Unknown(_) => "Unknown",
        }
    }
}

/// BACnet object identifier (4 bytes: 10-bit type + 22-bit instance).
#[derive(Clone, Debug)]
pub struct BacnetObjectId {
    pub object_type: u16,
    pub instance: u32,
}

impl BacnetObjectId {
    pub fn object_type_name(&self) -> &'static str {
        match self.object_type {
            0 => "analog-input",
            1 => "analog-output",
            2 => "analog-value",
            3 => "binary-input",
            4 => "binary-output",
            5 => "binary-value",
            8 => "device",
            13 => "multi-state-input",
            14 => "multi-state-output",
            15 => "multi-state-value",
            17 => "schedule",
            19 => "trend-log",
            _ => "unknown",
        }
    }
}

/// BACnet property value.
#[derive(Clone, Debug)]
pub enum BacnetPropertyValue {
    Real(f32),
    UnsignedInt(u32),
    SignedInt(i32),
    Boolean(bool),
    CharString(String),
    Enumerated(u32),
    Null,
}

impl BacnetPropertyValue {
    pub fn to_json(&self) -> JsonValue {
        match self {
            BacnetPropertyValue::Real(v) => json!(v),
            BacnetPropertyValue::UnsignedInt(v) => json!(v),
            BacnetPropertyValue::SignedInt(v) => json!(v),
            BacnetPropertyValue::Boolean(v) => json!(v),
            BacnetPropertyValue::CharString(v) => json!(v),
            BacnetPropertyValue::Enumerated(v) => json!(v),
            BacnetPropertyValue::Null => json!(null),
        }
    }
}

/// Parsed BACnet message.
#[derive(Clone, Debug)]
pub struct BacnetMessage {
    pub bvlc: BacnetBvlcHeader,
    pub npdu: BacnetNpdu,
    pub apdu_type: BacnetApduType,
    pub service: BacnetService,
    pub object_id: Option<BacnetObjectId>,
    pub property_id: Option<u32>,
    pub property_values: Vec<BacnetPropertyValue>,
    pub device_instance: Option<u32>,
}

impl BacnetMessage {
    pub fn property_name(id: u32) -> &'static str {
        match id {
            85 => "present-value",
            77 => "object-name",
            79 => "object-type",
            75 => "object-identifier",
            28 => "description",
            36 => "event-state",
            72 => "notification-class",
            103 => "reliability",
            111 => "status-flags",
            117 => "units",
            _ => "unknown-property",
        }
    }
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse BACnet/IP BVLC header (4 bytes).
pub fn parse_bacnet_bvlc(data: &[u8]) -> Result<BacnetBvlcHeader, ConnectorError> {
    if data.len() < 4 {
        return Err(ConnectorError::ParseError(
            "BACnet: BVLC header too short (need 4 bytes)".into(),
        ));
    }
    if data[0] != 0x81 {
        return Err(ConnectorError::ParseError(format!(
            "BACnet: invalid BVLC type byte 0x{:02X} (expected 0x81)",
            data[0]
        )));
    }

    let length = u16::from_be_bytes([data[2], data[3]]);

    Ok(BacnetBvlcHeader {
        type_byte: data[0],
        function: data[1],
        length,
    })
}

/// Parse 4-byte BACnet object identifier (10-bit type + 22-bit instance).
pub fn parse_bacnet_object_id(data: &[u8]) -> Option<BacnetObjectId> {
    if data.len() < 4 {
        return None;
    }
    let raw = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let object_type = ((raw >> 22) & 0x3FF) as u16;
    let instance = raw & 0x3FFFFF;
    Some(BacnetObjectId {
        object_type,
        instance,
    })
}

/// Parse a BACnet/IP packet from binary data.
pub fn parse_bacnet_packet(data: &[u8]) -> Result<BacnetMessage, ConnectorError> {
    let bvlc = parse_bacnet_bvlc(data)?;

    // NPDU starts after BVLC header (4 bytes), or after 10 bytes for Forwarded-NPDU
    let npdu_offset = if bvlc.function == 0x04 { 10 } else { 4 };
    if data.len() < npdu_offset + 2 {
        return Err(ConnectorError::ParseError(
            "BACnet: packet too short for NPDU".into(),
        ));
    }

    let npdu_version = data[npdu_offset];
    let npdu_control = data[npdu_offset + 1];
    let npdu = BacnetNpdu {
        version: npdu_version,
        control: npdu_control,
        hop_count: None,
    };

    // Skip NPDU routing info to find APDU
    let mut apdu_offset = npdu_offset + 2;
    if npdu.has_dnet() {
        // DNET(2) + DLEN(1) + DADR(DLEN) + hop_count(1)
        if data.len() > apdu_offset + 3 {
            let dlen = data[apdu_offset + 2] as usize;
            apdu_offset += 3 + dlen + 1; // DNET + DLEN + DADR + hop_count
        }
    }
    if npdu.has_snet() {
        // SNET(2) + SLEN(1) + SADR(SLEN)
        if data.len() > apdu_offset + 3 {
            let slen = data[apdu_offset + 2] as usize;
            apdu_offset += 3 + slen;
        }
    }

    if npdu.is_network_message() || data.len() <= apdu_offset {
        // Network layer message or no APDU
        return Ok(BacnetMessage {
            bvlc,
            npdu,
            apdu_type: BacnetApduType::Unknown(0),
            service: BacnetService::Unknown(0),
            object_id: None,
            property_id: None,
            property_values: Vec::new(),
            device_instance: None,
        });
    }

    let apdu_type = BacnetApduType::from_byte(data[apdu_offset]);
    let mut object_id = None;
    let mut property_id = None;
    let property_values = Vec::new();
    let mut device_instance = None;

    let service = match apdu_type {
        BacnetApduType::ConfirmedRequest => {
            // Confirmed: byte 0 = type|flags, byte 1 = max_segs|max_resp,
            //            byte 2 = invoke_id, byte 3 = service_choice
            if data.len() > apdu_offset + 3 {
                let sc = data[apdu_offset + 3];
                let svc = BacnetService::from_confirmed(sc);

                // Try to parse object identifier from tagged data
                let tag_offset = apdu_offset + 4;
                if svc == BacnetService::ReadProperty && data.len() > tag_offset + 5 {
                    // Context tag 0 = Object Identifier (4 bytes)
                    if data[tag_offset] == 0x0C && data.len() > tag_offset + 5 {
                        object_id = parse_bacnet_object_id(&data[tag_offset + 1..]);
                        // Context tag 1 = Property Identifier
                        let prop_offset = tag_offset + 5;
                        if data.len() > prop_offset + 1 {
                            let tag = data[prop_offset];
                            if tag & 0xF0 == 0x10 {
                                // Tag 1
                                let len = (tag & 0x07) as usize;
                                if len == 1 && data.len() > prop_offset + 1 {
                                    property_id = Some(data[prop_offset + 1] as u32);
                                }
                            }
                        }
                    }
                }
                svc
            } else {
                BacnetService::Unknown(0)
            }
        }
        BacnetApduType::UnconfirmedRequest => {
            // Unconfirmed: byte 0 = type, byte 1 = service_choice
            if data.len() > apdu_offset + 1 {
                let sc = data[apdu_offset + 1];
                let svc = BacnetService::from_unconfirmed(sc);

                // I-Am: parse device object identifier
                if svc == BacnetService::IAm && data.len() > apdu_offset + 6 {
                    // Tagged: context tag for object-id
                    let iam_offset = apdu_offset + 2;
                    if data.len() > iam_offset + 5 {
                        // Application tag 12 = Object Identifier
                        if data[iam_offset] == 0xC4 {
                            if let Some(oid) =
                                parse_bacnet_object_id(&data[iam_offset + 1..])
                            {
                                if oid.object_type == 8 {
                                    device_instance = Some(oid.instance);
                                }
                                object_id = Some(oid);
                            }
                        }
                    }
                }
                svc
            } else {
                BacnetService::Unknown(0)
            }
        }
        BacnetApduType::ComplexAck => {
            // Complex ACK: byte 0 = type, byte 1 = invoke_id, byte 2 = service_choice
            if data.len() > apdu_offset + 2 {
                BacnetService::from_confirmed(data[apdu_offset + 2])
            } else {
                BacnetService::Unknown(0)
            }
        }
        BacnetApduType::SimpleAck => {
            if data.len() > apdu_offset + 2 {
                BacnetService::from_confirmed(data[apdu_offset + 2])
            } else {
                BacnetService::Unknown(0)
            }
        }
        _ => BacnetService::Unknown(0),
    };

    Ok(BacnetMessage {
        bvlc,
        npdu,
        apdu_type,
        service,
        object_id,
        property_id,
        property_values,
        device_instance,
    })
}

/// Parse BACnet data from JSON log format.
///
/// Expected JSON: `{"device_instance": 1234, "object_type": "analog-input",
///   "object_instance": 1, "property": "present-value", "value": 72.5,
///   "service": "ReadProperty"}`
pub fn parse_bacnet_json(data: &str) -> Result<BacnetMessage, ConnectorError> {
    let value: JsonValue = serde_json::from_str(data)
        .map_err(|e| ConnectorError::ParseError(format!("BACnet JSON: {}", e)))?;

    let device_instance = value.get("device_instance").and_then(|v| v.as_u64()).map(|v| v as u32);

    let object_type_str = value
        .get("object_type")
        .and_then(|v| v.as_str())
        .unwrap_or("device");
    let object_type = match object_type_str {
        "analog-input" => 0,
        "analog-output" => 1,
        "analog-value" => 2,
        "binary-input" => 3,
        "binary-output" => 4,
        "binary-value" => 5,
        "device" => 8,
        "multi-state-input" => 13,
        "multi-state-output" => 14,
        _ => 0,
    };
    let object_instance = value
        .get("object_instance")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let property_str = value
        .get("property")
        .and_then(|v| v.as_str())
        .unwrap_or("present-value");
    let property_id = match property_str {
        "present-value" => 85,
        "object-name" => 77,
        "object-type" => 79,
        "description" => 28,
        "status-flags" => 111,
        "units" => 117,
        _ => 85,
    };

    let mut property_values = Vec::new();
    if let Some(val) = value.get("value") {
        if let Some(f) = val.as_f64() {
            property_values.push(BacnetPropertyValue::Real(f as f32));
        } else if let Some(b) = val.as_bool() {
            property_values.push(BacnetPropertyValue::Boolean(b));
        } else if let Some(s) = val.as_str() {
            property_values.push(BacnetPropertyValue::CharString(s.to_string()));
        } else if let Some(u) = val.as_u64() {
            property_values.push(BacnetPropertyValue::UnsignedInt(u as u32));
        }
    }

    let service_str = value
        .get("service")
        .and_then(|v| v.as_str())
        .unwrap_or("ReadProperty");
    let service = match service_str {
        "ReadProperty" => BacnetService::ReadProperty,
        "WriteProperty" => BacnetService::WriteProperty,
        "SubscribeCOV" => BacnetService::SubscribeCov,
        "COVNotification" | "ConfirmedCOVNotification" => {
            BacnetService::ConfirmedCovNotification
        }
        "UnconfirmedCOVNotification" => BacnetService::UnconfirmedCovNotification,
        "I-Am" => BacnetService::IAm,
        "Who-Is" => BacnetService::WhoIs,
        _ => BacnetService::Unknown(0),
    };

    Ok(BacnetMessage {
        bvlc: BacnetBvlcHeader {
            type_byte: 0x81,
            function: 0x0A,
            length: 0,
        },
        npdu: BacnetNpdu {
            version: 1,
            control: 0,
            hop_count: None,
        },
        apdu_type: BacnetApduType::ConfirmedRequest,
        service,
        object_id: Some(BacnetObjectId {
            object_type,
            instance: object_instance,
        }),
        property_id: Some(property_id),
        property_values,
        device_instance,
    })
}

// ---------------------------------------------------------------------------
// BACnet → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a BACnet message to a SourceEvent.
pub fn bacnet_to_source_event(
    msg: &BacnetMessage,
    connector_id: &str,
) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert("service".into(), json!(msg.service.as_str()));
    properties.insert("apdu_type".into(), json!(msg.apdu_type.as_str()));
    properties.insert("bvlc_function".into(), json!(msg.bvlc.function_name()));

    if let Some(ref oid) = msg.object_id {
        properties.insert("object_type".into(), json!(oid.object_type_name()));
        properties.insert("object_type_code".into(), json!(oid.object_type));
        properties.insert("object_instance".into(), json!(oid.instance));
    }

    if let Some(pid) = msg.property_id {
        properties.insert("property_id".into(), json!(pid));
        properties.insert("property_name".into(), json!(BacnetMessage::property_name(pid)));
    }

    if let Some(dev) = msg.device_instance {
        properties.insert("device_instance".into(), json!(dev));
    }

    for (i, pv) in msg.property_values.iter().enumerate() {
        let key = if i == 0 {
            "value".to_string()
        } else {
            format!("value_{}", i)
        };
        properties.insert(key, pv.to_json());
    }

    let device_id = msg
        .device_instance
        .or_else(|| msg.object_id.as_ref().map(|o| o.instance))
        .unwrap_or(0);
    let obj_suffix = msg
        .object_id
        .as_ref()
        .map(|o| format!(":{}-{}", o.object_type_name(), o.instance))
        .unwrap_or_default();
    let entity_id = format!("bacnet:device-{}{}", device_id, obj_suffix);

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "sensor".to_string(),
        properties,
        timestamp: Utc::now(),
        latitude: None,
        longitude: None,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct BacnetConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl BacnetConnector {
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
impl Connector for BacnetConnector {
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
                ConnectorError::ConfigError("BACnet: url (file path) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        for line in content.lines() {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match parse_bacnet_json(line) {
                Ok(msg) => {
                    let event = bacnet_to_source_event(&msg, &connector_id);
                    if tx.send(event).await.is_err() {
                        break;
                    }
                    events_processed.fetch_add(1, Ordering::Relaxed);
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
                "BACnet connector is not running".into(),
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
    fn test_parse_bvlc_header() {
        let data = [0x81, 0x0A, 0x00, 0x18];
        let bvlc = parse_bacnet_bvlc(&data).unwrap();
        assert_eq!(bvlc.type_byte, 0x81);
        assert_eq!(bvlc.function, 0x0A);
        assert_eq!(bvlc.length, 24);
        assert_eq!(bvlc.function_name(), "Original-Unicast-NPDU");
    }

    #[test]
    fn test_parse_bvlc_invalid() {
        let data = [0x00, 0x0A, 0x00, 0x18];
        assert!(parse_bacnet_bvlc(&data).is_err());
    }

    #[test]
    fn test_parse_bvlc_too_short() {
        let data = [0x81, 0x0A];
        assert!(parse_bacnet_bvlc(&data).is_err());
    }

    #[test]
    fn test_parse_bacnet_object_id() {
        // analog-input (0) instance 1: 0x00000001
        let data = [0x00, 0x00, 0x00, 0x01];
        let oid = parse_bacnet_object_id(&data).unwrap();
        assert_eq!(oid.object_type, 0);
        assert_eq!(oid.instance, 1);
        assert_eq!(oid.object_type_name(), "analog-input");

        // device (8) instance 1234: type=8 → (8 << 22) | 1234 = 0x020004D2
        let val: u32 = (8 << 22) | 1234;
        let bytes = val.to_be_bytes();
        let oid2 = parse_bacnet_object_id(&bytes).unwrap();
        assert_eq!(oid2.object_type, 8);
        assert_eq!(oid2.instance, 1234);
        assert_eq!(oid2.object_type_name(), "device");
    }

    #[test]
    fn test_parse_bacnet_packet_confirmed_read() {
        // Minimal BACnet/IP packet: BVLC + NPDU + Confirmed-Request ReadProperty
        let mut packet = vec![
            // BVLC
            0x81, 0x0A, 0x00, 0x11, // type=0x81, func=0x0A, length=17
            // NPDU
            0x01, 0x04, // version=1, control=0x04 (expecting reply)
            // APDU: Confirmed-Request
            0x00, // type=0 (confirmed), no segmentation
            0x05, // max_segs=0, max_resp=5
            0x01, // invoke_id=1
            0x0C, // service_choice=12 (ReadProperty)
            // Object ID context tag 0 (0x0C) + 4 bytes
            0x0C, 0x00, 0x00, 0x00, 0x01, // analog-input instance 1
            // Property ID context tag 1 (0x19) + 1 byte
            0x19, 0x55, // property 85 (present-value)
        ];
        // Fix length
        packet[2] = 0x00;
        packet[3] = packet.len() as u8;

        let msg = parse_bacnet_packet(&packet).unwrap();
        assert_eq!(msg.apdu_type, BacnetApduType::ConfirmedRequest);
        assert_eq!(msg.service, BacnetService::ReadProperty);
    }

    #[test]
    fn test_parse_bacnet_packet_unconfirmed_iam() {
        // BACnet I-Am packet
        let dev_oid: u32 = (8 << 22) | 5678; // device instance 5678
        let oid_bytes = dev_oid.to_be_bytes();
        let packet = vec![
            // BVLC
            0x81, 0x0B, 0x00, 0x14, // broadcast
            // NPDU
            0x01, 0x00, // version=1, control=0
            // APDU: Unconfirmed-Request
            0x10, // type=1 (unconfirmed)
            0x00, // service_choice=0 (I-Am)
            // Object Identifier application tag (0xC4 = app tag 12, length 4)
            0xC4,
            oid_bytes[0],
            oid_bytes[1],
            oid_bytes[2],
            oid_bytes[3],
            // remaining I-Am fields...
            0x22, 0x01, 0xE0, // max APDU
            0x91, 0x00, // segmentation
            0x21, 0x03, // vendor ID
        ];

        let msg = parse_bacnet_packet(&packet).unwrap();
        assert_eq!(msg.apdu_type, BacnetApduType::UnconfirmedRequest);
        assert_eq!(msg.service, BacnetService::IAm);
        assert_eq!(msg.device_instance, Some(5678));
    }

    #[test]
    fn test_bacnet_to_source_event() {
        let msg = BacnetMessage {
            bvlc: BacnetBvlcHeader {
                type_byte: 0x81,
                function: 0x0A,
                length: 24,
            },
            npdu: BacnetNpdu {
                version: 1,
                control: 0x04,
                hop_count: None,
            },
            apdu_type: BacnetApduType::ComplexAck,
            service: BacnetService::ReadProperty,
            object_id: Some(BacnetObjectId {
                object_type: 0,
                instance: 1,
            }),
            property_id: Some(85),
            property_values: vec![BacnetPropertyValue::Real(72.5)],
            device_instance: Some(1234),
        };

        let event = bacnet_to_source_event(&msg, "bacnet-test");
        assert_eq!(event.entity_type, "sensor");
        assert_eq!(event.entity_id, "bacnet:device-1234:analog-input-1");
        assert_eq!(event.properties["value"], json!(72.5));
        assert_eq!(event.properties["property_name"], json!("present-value"));
    }

    #[test]
    fn test_bacnet_entity_type() {
        let msg = BacnetMessage {
            bvlc: BacnetBvlcHeader {
                type_byte: 0x81,
                function: 0x0A,
                length: 0,
            },
            npdu: BacnetNpdu {
                version: 1,
                control: 0,
                hop_count: None,
            },
            apdu_type: BacnetApduType::ConfirmedRequest,
            service: BacnetService::ReadProperty,
            object_id: None,
            property_id: None,
            property_values: Vec::new(),
            device_instance: None,
        };
        let event = bacnet_to_source_event(&msg, "test");
        assert_eq!(event.entity_type, "sensor");
    }

    #[test]
    fn test_bacnet_object_types() {
        let types = vec![
            (0, "analog-input"),
            (1, "analog-output"),
            (2, "analog-value"),
            (3, "binary-input"),
            (4, "binary-output"),
            (5, "binary-value"),
            (8, "device"),
            (13, "multi-state-input"),
            (99, "unknown"),
        ];
        for (code, name) in types {
            let oid = BacnetObjectId {
                object_type: code,
                instance: 0,
            };
            assert_eq!(oid.object_type_name(), name);
        }
    }

    #[test]
    fn test_parse_bacnet_json() {
        let json_str = r#"{"device_instance": 1234, "object_type": "analog-input",
            "object_instance": 5, "property": "present-value", "value": 72.5,
            "service": "ReadProperty"}"#;
        let msg = parse_bacnet_json(json_str).unwrap();
        assert_eq!(msg.device_instance, Some(1234));
        assert_eq!(msg.service, BacnetService::ReadProperty);
        assert_eq!(msg.object_id.as_ref().unwrap().object_type, 0);
        assert_eq!(msg.object_id.as_ref().unwrap().instance, 5);
        assert_eq!(msg.property_values.len(), 1);
    }

    #[test]
    fn test_bacnet_connector_id() {
        let config = ConnectorConfig {
            connector_id: "bacnet-1".to_string(),
            connector_type: "bacnet".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = BacnetConnector::new(config);
        assert_eq!(connector.connector_id(), "bacnet-1");
    }

    #[tokio::test]
    async fn test_bacnet_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "bacnet-h".to_string(),
            connector_type: "bacnet".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = BacnetConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_bacnet_property_values() {
        assert_eq!(
            BacnetPropertyValue::Real(72.5).to_json(),
            json!(72.5)
        );
        assert_eq!(
            BacnetPropertyValue::Boolean(true).to_json(),
            json!(true)
        );
        assert_eq!(
            BacnetPropertyValue::UnsignedInt(42).to_json(),
            json!(42)
        );
        assert_eq!(
            BacnetPropertyValue::CharString("test".into()).to_json(),
            json!("test")
        );
        assert_eq!(BacnetPropertyValue::Null.to_json(), json!(null));
    }

    #[test]
    fn test_bacnet_service_names() {
        assert_eq!(BacnetService::ReadProperty.as_str(), "ReadProperty");
        assert_eq!(BacnetService::WriteProperty.as_str(), "WriteProperty");
        assert_eq!(BacnetService::IAm.as_str(), "I-Am");
        assert_eq!(BacnetService::WhoIs.as_str(), "Who-Is");
    }
}
