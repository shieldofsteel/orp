//! CCSDS Space Packet + TLE/SGP4 satellite tracking adapter.
//!
//! Two ingestion modes:
//! 1. **TLE feed** (`tle+https://...`): polls Celestrak (or any TLE URL)
//!    every `poll_interval_secs` seconds. Each TLE is parsed into
//!    [`sgp4::Elements`], propagated forward to the current epoch via
//!    [`sgp4::Constants::propagate`], and emitted as a [`SourceEvent`]
//!    with lat/lon/alt geometry plus orbital parameters as properties.
//! 2. **CCSDS Space Packet over UDP** (`ccsds-udp://host:port`): binds a
//!    UDP socket and decodes the CCSDS 133.0-B-2 primary header (and
//!    optional secondary header). Each packet becomes a [`SourceEvent`]
//!    keyed by APID with the user-data block base64-encoded as a property.
//!
//! Together CCSDS + SGP4 turn ORP into a passive Space Situational
//! Awareness node — the language of every smallsat operator (Planet,
//! Capella, Spire, BlackSky, USSF SDA).

#![allow(dead_code)]

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use base64::Engine;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// CCSDS primary header fixed length in bytes (CCSDS 133.0-B-2 §4.1.3).
pub const CCSDS_PRIMARY_HEADER_LEN: usize = 6;
/// Default polling cadence for the TLE feed (seconds).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 60;
/// Default secondary-header length when present and not overridden by config.
pub const DEFAULT_SECONDARY_HEADER_LEN: usize = 10;
const EARTH_RADIUS_KM: f64 = 6378.137;
const EARTH_FLATTENING: f64 = 1.0 / 298.257223563;

/// Two ingestion modes the connector understands.
#[derive(Clone, Debug, PartialEq)]
pub enum CcsdsMode {
    /// Poll a TLE feed and propagate via SGP4.
    Tle { url: String },
    /// Listen for CCSDS Space Packets over UDP.
    Ccsds { addr: String },
}

/// Parse the connector URL and return a discriminated [`CcsdsMode`].
///
/// `tle+https://...` and `tle+http://...` map to [`CcsdsMode::Tle`];
/// `ccsds-udp://host:port` maps to [`CcsdsMode::Ccsds`]. Anything else
/// is rejected with [`ConnectorError::ConfigError`].
pub fn parse_url(url: &str) -> Result<CcsdsMode, ConnectorError> {
    if let Some(rest) = url.strip_prefix("tle+") {
        if !rest.starts_with("http://") && !rest.starts_with("https://") {
            return Err(ConnectorError::ConfigError(format!(
                "tle+ URL must wrap http(s)://, got '{rest}'"
            )));
        }
        return Ok(CcsdsMode::Tle {
            url: rest.to_string(),
        });
    }
    if let Some(addr) = url.strip_prefix("ccsds-udp://") {
        if addr.is_empty() {
            return Err(ConnectorError::ConfigError(
                "ccsds-udp:// URL is missing host:port".to_string(),
            ));
        }
        return Ok(CcsdsMode::Ccsds {
            addr: addr.to_string(),
        });
    }
    Err(ConnectorError::ConfigError(format!(
        "unsupported CCSDS URL scheme: '{url}' (expected tle+https:// or ccsds-udp://)"
    )))
}

/// Decoded CCSDS primary header (CCSDS 133.0-B-2 §4.1.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CcsdsPrimaryHeader {
    pub version: u8,
    pub packet_type: u8,
    pub sec_hdr_flag: bool,
    pub apid: u16,
    pub seq_flags: u8,
    pub sequence_count: u16,
    pub packet_length: u16,
}

impl CcsdsPrimaryHeader {
    /// Total bytes of (secondary header + user data) the primary header
    /// claims follows it. CCSDS encodes this as `len - 1`, so we add 1.
    pub fn data_field_length(&self) -> usize {
        self.packet_length as usize + 1
    }

