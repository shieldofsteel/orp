//! MISB ST 0601 KLV (Key-Length-Value) UAV video metadata adapter.
//!
//! Group 3+ UAVs (MQ-9, MQ-1C, ScanEagle, Switchblade-600 GCS) emit MISB
//! ST 0601 metadata in-band with H.264/H.265 video. Without parsing it, drone
//! video can't be fused with ground tracks.
//!
//! Wire format: 16-byte Universal Key + BER-encoded length + local set
//! (1-byte tag + BER-encoded length + value bytes), repeated. Tag 1 holds a
//! 16-bit BSD checksum covering key+length+all bytes up to (but excluding)
//! the checksum value itself.
//!
//! URL schemes:
//!   `klv://0.0.0.0:8000`     — raw KLV over UDP
//!   `klv-ts://0.0.0.0:8000`  — KLV inside MPEG-TS (Phase 1: deferred)

#![allow(dead_code)]

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// MISB ST 0601 Universal Key (SMPTE 336M UL designator).
pub const ST0601_UNIVERSAL_KEY: [u8; 16] = [
    0x06, 0x0E, 0x2B, 0x34, 0x02, 0x0B, 0x01, 0x01, 0x0E, 0x01, 0x03, 0x01, 0x01, 0x00, 0x00, 0x00,
];

/// ST 0601 local-set tag identifiers we decode.
pub mod tag {
    pub const CHECKSUM: u8 = 1;
    pub const PRECISION_TIMESTAMP: u8 = 2;
    pub const MISSION_ID: u8 = 3;
    pub const PLATFORM_TAIL_NUMBER: u8 = 4;
    pub const PLATFORM_HEADING_ANGLE: u8 = 5;
    pub const PLATFORM_PITCH_ANGLE: u8 = 6;
    pub const PLATFORM_ROLL_ANGLE: u8 = 7;
    pub const PLATFORM_DESIGNATION: u8 = 10;
    pub const SENSOR_LATITUDE: u8 = 13;
    pub const SENSOR_LONGITUDE: u8 = 14;
    pub const SENSOR_TRUE_ALTITUDE: u8 = 15;
    pub const SENSOR_HFOV: u8 = 16;
    pub const SENSOR_VFOV: u8 = 17;
    pub const FRAME_CENTRE_LATITUDE: u8 = 23;
    pub const FRAME_CENTRE_LONGITUDE: u8 = 24;
    pub const FRAME_CENTRE_ELEVATION: u8 = 25;
}

// ---------------------------------------------------------------------------
// Decoded packet
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct St0601Packet {
    pub precision_timestamp_us: Option<u64>,
    pub mission_id: Option<String>,
    pub platform_tail_number: Option<String>,
    pub platform_heading_deg: Option<f64>,
    pub platform_pitch_deg: Option<f64>,
    pub platform_roll_deg: Option<f64>,
    pub platform_designation: Option<String>,
    pub sensor_latitude_deg: Option<f64>,
    pub sensor_longitude_deg: Option<f64>,
    pub sensor_true_altitude_m: Option<f64>,
    pub sensor_hfov_deg: Option<f64>,
    pub sensor_vfov_deg: Option<f64>,
    pub frame_centre_latitude_deg: Option<f64>,
    pub frame_centre_longitude_deg: Option<f64>,
    pub frame_centre_elevation_m: Option<f64>,
    pub raw_tags: Vec<(u8, Vec<u8>)>,
}

// ---------------------------------------------------------------------------
// BER length
// ---------------------------------------------------------------------------

