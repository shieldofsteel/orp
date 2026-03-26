use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// LoRaWAN frame parser
// ---------------------------------------------------------------------------
// LoRaWAN (Long Range Wide Area Network) is a low-power, long-range wireless
// protocol for IoT sensors (agriculture, asset tracking, smart city).
//
// PHYPayload structure:
//   MHDR (1 byte) + MACPayload (variable) + MIC (4 bytes)
//
// MHDR byte:
//   MType (3 bits, bits 7-5) | RFU (3 bits, bits 4-2) | Major (2 bits, bits 1-0)
//
// MType values:
//   000 = Join Request
//   001 = Join Accept
//   010 = Unconfirmed Data Up
//   011 = Unconfirmed Data Down
//   100 = Confirmed Data Up
//   101 = Confirmed Data Down
//   110 = Rejoin Request
//   111 = Proprietary
//
// For data messages (MType 010–101), MACPayload = FHDR + FPort(1) + FRMPayload
//
// FHDR = DevAddr(4 bytes LE) + FCtrl(1) + FCnt(2 bytes LE) + FOpts(0–15 bytes)
//
// FCtrl (uplink):  ADR(1) | ADRACKReq(1) | ACK(1) | ClassB(1) | FOptsLen(4)
// FCtrl (downlink): ADR(1) | RFU(1)     | ACK(1) | FPending(1) | FOptsLen(4)
//
// FPort: 0 = MAC commands in FRMPayload, 1–223 = application, 224+ = reserved
//
// Join Request payload: AppEUI(8) + DevEUI(8) + DevNonce(2)

/// LoRaWAN message type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoRaWanMType {
    JoinRequest,
    JoinAccept,
    UnconfirmedDataUp,
    UnconfirmedDataDown,
    ConfirmedDataUp,
    ConfirmedDataDown,
    RejoinRequest,
    Proprietary,
}

impl LoRaWanMType {
    pub fn from_bits(bits: u8) -> Self {
        match bits {
            0 => LoRaWanMType::JoinRequest,
            1 => LoRaWanMType::JoinAccept,
            2 => LoRaWanMType::UnconfirmedDataUp,
            3 => LoRaWanMType::UnconfirmedDataDown,
            4 => LoRaWanMType::ConfirmedDataUp,
            5 => LoRaWanMType::ConfirmedDataDown,
            6 => LoRaWanMType::RejoinRequest,
            _ => LoRaWanMType::Proprietary,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LoRaWanMType::JoinRequest => "JoinRequest",
            LoRaWanMType::JoinAccept => "JoinAccept",
            LoRaWanMType::UnconfirmedDataUp => "UnconfirmedDataUp",
            LoRaWanMType::UnconfirmedDataDown => "UnconfirmedDataDown",
            LoRaWanMType::ConfirmedDataUp => "ConfirmedDataUp",
            LoRaWanMType::ConfirmedDataDown => "ConfirmedDataDown",
            LoRaWanMType::RejoinRequest => "RejoinRequest",
            LoRaWanMType::Proprietary => "Proprietary",
        }
    }

    pub fn is_uplink(&self) -> bool {
        matches!(
            self,
            LoRaWanMType::UnconfirmedDataUp | LoRaWanMType::ConfirmedDataUp
        )
    }

    pub fn is_data(&self) -> bool {
        matches!(
            self,
            LoRaWanMType::UnconfirmedDataUp
                | LoRaWanMType::UnconfirmedDataDown
                | LoRaWanMType::ConfirmedDataUp
                | LoRaWanMType::ConfirmedDataDown
        )
    }
}

/// Parsed MHDR byte.
#[derive(Clone, Debug)]
pub struct LoRaWanMhdr {
    pub mtype: LoRaWanMType,
    pub major: u8,
}

/// Parsed FCtrl byte (uplink perspective).
#[derive(Clone, Debug)]
pub struct LoRaWanFCtrl {
    pub adr: bool,
    pub adr_ack_req: bool,
    pub ack: bool,
    pub class_b_or_fpending: bool,
    pub fopts_len: u8,
}

/// Parsed FHDR (Frame Header).
#[derive(Clone, Debug)]
pub struct LoRaWanFhdr {
    pub dev_addr: u32,
    pub fctrl: LoRaWanFCtrl,
    pub fcnt: u16,
    pub fopts: Vec<u8>,
}

/// Data message payload.
#[derive(Clone, Debug)]
pub struct LoRaWanDataPayload {
    pub fhdr: LoRaWanFhdr,
    pub fport: Option<u8>,
    pub frm_payload: Vec<u8>,
}