    /// Encode to the wire-format 6 bytes — inverse of [`Self::decode`].
    pub fn encode(&self) -> [u8; CCSDS_PRIMARY_HEADER_LEN] {
        let word0: u16 = ((self.version as u16 & 0x7) << 13)
            | ((self.packet_type as u16 & 0x1) << 12)
            | ((self.sec_hdr_flag as u16) << 11)
            | (self.apid & 0x07FF);
        let word1: u16 = ((self.seq_flags as u16 & 0x3) << 14) | (self.sequence_count & 0x3FFF);
        let mut buf = [0u8; CCSDS_PRIMARY_HEADER_LEN];
        buf[0..2].copy_from_slice(&word0.to_be_bytes());
        buf[2..4].copy_from_slice(&word1.to_be_bytes());
        buf[4..6].copy_from_slice(&self.packet_length.to_be_bytes());
        buf
    }

    /// Decode the 6-byte primary header. Returns [`ConnectorError::ParseError`]
    /// if the buffer is too short — never panics on truncated input.
    pub fn decode(buf: &[u8]) -> Result<Self, ConnectorError> {
        if buf.len() < CCSDS_PRIMARY_HEADER_LEN {
            return Err(ConnectorError::ParseError(format!(
                "CCSDS packet too small: {} < {} bytes",
                buf.len(),
                CCSDS_PRIMARY_HEADER_LEN
            )));
        }
        let word0 = u16::from_be_bytes([buf[0], buf[1]]);
        let word1 = u16::from_be_bytes([buf[2], buf[3]]);
        let word2 = u16::from_be_bytes([buf[4], buf[5]]);
        Ok(Self {
            version: ((word0 >> 13) & 0x7) as u8,
            packet_type: ((word0 >> 12) & 0x1) as u8,
            sec_hdr_flag: ((word0 >> 11) & 0x1) != 0,
            apid: word0 & 0x07FF,
            seq_flags: ((word1 >> 14) & 0x3) as u8,
            sequence_count: word1 & 0x3FFF,
            packet_length: word2,
        })
    }
}

/// Result of decoding a single CCSDS Space Packet.
#[derive(Clone, Debug)]
pub struct CcsdsPacket {
    pub header: CcsdsPrimaryHeader,
    pub secondary_header: Vec<u8>,
    pub user_data: Vec<u8>,
}

/// Decode a complete CCSDS Space Packet from `buf`. `secondary_header_len`
/// is consulted only when the primary header's secondary-header flag is
/// set. If the claimed packet length exceeds the buffer, a parse error is
/// returned (never a panic).
pub fn decode_ccsds_packet(
    buf: &[u8],
    secondary_header_len: usize,
) -> Result<CcsdsPacket, ConnectorError> {
    let header = CcsdsPrimaryHeader::decode(buf)?;
    let claimed = header.data_field_length();
    let total_required = CCSDS_PRIMARY_HEADER_LEN.saturating_add(claimed);
    if buf.len() < total_required {
        return Err(ConnectorError::ParseError(format!(
            "CCSDS packet truncated: claimed {} bytes, only {} available",
            claimed,
            buf.len() - CCSDS_PRIMARY_HEADER_LEN
        )));
    }
    let data_field = &buf[CCSDS_PRIMARY_HEADER_LEN..total_required];
    let (secondary_header, user_data) = if header.sec_hdr_flag {
        if data_field.len() < secondary_header_len {
            return Err(ConnectorError::ParseError(format!(
                "CCSDS secondary header truncated: need {} bytes, have {}",
                secondary_header_len,
                data_field.len()
            )));
        }
        let (sh, ud) = data_field.split_at(secondary_header_len);
        (sh.to_vec(), ud.to_vec())
    } else {
        (Vec::new(), data_field.to_vec())
    };
    Ok(CcsdsPacket {
        header,
        secondary_header,
        user_data,
    })
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn ccsds_packet_to_event(pkt: &CcsdsPacket, connector_id: &str, entity_type: &str) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert(
        "apid".into(),
        serde_json::Value::Number(pkt.header.apid.into()),
    );
    properties.insert(
        "sequence_count".into(),
        serde_json::Value::Number(pkt.header.sequence_count.into()),
    );
    properties.insert(
        "packet_length".into(),
        serde_json::Value::Number(pkt.header.packet_length.into()),
    );
    properties.insert(
        "packet_type".into(),
        serde_json::Value::Number(pkt.header.packet_type.into()),
    );
    properties.insert(
        "sec_hdr_flag".into(),
        serde_json::Value::Bool(pkt.header.sec_hdr_flag),
    );
    if !pkt.secondary_header.is_empty() {
        properties.insert(
            "secondary_header_b64".into(),
            serde_json::Value::String(b64(&pkt.secondary_header)),
        );
    }
    properties.insert(
        "user_data_b64".into(),
        serde_json::Value::String(b64(&pkt.user_data)),
    );
    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id: format!("ccsds-apid-{}", pkt.header.apid),
        entity_type: entity_type.to_string(),
        properties,
        timestamp: Utc::now(),
        latitude: None,
        longitude: None,
    }
}