/// Parse a BER length. Returns (length, bytes_consumed).
/// Short form: first byte MSB=0, length=byte (0..127).
/// Long form: first byte = 0x80|n where n in 1..=8; next n bytes are BE length.
pub fn parse_ber_length(buf: &[u8]) -> Result<(usize, usize), ConnectorError> {
    if buf.is_empty() {
        return Err(ConnectorError::ParseError("KLV: empty BER length".into()));
    }
    let first = buf[0];
    if first & 0x80 == 0 {
        return Ok((first as usize, 1));
    }
    let n = (first & 0x7F) as usize;
    if n == 0 {
        return Err(ConnectorError::ParseError(
            "KLV: indefinite BER length not supported".into(),
        ));
    }
    if n > 8 {
        return Err(ConnectorError::ParseError(format!(
            "KLV: BER length too large ({} bytes)",
            n
        )));
    }
    if 1 + n > buf.len() {
        return Err(ConnectorError::ParseError(
            "KLV: truncated BER length".into(),
        ));
    }
    let mut len: u64 = 0;
    for &b in &buf[1..=n] {
        len = (len << 8) | (b as u64);
    }
    if len > usize::MAX as u64 {
        return Err(ConnectorError::ParseError(
            "KLV: BER length overflow".into(),
        ));
    }
    Ok((len as usize, 1 + n))
}

// ---------------------------------------------------------------------------
// BSD 16-bit checksum (MISB ST 0601 §7.1)
// ---------------------------------------------------------------------------

/// 16-bit BSD checksum: even-index bytes go to the high byte, odd-index bytes
/// to the low byte; running u16 sum with wrap-around.
pub fn bsd_checksum_16(data: &[u8]) -> u16 {
    let mut sum: u16 = 0;
    for (i, &b) in data.iter().enumerate() {
        let shift = 8 * ((i + 1) % 2);
        sum = sum.wrapping_add((b as u16) << shift);
    }
    sum
}

// ---------------------------------------------------------------------------
// Tag value decoders. Per MISB ST 0601: 0x80000000 / 0x8000 are reserved
// "out-of-range" sentinels for signed lat/lon and pitch/roll respectively.
// ---------------------------------------------------------------------------

/// int32 → ±90° (i32::MIN reserved sentinel).
pub fn decode_lat_int32(raw: &[u8]) -> Option<f64> {
    let v = (raw.len() == 4).then(|| i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))?;
    if v == i32::MIN {
        return None;
    }
    Some(v as f64 * 90.0 / (i32::MAX as f64))
}

/// int32 → ±180°.
pub fn decode_lon_int32(raw: &[u8]) -> Option<f64> {
    let v = (raw.len() == 4).then(|| i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))?;
    if v == i32::MIN {
        return None;
    }
    Some(v as f64 * 180.0 / (i32::MAX as f64))
}

/// uint16 → 0..360°.
pub fn decode_heading_uint16(raw: &[u8]) -> Option<f64> {
    let v = (raw.len() == 2).then(|| u16::from_be_bytes([raw[0], raw[1]]))?;
    Some(v as f64 * 360.0 / 65535.0)
}

/// int16 → ±20° (i16::MIN reserved).
pub fn decode_pitch_roll_int16(raw: &[u8]) -> Option<f64> {
    let v = (raw.len() == 2).then(|| i16::from_be_bytes([raw[0], raw[1]]))?;
    if v == i16::MIN {
        return None;
    }
    Some(v as f64 * 20.0 / (i16::MAX as f64))
}

/// uint16 → −900..+19000 m (per ST 0601).
pub fn decode_altitude_uint16(raw: &[u8]) -> Option<f64> {
    let v = (raw.len() == 2).then(|| u16::from_be_bytes([raw[0], raw[1]]))?;
    Some(v as f64 * 19900.0 / 65535.0 - 900.0)
}

/// uint16 → 0..180°.
pub fn decode_fov_uint16(raw: &[u8]) -> Option<f64> {
    let v = (raw.len() == 2).then(|| u16::from_be_bytes([raw[0], raw[1]]))?;
    Some(v as f64 * 180.0 / 65535.0)
}

/// 8-byte big-endian u64 microseconds since UNIX epoch.
pub fn decode_precision_timestamp(raw: &[u8]) -> Option<u64> {
    if raw.len() != 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(raw);
    Some(u64::from_be_bytes(buf))
}

fn decode_ascii(raw: &[u8]) -> Option<String> {
    if raw.is_empty() || !raw.iter().all(|b| (0x20..=0x7E).contains(b)) {
        return None;
    }
    Some(String::from_utf8_lossy(raw).into_owned())
}

// ---------------------------------------------------------------------------
// Local-set parser
// ---------------------------------------------------------------------------