/// Join Request payload.
#[derive(Clone, Debug)]
pub struct LoRaWanJoinRequest {
    pub app_eui: u64,
    pub dev_eui: u64,
    pub dev_nonce: u16,
}

/// LoRaWAN payload variants.
#[derive(Clone, Debug)]
pub enum LoRaWanPayload {
    Data(LoRaWanDataPayload),
    JoinRequest(LoRaWanJoinRequest),
    JoinAccept(Vec<u8>),
    Raw(Vec<u8>),
}

/// Complete parsed LoRaWAN frame.
#[derive(Clone, Debug)]
pub struct LoRaWanFrame {
    pub mhdr: LoRaWanMhdr,
    pub payload: LoRaWanPayload,
    pub mic: u32,
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse MHDR byte.
pub fn parse_lorawan_mhdr(byte: u8) -> LoRaWanMhdr {
    let mtype = LoRaWanMType::from_bits((byte >> 5) & 0x07);
    let major = byte & 0x03;
    LoRaWanMhdr { mtype, major }
}

/// Parse FCtrl byte (uplink).
pub fn parse_lorawan_fctrl_uplink(byte: u8) -> LoRaWanFCtrl {
    LoRaWanFCtrl {
        adr: byte & 0x80 != 0,
        adr_ack_req: byte & 0x40 != 0,
        ack: byte & 0x20 != 0,
        class_b_or_fpending: byte & 0x10 != 0,
        fopts_len: byte & 0x0F,
    }
}

/// Parse FHDR from data slice. Returns (FHDR, bytes consumed).
pub fn parse_lorawan_fhdr(data: &[u8]) -> Result<(LoRaWanFhdr, usize), ConnectorError> {
    if data.len() < 7 {
        return Err(ConnectorError::ParseError(
            "LoRaWAN: FHDR too short (need at least 7 bytes)".into(),
        ));
    }

    let dev_addr = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let fctrl = parse_lorawan_fctrl_uplink(data[4]);
    let fcnt = u16::from_le_bytes([data[5], data[6]]);

    let fopts_len = fctrl.fopts_len as usize;
    if data.len() < 7 + fopts_len {
        return Err(ConnectorError::ParseError(
            "LoRaWAN: FHDR FOpts extends beyond data".into(),
        ));
    }

    let fopts = data[7..7 + fopts_len].to_vec();
    let consumed = 7 + fopts_len;

    Ok((
        LoRaWanFhdr {
            dev_addr,
            fctrl,
            fcnt,
            fopts,
        },
        consumed,
    ))
}

/// Parse a complete LoRaWAN PHYPayload from binary data.
pub fn parse_lorawan_frame(data: &[u8]) -> Result<LoRaWanFrame, ConnectorError> {
    if data.len() < 5 {
        return Err(ConnectorError::ParseError(
            "LoRaWAN: frame too short (need at least 5 bytes: MHDR + MIC)".into(),
        ));
    }

    let mhdr = parse_lorawan_mhdr(data[0]);
    let mic_offset = data.len() - 4;
    let mic = u32::from_le_bytes([
        data[mic_offset],
        data[mic_offset + 1],
        data[mic_offset + 2],
        data[mic_offset + 3],
    ]);

    let mac_payload = &data[1..mic_offset];

    let payload = match mhdr.mtype {
        LoRaWanMType::JoinRequest => {
            if mac_payload.len() < 18 {
                return Err(ConnectorError::ParseError(
                    "LoRaWAN: Join Request payload too short".into(),
                ));
            }
            let app_eui = u64::from_le_bytes([
                mac_payload[0],
                mac_payload[1],
                mac_payload[2],
                mac_payload[3],
                mac_payload[4],
                mac_payload[5],
                mac_payload[6],
                mac_payload[7],
            ]);
            let dev_eui = u64::from_le_bytes([
                mac_payload[8],
                mac_payload[9],
                mac_payload[10],
                mac_payload[11],
                mac_payload[12],
                mac_payload[13],
                mac_payload[14],
                mac_payload[15],
            ]);
            let dev_nonce = u16::from_le_bytes([mac_payload[16], mac_payload[17]]);
            LoRaWanPayload::JoinRequest(LoRaWanJoinRequest {
                app_eui,
                dev_eui,
                dev_nonce,
            })
        }
        LoRaWanMType::JoinAccept => {
            LoRaWanPayload::JoinAccept(mac_payload.to_vec())
        }
        mtype if mtype.is_data() => {
            let (fhdr, consumed) = parse_lorawan_fhdr(mac_payload)?;
            let remaining = &mac_payload[consumed..];
            let (fport, frm_payload) = if remaining.is_empty() {
                (None, Vec::new())
            } else {
                (Some(remaining[0]), remaining[1..].to_vec())
            };
            LoRaWanPayload::Data(LoRaWanDataPayload {
                fhdr,
                fport,
                frm_payload,
            })
        }
        _ => LoRaWanPayload::Raw(mac_payload.to_vec()),
    };

    Ok(LoRaWanFrame {
        mhdr,
        payload,
        mic,
    })
}

/// Parse LoRaWAN frame from hex string.
pub fn parse_lorawan_hex(hex_str: &str) -> Result<LoRaWanFrame, ConnectorError> {
    let hex_str = hex_str.trim().replace(' ', "");
    let bytes: Result<Vec<u8>, _> = (0..hex_str.len())
        .step_by(2)
        .map(|i| {
            if i + 2 <= hex_str.len() {
                u8::from_str_radix(&hex_str[i..i + 2], 16)
                    .map_err(|_| ConnectorError::ParseError("LoRaWAN: invalid hex".into()))
            } else {
                Err(ConnectorError::ParseError(
                    "LoRaWAN: odd hex string length".into(),
                ))
            }
        })
        .collect();
    parse_lorawan_frame(&bytes?)
}

// ---------------------------------------------------------------------------
// LoRaWAN → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a LoRaWAN frame to a SourceEvent.
pub fn lorawan_to_source_event(
    frame: &LoRaWanFrame,
    connector_id: &str,
) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert("mtype".into(), json!(frame.mhdr.mtype.as_str()));
    properties.insert("major".into(), json!(frame.mhdr.major));
    properties.insert("mic".into(), json!(format!("{:08X}", frame.mic)));