/// Convert TEME-frame (km) position to geodetic lat/lon/alt. Standard
/// rotation by sidereal time → ECEF, then Bowring iteration on the
/// WGS-84 ellipsoid. Adequate for tracking; sub-degree accuracy.
fn teme_to_geodetic(position_km: [f64; 3], epoch_years_j2000: f64) -> (f64, f64, f64) {
    let theta = sgp4::iau_epoch_to_sidereal_time(epoch_years_j2000);
    let (sin_t, cos_t) = theta.sin_cos();
    let x = cos_t * position_km[0] + sin_t * position_km[1];
    let y = -sin_t * position_km[0] + cos_t * position_km[1];
    let z = position_km[2];
    let a = EARTH_RADIUS_KM;
    let e2 = EARTH_FLATTENING * (2.0 - EARTH_FLATTENING);
    let r = (x * x + y * y).sqrt();
    let lon = y.atan2(x);
    let mut lat = z.atan2(r * (1.0 - e2));
    let mut alt = 0.0;
    for _ in 0..5 {
        let sin_lat = lat.sin();
        let n = a / (1.0 - e2 * sin_lat * sin_lat).sqrt();
        alt = r / lat.cos() - n;
        lat = z.atan2(r * (1.0 - e2 * n / (n + alt)));
    }
    (lat.to_degrees(), lon.to_degrees(), alt)
}

fn insert_f64(props: &mut HashMap<String, serde_json::Value>, key: &str, value: f64) {
    if let Some(n) = serde_json::Number::from_f64(value) {
        props.insert(key.to_string(), serde_json::Value::Number(n));
    }
}

fn tle_to_event(
    elements: &sgp4::Elements,
    prediction: &sgp4::Prediction,
    connector_id: &str,
    entity_type: &str,
) -> SourceEvent {
    let (lat, lon, alt) = teme_to_geodetic(prediction.position, elements.epoch());
    let mut properties = HashMap::new();
    properties.insert(
        "norad_cat_id".into(),
        serde_json::Value::Number(elements.norad_id.into()),
    );
    if let Some(ref name) = elements.object_name {
        properties.insert(
            "object_name".into(),
            serde_json::Value::String(name.clone()),
        );
    }
    if let Some(ref id) = elements.international_designator {
        properties.insert(
            "international_designator".into(),
            serde_json::Value::String(id.clone()),
        );
    }
    insert_f64(&mut properties, "inclination_deg", elements.inclination);
    insert_f64(&mut properties, "eccentricity", elements.eccentricity);
    insert_f64(
        &mut properties,
        "mean_motion_rev_per_day",
        elements.mean_motion,
    );
    insert_f64(
        &mut properties,
        "right_ascension_deg",
        elements.right_ascension,
    );
    insert_f64(
        &mut properties,
        "argument_of_perigee_deg",
        elements.argument_of_perigee,
    );
    insert_f64(&mut properties, "mean_anomaly_deg", elements.mean_anomaly);
    insert_f64(&mut properties, "altitude_km", alt);
    insert_f64(&mut properties, "velocity_x_km_s", prediction.velocity[0]);
    insert_f64(&mut properties, "velocity_y_km_s", prediction.velocity[1]);
    insert_f64(&mut properties, "velocity_z_km_s", prediction.velocity[2]);
    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id: format!("sat-{}", elements.norad_id),
        entity_type: entity_type.to_string(),
        properties,
        timestamp: Utc::now(),
        latitude: Some(lat),
        longitude: Some(lon),
    }
}