/// Parse a single ST 0601 local set starting at `buf[0]`.
/// Returns `(packet, total_bytes_consumed)` on success.
/// On checksum mismatch, returns `ParseError("…checksum mismatch…")`.
pub fn parse_st0601_set(buf: &[u8]) -> Result<(St0601Packet, usize), ConnectorError> {
    let parse_err = |s: String| ConnectorError::ParseError(s);
    if buf.len() < ST0601_UNIVERSAL_KEY.len() {
        return Err(parse_err("KLV: buffer too short for universal key".into()));
    }
    if buf[..16] != ST0601_UNIVERSAL_KEY {
        return Err(parse_err("KLV: universal key mismatch".into()));
    }
    let (set_len, len_consumed) = parse_ber_length(&buf[16..])?;
    let header_len = 16 + len_consumed;
    let total_len = header_len + set_len;
    if total_len > buf.len() {
        return Err(parse_err(format!(
            "KLV: declared length {} exceeds buffer ({})",
            total_len,
            buf.len()
        )));
    }

    let body = &buf[header_len..total_len];
    let mut packet = St0601Packet::default();
    let mut idx = 0usize;

    while idx < body.len() {
        let tag = body[idx];
        idx += 1;
        if idx >= body.len() {
            return Err(parse_err("KLV: missing length after tag".into()));
        }
        let (val_len, vl_consumed) = parse_ber_length(&body[idx..])?;
        idx += vl_consumed;
        if idx + val_len > body.len() {
            return Err(parse_err(format!(
                "KLV: tag {} value length {} exceeds set body",
                tag, val_len
            )));
        }
        let val = &body[idx..idx + val_len];

        match tag {
            tag::CHECKSUM => {
                if val.len() != 2 {
                    return Err(parse_err("KLV: checksum value must be 2 bytes".into()));
                }
                // Checksum covers everything up to but excluding the value.
                let claimed = u16::from_be_bytes([val[0], val[1]]);
                let computed = bsd_checksum_16(&buf[..header_len + idx]);
                if claimed != computed {
                    return Err(parse_err(format!(
                        "KLV: checksum mismatch (claimed=0x{:04X}, computed=0x{:04X})",
                        claimed, computed
                    )));
                }
            }
            tag::PRECISION_TIMESTAMP => {
                packet.precision_timestamp_us = decode_precision_timestamp(val)
            }
            tag::MISSION_ID => packet.mission_id = decode_ascii(val),
            tag::PLATFORM_TAIL_NUMBER => packet.platform_tail_number = decode_ascii(val),
            tag::PLATFORM_HEADING_ANGLE => packet.platform_heading_deg = decode_heading_uint16(val),
            tag::PLATFORM_PITCH_ANGLE => packet.platform_pitch_deg = decode_pitch_roll_int16(val),
            tag::PLATFORM_ROLL_ANGLE => packet.platform_roll_deg = decode_pitch_roll_int16(val),
            tag::PLATFORM_DESIGNATION => packet.platform_designation = decode_ascii(val),
            tag::SENSOR_LATITUDE => packet.sensor_latitude_deg = decode_lat_int32(val),
            tag::SENSOR_LONGITUDE => packet.sensor_longitude_deg = decode_lon_int32(val),
            tag::SENSOR_TRUE_ALTITUDE => {
                packet.sensor_true_altitude_m = decode_altitude_uint16(val)
            }
            tag::SENSOR_HFOV => packet.sensor_hfov_deg = decode_fov_uint16(val),
            tag::SENSOR_VFOV => packet.sensor_vfov_deg = decode_fov_uint16(val),
            tag::FRAME_CENTRE_LATITUDE => packet.frame_centre_latitude_deg = decode_lat_int32(val),
            tag::FRAME_CENTRE_LONGITUDE => {
                packet.frame_centre_longitude_deg = decode_lon_int32(val)
            }
            tag::FRAME_CENTRE_ELEVATION => {
                packet.frame_centre_elevation_m = decode_altitude_uint16(val)
            }
            other => packet.raw_tags.push((other, val.to_vec())),
        }
        idx += val_len;
    }

    Ok((packet, total_len))
}