    let entity_id = match &frame.payload {
        LoRaWanPayload::Data(data) => {
            properties.insert(
                "dev_addr".into(),
                json!(format!("{:08X}", data.fhdr.dev_addr)),
            );
            properties.insert("fcnt".into(), json!(data.fhdr.fcnt));
            properties.insert("adr".into(), json!(data.fhdr.fctrl.adr));
            properties.insert("ack".into(), json!(data.fhdr.fctrl.ack));
            properties.insert(
                "fopts_len".into(),
                json!(data.fhdr.fctrl.fopts_len),
            );

            if let Some(fport) = data.fport {
                properties.insert("fport".into(), json!(fport));
                if fport == 0 {
                    properties.insert("fport_type".into(), json!("MAC commands"));
                } else if fport <= 223 {
                    properties.insert("fport_type".into(), json!("Application"));
                } else {
                    properties.insert("fport_type".into(), json!("Reserved"));
                }
            }

            if !data.frm_payload.is_empty() {
                let hex: String = data
                    .frm_payload
                    .iter()
                    .map(|b| format!("{:02X}", b))
                    .collect();
                properties.insert("frm_payload_hex".into(), json!(hex));
                properties.insert(
                    "frm_payload_length".into(),
                    json!(data.frm_payload.len()),
                );
            }

            format!("lorawan:{:08X}", data.fhdr.dev_addr)
        }
        LoRaWanPayload::JoinRequest(jr) => {
            properties.insert(
                "app_eui".into(),
                json!(format!("{:016X}", jr.app_eui)),
            );
            properties.insert(
                "dev_eui".into(),
                json!(format!("{:016X}", jr.dev_eui)),
            );
            properties.insert("dev_nonce".into(), json!(jr.dev_nonce));
            format!("lorawan:join:{:016X}", jr.dev_eui)
        }
        LoRaWanPayload::JoinAccept(data) => {
            let hex: String = data.iter().map(|b| format!("{:02X}", b)).collect();
            properties.insert("join_accept_data".into(), json!(hex));
            "lorawan:join_accept".to_string()
        }
        LoRaWanPayload::Raw(data) => {
            let hex: String = data.iter().map(|b| format!("{:02X}", b)).collect();
            properties.insert("raw_payload".into(), json!(hex));
            "lorawan:raw".to_string()
        }
    };

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

pub struct LoRaWanConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl LoRaWanConnector {
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
impl Connector for LoRaWanConnector {
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
                ConnectorError::ConfigError("LoRaWAN: url (file path) required".into())
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
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Try JSON format first: {"payload_hex": "..."}
            if line.starts_with('{') {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(hex) = val.get("payload_hex").and_then(|v| v.as_str()) {
                        match parse_lorawan_hex(hex) {
                            Ok(frame) => {
                                let event =
                                    lorawan_to_source_event(&frame, &connector_id);
                                if tx.send(event).await.is_err() {
                                    break;
                                }
                                events_processed.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                            Err(_) => {
                                errors.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                        }
                    }
                }
            }

            // Try raw hex format
            match parse_lorawan_hex(line) {
                Ok(frame) => {
                    let event = lorawan_to_source_event(&frame, &connector_id);
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
                "LoRaWAN connector is not running".into(),
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
    fn test_parse_mhdr_unconfirmed_up() {
        // MType=010, RFU=000, Major=00 → 0b01000000 = 0x40
        let mhdr = parse_lorawan_mhdr(0x40);
        assert_eq!(mhdr.mtype, LoRaWanMType::UnconfirmedDataUp);
        assert_eq!(mhdr.major, 0);
        assert!(mhdr.mtype.is_uplink());
        assert!(mhdr.mtype.is_data());
    }

    #[test]
    fn test_parse_mhdr_confirmed_up() {
        // MType=100, Major=00 → 0b10000000 = 0x80
        let mhdr = parse_lorawan_mhdr(0x80);
        assert_eq!(mhdr.mtype, LoRaWanMType::ConfirmedDataUp);
        assert!(mhdr.mtype.is_uplink());
    }

    #[test]
    fn test_parse_mhdr_join_request() {
        // MType=000, Major=00 → 0x00
        let mhdr = parse_lorawan_mhdr(0x00);
        assert_eq!(mhdr.mtype, LoRaWanMType::JoinRequest);
        assert!(!mhdr.mtype.is_data());
    }

    #[test]
    fn test_parse_fctrl_uplink() {
        // ADR=1, ADRACKReq=0, ACK=1, ClassB=0, FOptsLen=3 → 0b10100011 = 0xA3
        let fctrl = parse_lorawan_fctrl_uplink(0xA3);
        assert!(fctrl.adr);
        assert!(!fctrl.adr_ack_req);
        assert!(fctrl.ack);
        assert!(!fctrl.class_b_or_fpending);
        assert_eq!(fctrl.fopts_len, 3);
    }

    #[test]
    fn test_parse_fhdr() {
        // DevAddr=0x01020304 (LE), FCtrl=0x80 (ADR=1), FCnt=0x000A (LE), no FOpts
        let data = [0x04, 0x03, 0x02, 0x01, 0x80, 0x0A, 0x00];
        let (fhdr, consumed) = parse_lorawan_fhdr(&data).unwrap();
        assert_eq!(fhdr.dev_addr, 0x01020304);
        assert!(fhdr.fctrl.adr);
        assert_eq!(fhdr.fcnt, 10);
        assert!(fhdr.fopts.is_empty());
        assert_eq!(consumed, 7);
    }

    #[test]
    fn test_parse_lorawan_frame_data_up() {
        // Unconfirmed Data Up: MHDR(0x40) + DevAddr(4) + FCtrl(1) + FCnt(2) + MIC(4)
        let mut frame = vec![
            0x40, // MHDR: UnconfirmedDataUp
            0x04, 0x03, 0x02, 0x01, // DevAddr (LE)
            0x00, // FCtrl: no flags, fopts_len=0
            0x01, 0x00, // FCnt=1 (LE)
            // MIC (4 bytes)
            0xAA, 0xBB, 0xCC, 0xDD,
        ];
        let _ = frame.len();

        let parsed = parse_lorawan_frame(&frame).unwrap();
        assert_eq!(parsed.mhdr.mtype, LoRaWanMType::UnconfirmedDataUp);
        assert_eq!(parsed.mic, 0xDDCCBBAA);
        if let LoRaWanPayload::Data(data) = &parsed.payload {
            assert_eq!(data.fhdr.dev_addr, 0x01020304);
            assert_eq!(data.fhdr.fcnt, 1);
            assert!(data.fport.is_none());
        } else {
            panic!("Expected Data payload");
        }
    }

    #[test]
    fn test_parse_lorawan_frame_with_fport() {
        // Unconfirmed Data Up with FPort and payload
        let frame = vec![
            0x40, // MHDR: UnconfirmedDataUp
            0x78, 0x56, 0x34, 0x12, // DevAddr
            0x00, // FCtrl
            0x05, 0x00, // FCnt=5
            0x01, // FPort=1 (application)
            0xDE, 0xAD, 0xBE, 0xEF, // FRMPayload
            // MIC
            0x11, 0x22, 0x33, 0x44,
        ];

        let parsed = parse_lorawan_frame(&frame).unwrap();
        if let LoRaWanPayload::Data(data) = &parsed.payload {
            assert_eq!(data.fhdr.dev_addr, 0x12345678);
            assert_eq!(data.fport, Some(1));
            assert_eq!(data.frm_payload, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        } else {
            panic!("Expected Data payload");
        }
    }

    #[test]
    fn test_parse_lorawan_hex() {
        let hex = "40 04 03 02 01 00 01 00 AA BB CC DD";
        let frame = parse_lorawan_hex(hex).unwrap();
        assert_eq!(frame.mhdr.mtype, LoRaWanMType::UnconfirmedDataUp);
    }

    #[test]
    fn test_lorawan_to_source_event() {
        let frame = LoRaWanFrame {
            mhdr: LoRaWanMhdr {
                mtype: LoRaWanMType::UnconfirmedDataUp,
                major: 0,
            },
            payload: LoRaWanPayload::Data(LoRaWanDataPayload {
                fhdr: LoRaWanFhdr {
                    dev_addr: 0x01020304,
                    fctrl: LoRaWanFCtrl {
                        adr: true,
                        adr_ack_req: false,
                        ack: false,
                        class_b_or_fpending: false,
                        fopts_len: 0,
                    },
                    fcnt: 42,
                    fopts: Vec::new(),
                },
                fport: Some(1),
                frm_payload: vec![0xAA, 0xBB],
            }),
            mic: 0xDEADBEEF,
        };

        let event = lorawan_to_source_event(&frame, "lorawan-test");
        assert_eq!(event.entity_type, "sensor");
        assert_eq!(event.entity_id, "lorawan:01020304");
        assert_eq!(event.properties["mtype"], json!("UnconfirmedDataUp"));
        assert_eq!(event.properties["fcnt"], json!(42));
        assert_eq!(event.properties["fport"], json!(1));
        assert_eq!(event.properties["frm_payload_hex"], json!("AABB"));
    }

    #[test]
    fn test_lorawan_entity_type() {
        let frame = LoRaWanFrame {
            mhdr: LoRaWanMhdr {
                mtype: LoRaWanMType::ConfirmedDataUp,
                major: 0,
            },
            payload: LoRaWanPayload::Data(LoRaWanDataPayload {
                fhdr: LoRaWanFhdr {
                    dev_addr: 0xAABBCCDD,
                    fctrl: parse_lorawan_fctrl_uplink(0),
                    fcnt: 0,
                    fopts: Vec::new(),
                },
                fport: None,
                frm_payload: Vec::new(),
            }),
            mic: 0,
        };
        let event = lorawan_to_source_event(&frame, "test");
        assert_eq!(event.entity_type, "sensor");
    }

    #[test]
    fn test_lorawan_frame_too_short() {
        let data = vec![0x40, 0x01, 0x02];
        assert!(parse_lorawan_frame(&data).is_err());
    }

    #[test]
    fn test_lorawan_join_request() {
        // Join Request: MHDR(0x00) + AppEUI(8) + DevEUI(8) + DevNonce(2) + MIC(4)
        let mut frame = vec![0x00]; // MHDR: JoinRequest
        // AppEUI (LE)
        frame.extend_from_slice(&0x0102030405060708u64.to_le_bytes());
        // DevEUI (LE)
        frame.extend_from_slice(&0x1112131415161718u64.to_le_bytes());
        // DevNonce (LE)
        frame.extend_from_slice(&0x00ABu16.to_le_bytes());
        // MIC
        frame.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);

        let parsed = parse_lorawan_frame(&frame).unwrap();
        assert_eq!(parsed.mhdr.mtype, LoRaWanMType::JoinRequest);
        if let LoRaWanPayload::JoinRequest(jr) = &parsed.payload {
            assert_eq!(jr.app_eui, 0x0102030405060708);
            assert_eq!(jr.dev_eui, 0x1112131415161718);
            assert_eq!(jr.dev_nonce, 0x00AB);
        } else {
            panic!("Expected JoinRequest payload");
        }
    }

    #[test]
    fn test_lorawan_connector_id() {
        let config = ConnectorConfig {
            connector_id: "lorawan-1".to_string(),
            connector_type: "lorawan".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = LoRaWanConnector::new(config);
        assert_eq!(connector.connector_id(), "lorawan-1");
    }

    #[tokio::test]
    async fn test_lorawan_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "lorawan-h".to_string(),
            connector_type: "lorawan".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = LoRaWanConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }
}