/// Parse a Celestrak-style TLE blob: handles 2-line and 3-line
/// (TLE-with-name) formats. Lines starting with "1 " are the first
/// element-set line, "2 " the second; non-empty preceding lines become
/// the satellite name.
pub fn parse_tle_feed(body: &str) -> Result<Vec<sgp4::Elements>, ConnectorError> {
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    let mut line1: Option<String> = None;
    for raw in body.lines() {
        let trimmed = raw.trim_end_matches('\r').trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("1 ") && line1.is_none() {
            line1 = Some(trimmed.to_string());
        } else if trimmed.starts_with("2 ") {
            if let Some(l1) = line1.take() {
                let elem = sgp4::Elements::from_tle(name.take(), l1.as_bytes(), trimmed.as_bytes())
                    .map_err(|e| ConnectorError::ParseError(format!("TLE parse: {e}")))?;
                out.push(elem);
            }
        } else {
            name = Some(trimmed.to_string());
            line1 = None;
        }
    }
    Ok(out)
}

fn propagate_to_now(elements: &sgp4::Elements) -> Result<sgp4::Prediction, ConnectorError> {
    let constants = sgp4::Constants::from_elements(elements)
        .map_err(|e| ConnectorError::ParseError(format!("SGP4 init: {e}")))?;
    let now = Utc::now().naive_utc();
    let minutes = elements
        .datetime_to_minutes_since_epoch(&now)
        .map_err(|e| ConnectorError::ParseError(format!("SGP4 epoch math: {e}")))?;
    constants
        .propagate(minutes)
        .map_err(|e| ConnectorError::ParseError(format!("SGP4 propagate: {e}")))
}

pub struct CcsdsConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl CcsdsConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    fn poll_interval_secs(&self) -> u64 {
        self.config
            .properties
            .get("poll_interval_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECS)
    }

    fn secondary_header_len(&self) -> usize {
        self.config
            .properties
            .get("secondary_header_len")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_SECONDARY_HEADER_LEN)
    }

    async fn run_tle_loop(ctx: RunCtx, url: String, poll_secs: u64) {
        let client = reqwest::Client::new();
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(poll_secs.max(1)));
        while ctx.running.load(Ordering::SeqCst) {
            interval.tick().await;
            let body = match client.get(&url).send().await {
                Ok(r) => match r.text().await {
                    Ok(t) => t,
                    Err(e) => {
                        ctx.bump_err(format!("body read failed: {e}"));
                        continue;
                    }
                },
                Err(e) => {
                    ctx.bump_err(format!("fetch failed: {e}"));
                    continue;
                }
            };
            let elements_vec = match parse_tle_feed(&body) {
                Ok(v) => v,
                Err(e) => {
                    ctx.bump_err(format!("parse failed: {e}"));
                    continue;
                }
            };
            for elements in &elements_vec {
                match propagate_to_now(elements) {
                    Ok(pred) => {
                        let event =
                            tle_to_event(elements, &pred, &ctx.connector_id, &ctx.entity_type);
                        if ctx.tx.send(event).await.is_err() {
                            return;
                        }
                        ctx.events_count.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => ctx.bump_err(format!("SGP4 sat {}: {e}", elements.norad_id)),
                }
            }
        }
    }

    async fn run_ccsds_udp_loop(ctx: RunCtx, addr: String, secondary_header_len: usize) {
        let socket = match tokio::net::UdpSocket::bind(&addr).await {
            Ok(s) => s,
            Err(e) => {
                ctx.bump_err(format!("UDP bind {addr} failed: {e}"));
                return;
            }
        };
        tracing::info!("CCSDS connector listening on UDP {}", addr);
        let mut buf = vec![0u8; 65535];
        while ctx.running.load(Ordering::SeqCst) {
            match socket.recv_from(&mut buf).await {
                Ok((n, _)) => match decode_ccsds_packet(&buf[..n], secondary_header_len) {
                    Ok(pkt) => {
                        let event =
                            ccsds_packet_to_event(&pkt, &ctx.connector_id, &ctx.entity_type);
                        if ctx.tx.send(event).await.is_err() {
                            return;
                        }
                        ctx.events_count.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => ctx.bump_err(format!("decode: {e}")),
                },
                Err(e) => ctx.bump_err(format!("UDP recv: {e}")),
            }
        }
    }
}