/// Parse one or more concatenated ST 0601 sets, skipping 0x00 padding between
/// them. Stops at the first parse error.
pub fn parse_all_sets(buf: &[u8]) -> (Vec<St0601Packet>, Result<(), ConnectorError>) {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset < buf.len() {
        if buf[offset] == 0 {
            offset += 1;
            continue;
        }
        match parse_st0601_set(&buf[offset..]) {
            Ok((pkt, used)) => {
                out.push(pkt);
                offset += used;
            }
            Err(e) => return (out, Err(e)),
        }
    }
    (out, Ok(()))
}

// ---------------------------------------------------------------------------
// SourceEvent mapping
// ---------------------------------------------------------------------------

/// Convert a parsed packet to an ORP SourceEvent. `src_addr` is the wire-level
/// peer; used only as a final-fallback entity_id.
pub fn packet_to_source_event(
    pkt: &St0601Packet,
    connector_id: &str,
    src_addr: &str,
) -> SourceEvent {
    let entity_id = match (&pkt.mission_id, &pkt.platform_tail_number) {
        (Some(m), Some(t)) => format!("{}-{}", m, t),
        (_, Some(t)) => format!("klv-{}", t),
        _ => format!("klv-{}", src_addr),
    };

    let mut p: HashMap<String, serde_json::Value> = HashMap::new();
    macro_rules! ins {
        ($k:expr, $v:expr) => {
            if let Some(v) = $v {
                p.insert($k.into(), json!(v));
            }
        };
    }
    ins!("precision_timestamp_us", pkt.precision_timestamp_us);
    ins!("mission_id", pkt.mission_id.as_ref());
    ins!("platform_tail_number", pkt.platform_tail_number.as_ref());
    ins!("platform_heading_deg", pkt.platform_heading_deg);
    ins!("platform_pitch_deg", pkt.platform_pitch_deg);
    ins!("platform_roll_deg", pkt.platform_roll_deg);
    ins!("platform_designation", pkt.platform_designation.as_ref());
    ins!("sensor_true_altitude_m", pkt.sensor_true_altitude_m);
    ins!("sensor_hfov_deg", pkt.sensor_hfov_deg);
    ins!("sensor_vfov_deg", pkt.sensor_vfov_deg);
    ins!("frame_centre_latitude_deg", pkt.frame_centre_latitude_deg);
    ins!("frame_centre_longitude_deg", pkt.frame_centre_longitude_deg);
    ins!("frame_centre_elevation_m", pkt.frame_centre_elevation_m);

    let timestamp = pkt
        .precision_timestamp_us
        .and_then(|us| {
            let secs = (us / 1_000_000) as i64;
            let nanos = ((us % 1_000_000) * 1_000) as u32;
            Utc.timestamp_opt(secs, nanos).single()
        })
        .unwrap_or_else(Utc::now);

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "uav".to_string(),
        properties: p,
        timestamp,
        latitude: pkt.sensor_latitude_deg,
        longitude: pkt.sensor_longitude_deg,
    }
}

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum KlvTransport {
    UdpRaw,
    UdpMpegTs,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KlvUrl {
    pub transport: KlvTransport,
    pub bind_addr: String,
}

pub fn parse_klv_url(url: &str) -> Result<KlvUrl, ConnectorError> {
    let cfg = |m: String| ConnectorError::ConfigError(m);
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| cfg(format!("KLV: URL missing scheme separator: {}", url)))?;
    let transport = match scheme {
        "klv" => KlvTransport::UdpRaw,
        "klv-ts" => KlvTransport::UdpMpegTs,
        other => {
            return Err(cfg(format!(
                "KLV: unsupported scheme '{}', expected klv:// or klv-ts://",
                other
            )))
        }
    };
    if rest.is_empty() {
        return Err(cfg("KLV: URL missing host:port".into()));
    }
    if !rest.contains(':') {
        return Err(cfg(format!("KLV: URL missing port: {}", url)));
    }
    Ok(KlvUrl {
        transport,
        bind_addr: rest.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct KlvConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl KlvConnector {
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
impl Connector for KlvConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        let url = self.config.url.as_deref().ok_or_else(|| {
            ConnectorError::ConfigError("KLV: url required (klv://host:port)".into())
        })?;
        let parsed = parse_klv_url(url)?;
        let socket = tokio::net::UdpSocket::bind(&parsed.bind_addr)
            .await
            .map_err(|e| {
                ConnectorError::ConnectionError(format!(
                    "KLV: cannot bind UDP {}: {}",
                    parsed.bind_addr, e
                ))
            })?;
        tracing::info!(
            connector_id = %self.config.connector_id,
            bind_addr = %parsed.bind_addr,
            transport = ?parsed.transport,
            "KLV connector listening"
        );
        self.running.store(true, Ordering::SeqCst);

        let running = Arc::clone(&self.running);
        let events_count = Arc::clone(&self.events_count);
        let errors_count = Arc::clone(&self.errors_count);
        let connector_id = self.config.connector_id.clone();
        let transport = parsed.transport;

        tokio::spawn(async move {
            let mut buf = vec![0u8; 65535];
            while running.load(Ordering::SeqCst) {
                let recv = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    socket.recv_from(&mut buf),
                )
                .await;
                let (n, peer) = match recv {
                    Ok(Ok((n, peer))) => (n, peer),
                    Ok(Err(e)) => {
                        tracing::warn!("KLV UDP recv error: {}", e);
                        errors_count.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    Err(_) => continue, // timeout — re-check `running`
                };
                let payload: &[u8] = match transport {
                    KlvTransport::UdpRaw => &buf[..n],
                    KlvTransport::UdpMpegTs => {
                        tracing::warn!(
                            "KLV: klv-ts:// MPEG-TS framing not implemented (TODO); drop"
                        );
                        errors_count.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                let (packets, parse_result) = parse_all_sets(payload);
                if let Err(e) = parse_result {
                    tracing::debug!("KLV parse error from {}: {}", peer, e);
                    errors_count.fetch_add(1, Ordering::Relaxed);
                }
                for pkt in &packets {
                    let ev = packet_to_source_event(pkt, &connector_id, &peer.to_string());
                    if tx.send(ev).await.is_err() {
                        return;
                    }
                    events_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError("KLV: not running".into()))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
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

    fn cfg(id: &str, url: Option<&str>) -> ConnectorConfig {
        ConnectorConfig {
            connector_id: id.into(),
            connector_type: "klv".into(),
            url: url.map(str::to_string),
            entity_type: "uav".into(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        }
    }

    /// BER-encode a length, appending to `out`.
    fn ber_encode_len(out: &mut Vec<u8>, len: usize) {
        if len < 0x80 {
            out.push(len as u8);
            return;
        }
        let bytes = len.to_be_bytes();
        let nz = bytes
            .iter()
            .position(|&b| b != 0)
            .unwrap_or(bytes.len() - 1);
        let n = bytes.len() - nz;
        out.push(0x80 | n as u8);
        out.extend_from_slice(&bytes[nz..]);
    }

    /// Build a syntactically-valid ST 0601 set; appends a Tag 1 checksum if requested.
    fn build_set(entries: &[(u8, Vec<u8>)], with_checksum: bool) -> Vec<u8> {
        let mut body: Vec<u8> = Vec::new();
        for (t, v) in entries {
            body.push(*t);
            ber_encode_len(&mut body, v.len());
            body.extend_from_slice(v);
        }
        let body_total = if with_checksum {
            body.len() + 4
        } else {
            body.len()
        };
        let mut packet = Vec::new();
        packet.extend_from_slice(&ST0601_UNIVERSAL_KEY);
        ber_encode_len(&mut packet, body_total);
        packet.extend_from_slice(&body);
        if with_checksum {
            packet.push(tag::CHECKSUM);
            packet.push(2);
            let csum = bsd_checksum_16(&packet);
            packet.extend_from_slice(&csum.to_be_bytes());
        }
        packet
    }

    // 1. URL parse happy path.
    #[test]
    fn test_url_parse_happy_path() {
        let u = parse_klv_url("klv://0.0.0.0:8000").unwrap();
        assert_eq!(u.transport, KlvTransport::UdpRaw);
        assert_eq!(u.bind_addr, "0.0.0.0:8000");

        let u = parse_klv_url("klv-ts://10.0.0.1:5005").unwrap();
        assert_eq!(u.transport, KlvTransport::UdpMpegTs);
        assert_eq!(u.bind_addr, "10.0.0.1:5005");
    }

    // 2. Bad scheme → ConfigError.
    #[test]
    fn test_url_parse_bad_scheme() {
        let err = parse_klv_url("tcp://0.0.0.0:8000").unwrap_err();
        assert!(matches!(err, ConnectorError::ConfigError(ref m) if m.contains("scheme")));
        assert!(matches!(
            parse_klv_url("not-a-url"),
            Err(ConnectorError::ConfigError(_))
        ));
        assert!(matches!(
            parse_klv_url("klv://"),
            Err(ConnectorError::ConfigError(_))
        ));
        assert!(matches!(
            parse_klv_url("klv://0.0.0.0"),
            Err(ConnectorError::ConfigError(_))
        ));
    }

    // 3. Universal Key: 16-byte key match → continue, mismatch → drop.
    #[test]
    fn test_universal_key_match_and_mismatch() {
        let set = build_set(&[(tag::PLATFORM_TAIL_NUMBER, b"MQ-9".to_vec())], false);
        assert!(parse_st0601_set(&set).is_ok());

        let mut bad = set.clone();
        bad[0] ^= 0xFF;
        let err = parse_st0601_set(&bad).unwrap_err();
        assert!(matches!(err, ConnectorError::ParseError(ref m) if m.contains("universal key")));

        assert!(matches!(
            parse_st0601_set(&[0x06, 0x0E]),
            Err(ConnectorError::ParseError(_))
        ));
    }

    // 4. BER short-form length (1 byte, 0..127).
    #[test]
    fn test_ber_short_form() {
        for v in [0u8, 1, 16, 127] {
            let (len, used) = parse_ber_length(&[v]).unwrap();
            assert_eq!(len, v as usize);
            assert_eq!(used, 1);
        }
    }

    // 5. BER long-form length (1+N bytes, N=1, 2, or 4).
    #[test]
    fn test_ber_long_form() {
        let (len, used) = parse_ber_length(&[0x81, 0xC8]).unwrap();
        assert_eq!((len, used), (200, 2));
        let (len, used) = parse_ber_length(&[0x82, 0x01, 0x00]).unwrap();
        assert_eq!((len, used), (256, 3));
        let (len, used) = parse_ber_length(&[0x84, 0x00, 0x01, 0x00, 0x00]).unwrap();
        assert_eq!((len, used), (65536, 5));

        assert!(parse_ber_length(&[0x82, 0x01]).is_err()); // truncated
        assert!(parse_ber_length(&[]).is_err()); // empty
        assert!(parse_ber_length(&[0x80]).is_err()); // indefinite
        assert!(parse_ber_length(&[0x89]).is_err()); // n > 8
    }

    // 6. Tag 13/14 lat/lon decode.
    #[test]
    fn test_tag_13_14_lat_lon_decode() {
        // 0x00C0_0000 ≈ 0.527° (close to "0.5° approx" per spec example).
        let near = 0x00C0_0000_i32;
        let lat = decode_lat_int32(&near.to_be_bytes()).unwrap();
        assert!((lat - 0.527).abs() < 0.01, "lat={}", lat);

        // 45° latitude round-trip.
        let lat45 = (45.0 * (i32::MAX as f64) / 90.0) as i32;
        assert!((decode_lat_int32(&lat45.to_be_bytes()).unwrap() - 45.0).abs() < 1e-3);

        // -120° longitude.
        let neg120 = (-120.0 * (i32::MAX as f64) / 180.0) as i32;
        assert!((decode_lon_int32(&neg120.to_be_bytes()).unwrap() + 120.0).abs() < 1e-3);

        // Sentinel 0x80000000 → None.
        assert_eq!(decode_lat_int32(&i32::MIN.to_be_bytes()), None);
        assert_eq!(decode_lon_int32(&i32::MIN.to_be_bytes()), None);

        // Wrong length → None.
        assert_eq!(decode_lat_int32(&[0u8, 0]), None);
    }

    // 7. Tag 5 heading uint16 → degrees mapping (full-scale → 360°).
    #[test]
    fn test_tag_5_heading_full_scale() {
        assert!(decode_heading_uint16(&[0u8, 0]).unwrap().abs() < 1e-9);
        assert!((decode_heading_uint16(&[0xFF, 0xFF]).unwrap() - 360.0).abs() < 1e-9);
        assert!((decode_heading_uint16(&[0x80, 0x00]).unwrap() - 180.0).abs() < 0.01);
        assert_eq!(decode_heading_uint16(&[0xFF]), None);
    }

    // 8. Checksum mismatch → packet rejected, errors_count++, no panic.
    #[test]
    fn test_checksum_mismatch_rejected() {
        let entries = vec![
            (tag::MISSION_ID, b"MISSION-1".to_vec()),
            (tag::PLATFORM_TAIL_NUMBER, b"AF-99".to_vec()),
        ];
        let mut packet = build_set(&entries, true);
        let n = packet.len();
        packet[n - 1] ^= 0xFF;
        packet[n - 2] ^= 0xFF;

        let err = parse_st0601_set(&packet).unwrap_err();
        assert!(matches!(err, ConnectorError::ParseError(ref m) if m.contains("checksum")));

        let (pkts, res) = parse_all_sets(&packet);
        assert!(pkts.is_empty());
        assert!(res.is_err());
    }

    // 9. Truncated buffer (LV says 100, only 50 available) → returns Err, no panic.
    #[test]
    fn test_truncated_buffer_returns_error() {
        let mut packet = Vec::new();
        packet.extend_from_slice(&ST0601_UNIVERSAL_KEY);
        packet.push(100); // BER short form claims 100-byte body
        packet.extend_from_slice(&[0u8; 10]);
        assert!(matches!(
            parse_st0601_set(&packet),
            Err(ConnectorError::ParseError(ref m)) if m.contains("exceeds buffer")
        ));

        // Truncated tag-length pair inside body.
        let mut packet = Vec::new();
        packet.extend_from_slice(&ST0601_UNIVERSAL_KEY);
        packet.push(3);
        packet.push(tag::PLATFORM_TAIL_NUMBER);
        packet.push(0x10); // claims 16 bytes, only 1 available
        packet.push(0xAA);
        assert!(matches!(
            parse_st0601_set(&packet),
            Err(ConnectorError::ParseError(_))
        ));
    }

    // 10. Multiple metadata sets in one buffer → all decoded.
    #[test]
    fn test_multiple_sets_in_buffer() {
        let a = build_set(&[(tag::PLATFORM_TAIL_NUMBER, b"AF-1".to_vec())], true);
        let b = build_set(&[(tag::PLATFORM_TAIL_NUMBER, b"AF-2".to_vec())], true);
        let mut combined = Vec::new();
        combined.extend_from_slice(&a);
        combined.extend_from_slice(&b);
        let (pkts, res) = parse_all_sets(&combined);
        assert!(res.is_ok(), "{:?}", res);
        assert_eq!(pkts.len(), 2);
        assert_eq!(pkts[0].platform_tail_number.as_deref(), Some("AF-1"));
        assert_eq!(pkts[1].platform_tail_number.as_deref(), Some("AF-2"));
    }

    // 11. Round-trip with tags 2, 3, 13, 14, 15, 23, 24.
    #[test]
    fn test_roundtrip_known_tags() {
        let pts: u64 = 1_700_000_000_000_000;
        let lat = (51.0 * (i32::MAX as f64) / 90.0) as i32;
        let lon = (-1.0 * (i32::MAX as f64) / 180.0) as i32;
        let alt: u16 = ((1500.0 + 900.0) * 65535.0 / 19900.0) as u16;
        let flat = (51.001 * (i32::MAX as f64) / 90.0) as i32;
        let flon = (-0.999 * (i32::MAX as f64) / 180.0) as i32;
        let entries = vec![
            (tag::PRECISION_TIMESTAMP, pts.to_be_bytes().to_vec()),
            (tag::MISSION_ID, b"OP-CITADEL".to_vec()),
            (tag::SENSOR_LATITUDE, lat.to_be_bytes().to_vec()),
            (tag::SENSOR_LONGITUDE, lon.to_be_bytes().to_vec()),
            (tag::SENSOR_TRUE_ALTITUDE, alt.to_be_bytes().to_vec()),
            (tag::FRAME_CENTRE_LATITUDE, flat.to_be_bytes().to_vec()),
            (tag::FRAME_CENTRE_LONGITUDE, flon.to_be_bytes().to_vec()),
        ];
        let packet = build_set(&entries, true);
        let (pkt, used) = parse_st0601_set(&packet).unwrap();
        assert_eq!(used, packet.len());
        assert_eq!(pkt.precision_timestamp_us, Some(pts));
        assert_eq!(pkt.mission_id.as_deref(), Some("OP-CITADEL"));
        assert!((pkt.sensor_latitude_deg.unwrap() - 51.0).abs() < 1e-3);
        assert!((pkt.sensor_longitude_deg.unwrap() + 1.0).abs() < 1e-3);
        assert!((pkt.sensor_true_altitude_m.unwrap() - 1500.0).abs() < 1.0);
        assert!((pkt.frame_centre_latitude_deg.unwrap() - 51.001).abs() < 1e-3);
        assert!((pkt.frame_centre_longitude_deg.unwrap() + 0.999).abs() < 1e-3);

        let ev = packet_to_source_event(&pkt, "klv-1", "1.2.3.4:5005");
        assert_eq!(ev.entity_type, "uav");
        // mission_id present, no tail → entity_id falls back to "klv-{src_addr}"
        assert_eq!(ev.entity_id, "klv-1.2.3.4:5005");
        assert!((ev.latitude.unwrap() - 51.0).abs() < 1e-3);
        assert!((ev.longitude.unwrap() + 1.0).abs() < 1e-3);
        assert!(ev.properties.contains_key("mission_id"));
        assert!(ev.properties.contains_key("frame_centre_latitude_deg"));
    }

    // ---- additional unit tests ----

    #[test]
    fn test_bsd_checksum_16_known_vector() {
        // [0xAA, 0xBB, 0xCC, 0xDD] → 0xAABB + 0xCC00 = 0x176BB (wrap → 0x76BB)
        // 0x76BB + 0x00DD = 0x7798
        assert_eq!(bsd_checksum_16(&[0xAA, 0xBB, 0xCC, 0xDD]), 0x7798);
        assert_eq!(bsd_checksum_16(&[]), 0);
    }

    #[test]
    fn test_packet_to_source_event_entity_id_priority() {
        let p = St0601Packet {
            mission_id: Some("OP1".into()),
            platform_tail_number: Some("AF-9".into()),
            ..Default::default()
        };
        assert_eq!(
            packet_to_source_event(&p, "k", "1.1.1.1:1").entity_id,
            "OP1-AF-9"
        );

        let p = St0601Packet {
            platform_tail_number: Some("AF-9".into()),
            ..Default::default()
        };
        assert_eq!(
            packet_to_source_event(&p, "k", "1.1.1.1:1").entity_id,
            "klv-AF-9"
        );

        let p = St0601Packet::default();
        assert_eq!(
            packet_to_source_event(&p, "k", "1.1.1.1:1").entity_id,
            "klv-1.1.1.1:1"
        );
    }

    #[test]
    fn test_klv_connector_id() {
        let c = KlvConnector::new(cfg("klv-1", Some("klv://127.0.0.1:0")));
        assert_eq!(c.connector_id(), "klv-1");
    }

    #[tokio::test]
    async fn test_klv_health_check_not_running() {
        let c = KlvConnector::new(cfg("klv-h", Some("klv://127.0.0.1:0")));
        assert!(c.health_check().await.is_err());
    }

    #[tokio::test]
    async fn test_klv_start_invalid_scheme_returns_config_error() {
        let c = KlvConnector::new(cfg("klv-bad", Some("tcp://127.0.0.1:0")));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        assert!(matches!(
            c.start(tx).await,
            Err(ConnectorError::ConfigError(_))
        ));
    }

    #[tokio::test]
    async fn test_klv_start_no_url_returns_config_error() {
        let c = KlvConnector::new(cfg("klv-nourl", None));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        assert!(matches!(
            c.start(tx).await,
            Err(ConnectorError::ConfigError(_))
        ));
    }
}