/// Shared state passed to each background loop. Lets the loops bump
/// counters and emit events without copying many `Arc`s as separate
/// arguments.
#[derive(Clone)]
struct RunCtx {
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    tx: tokio::sync::mpsc::Sender<SourceEvent>,
    connector_id: String,
    entity_type: String,
}

impl RunCtx {
    fn bump_err(&self, msg: String) {
        tracing::warn!("CCSDS: {}", msg);
        self.errors_count.fetch_add(1, Ordering::Relaxed);
    }
}

#[async_trait]
impl Connector for CcsdsConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        let url =
            self.config.url.clone().ok_or_else(|| {
                ConnectorError::ConfigError("CCSDS connector requires a URL".into())
            })?;
        let mode = parse_url(&url)?;
        self.running.store(true, Ordering::SeqCst);
        let ctx = RunCtx {
            running: self.running.clone(),
            events_count: self.events_count.clone(),
            errors_count: self.errors_count.clone(),
            tx,
            connector_id: self.config.connector_id.clone(),
            entity_type: self.config.entity_type.clone(),
        };
        match mode {
            CcsdsMode::Tle { url } => {
                let poll_secs = self.poll_interval_secs();
                tokio::spawn(Self::run_tle_loop(ctx, url, poll_secs));
            }
            CcsdsMode::Ccsds { addr } => {
                let sh_len = self.secondary_header_len();
                tokio::spawn(Self::run_ccsds_udp_loop(ctx, addr, sh_len));
            }
        }
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
            Err(ConnectorError::ConnectionError("not running".into()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str) -> ConnectorConfig {
        ConnectorConfig {
            connector_id: "ccsds-test".into(),
            connector_type: "ccsds".into(),
            url: Some(url.to_string()),
            entity_type: "satellite".into(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        }
    }

    fn iss_tle() -> (&'static str, &'static str, &'static str) {
        (
            "ISS (ZARYA)",
            "1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992",
            "2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008",
        )
    }

    #[test]
    fn test_parse_url_tle_https() {
        assert_eq!(
            parse_url("tle+https://celestrak.org/x.txt").expect("ok"),
            CcsdsMode::Tle {
                url: "https://celestrak.org/x.txt".into()
            }
        );
    }

    #[test]
    fn test_parse_url_tle_http() {
        assert_eq!(
            parse_url("tle+http://example.com/tle").expect("ok"),
            CcsdsMode::Tle {
                url: "http://example.com/tle".into()
            }
        );
    }

    #[test]
    fn test_parse_url_ccsds_udp() {
        assert_eq!(
            parse_url("ccsds-udp://0.0.0.0:14000").expect("ok"),
            CcsdsMode::Ccsds {
                addr: "0.0.0.0:14000".into()
            }
        );
    }

    #[test]
    fn test_parse_url_bad_scheme() {
        assert!(matches!(
            parse_url("kafka://nope"),
            Err(ConnectorError::ConfigError(_))
        ));
        assert!(matches!(
            parse_url("tle+ftp://nope.com"),
            Err(ConnectorError::ConfigError(_))
        ));
        assert!(matches!(
            parse_url("ccsds-udp://"),
            Err(ConnectorError::ConfigError(_))
        ));
    }

    #[test]
    fn test_ccsds_primary_header_roundtrip() {
        let h = CcsdsPrimaryHeader {
            version: 0,
            packet_type: 1,
            sec_hdr_flag: false,
            apid: 100,
            seq_flags: 0b11,
            sequence_count: 42,
            packet_length: 128,
        };
        let bytes = h.encode();
        assert_eq!(bytes.len(), 6);
        let decoded = CcsdsPrimaryHeader::decode(&bytes).expect("decode");
        assert_eq!(decoded, h);
        assert_eq!(decoded.apid, 100);
        assert_eq!(decoded.sequence_count, 42);
        assert_eq!(decoded.packet_length, 128);
    }

    #[test]
    fn test_ccsds_secondary_header_flag_respected() {
        let h_no = CcsdsPrimaryHeader {
            version: 0,
            packet_type: 0,
            sec_hdr_flag: false,
            apid: 7,
            seq_flags: 0b11,
            sequence_count: 1,
            packet_length: 3,
        };
        let mut buf = Vec::from(h_no.encode());
        buf.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        let pkt = decode_ccsds_packet(&buf, 4).expect("decode");
        assert!(pkt.secondary_header.is_empty());
        assert_eq!(pkt.user_data, vec![0xAA, 0xBB, 0xCC, 0xDD]);

        let h_yes = CcsdsPrimaryHeader {
            sec_hdr_flag: true,
            packet_length: 7,
            ..h_no
        };
        let mut buf = Vec::from(h_yes.encode());
        buf.extend_from_slice(&[1, 2, 3, 4, 0xAA, 0xBB, 0xCC, 0xDD]);
        let pkt = decode_ccsds_packet(&buf, 4).expect("decode");
        assert_eq!(pkt.secondary_header, vec![1, 2, 3, 4]);
        assert_eq!(pkt.user_data, vec![0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn test_ccsds_packet_smaller_than_primary_header() {
        let buf = [0u8; 3];
        assert!(matches!(
            decode_ccsds_packet(&buf, 0),
            Err(ConnectorError::ParseError(_))
        ));
        assert!(matches!(
            CcsdsPrimaryHeader::decode(&buf),
            Err(ConnectorError::ParseError(_))
        ));
    }

    #[test]
    fn test_ccsds_claimed_length_exceeds_buffer() {
        let h = CcsdsPrimaryHeader {
            version: 0,
            packet_type: 0,
            sec_hdr_flag: false,
            apid: 1,
            seq_flags: 0,
            sequence_count: 0,
            packet_length: 999,
        };
        assert!(matches!(
            decode_ccsds_packet(&h.encode(), 0),
            Err(ConnectorError::ParseError(_))
        ));
    }

    #[test]
    fn test_tle_round_trip_at_epoch() {
        let (name, l1, l2) = iss_tle();
        let elements = sgp4::Elements::from_tle(Some(name.into()), l1.as_bytes(), l2.as_bytes())
            .expect("parse TLE");
        assert_eq!(elements.norad_id, 25544);
        let constants = sgp4::Constants::from_elements(&elements).expect("constants");
        let pred = constants
            .propagate(sgp4::MinutesSinceEpoch(0.0))
            .expect("propagate");
        let (lat, lon, _) = teme_to_geodetic(pred.position, elements.epoch());
        // ISS inclination 51.6° — lat must lie within that band.
        assert!(lat.abs() < 60.0, "ISS lat {lat} not within band");
        assert!((-180.0..=180.0).contains(&lon));
        let r =
            (pred.position[0].powi(2) + pred.position[1].powi(2) + pred.position[2].powi(2)).sqrt();
        assert!((6700.0..7100.0).contains(&r), "ISS radius {r} km not LEO");
    }

    #[test]
    fn test_tle_parse_multiple_separated_by_newlines() {
        let (name, l1, l2) = iss_tle();
        let body = format!(
            "{name}\n{l1}\n{l2}\n\n\
             NOAA 19\n\
             1 33591U 09005A   20194.55000000  .00000049  00000-0  53889-4 0  9991\n\
             2 33591  99.1900 200.0000 0014000  60.0000 300.0000 14.12000000  1231\n"
        );
        let parsed = parse_tle_feed(&body).expect("parse_tle_feed");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].norad_id, 25544);
        assert_eq!(parsed[1].norad_id, 33591);
    }

    #[test]
    fn test_tle_parse_2le_no_names() {
        let (_, l1, l2) = iss_tle();
        let body = format!("{l1}\n{l2}\n");
        let parsed = parse_tle_feed(&body).expect("parse 2le");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].norad_id, 25544);
        assert!(parsed[0].object_name.is_none());
    }

    #[test]
    fn test_sgp4_propagation_stale_tle_does_not_panic() {
        let (name, l1, l2) = iss_tle();
        let elements = sgp4::Elements::from_tle(Some(name.into()), l1.as_bytes(), l2.as_bytes())
            .expect("parse TLE");
        // Propagate to "now" (years past epoch). Must not panic.
        if let Ok(pred) = propagate_to_now(&elements) {
            let r =
                (pred.position[0].powi(2) + pred.position[1].powi(2) + pred.position[2].powi(2))
                    .sqrt();
            assert!(r.is_finite(), "stale TLE produced non-finite radius");
        }
    }

    #[tokio::test]
    async fn test_ccsds_polling_tick_increments_events_count() {
        // Bind ephemeral, free it, hand the addr to the connector; send
        // one synthetic packet; verify events_count == 1.
        let listen = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listen.local_addr().expect("addr");
        drop(listen);

        let connector = CcsdsConnector::new(cfg(&format!("ccsds-udp://{addr}")));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        connector.start(tx).await.expect("start");
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        let h = CcsdsPrimaryHeader {
            version: 0,
            packet_type: 0,
            sec_hdr_flag: false,
            apid: 100,
            seq_flags: 0b11,
            sequence_count: 7,
            packet_length: 3,
        };
        let mut packet = Vec::from(h.encode());
        packet.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind");
        sender.send_to(&packet, addr).await.expect("send");

        let event = tokio::time::timeout(tokio::time::Duration::from_secs(3), rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        assert_eq!(event.entity_id, "ccsds-apid-100");
        assert_eq!(event.entity_type, "satellite");
        assert_eq!(connector.stats().events_processed, 1);
        connector.stop().await.expect("stop");
    }

    #[test]
    fn test_ccsds_packet_to_event_includes_b64() {
        let h = CcsdsPrimaryHeader {
            version: 0,
            packet_type: 1,
            sec_hdr_flag: false,
            apid: 256,
            seq_flags: 0,
            sequence_count: 0,
            packet_length: 3,
        };
        let pkt = CcsdsPacket {
            header: h,
            secondary_header: Vec::new(),
            user_data: vec![0x01, 0x02, 0x03, 0x04],
        };
        let evt = ccsds_packet_to_event(&pkt, "test", "satellite");
        assert_eq!(evt.entity_id, "ccsds-apid-256");
        let b64 = evt
            .properties
            .get("user_data_b64")
            .and_then(|v| v.as_str())
            .expect("b64");
        assert_eq!(b64, "AQIDBA==");
        assert_eq!(
            evt.properties.get("apid").and_then(|v| v.as_u64()),
            Some(256)
        );
    }

    #[test]
    fn test_health_check_states() {
        let connector = CcsdsConnector::new(cfg("ccsds-udp://127.0.0.1:0"));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async {
            assert!(connector.health_check().await.is_err());
            connector.running.store(true, Ordering::SeqCst);
            assert!(connector.health_check().await.is_ok());
            connector.stop().await.expect("stop");
        });
    }

    #[test]
    fn test_start_rejects_missing_url() {
        let mut config = cfg("ccsds-udp://127.0.0.1:0");
        config.url = None;
        let connector = CcsdsConnector::new(config);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async {
            let (tx, _rx) = tokio::sync::mpsc::channel(1);
            assert!(matches!(
                connector.start(tx).await,
                Err(ConnectorError::ConfigError(_))
            ));
        });
    }
}
