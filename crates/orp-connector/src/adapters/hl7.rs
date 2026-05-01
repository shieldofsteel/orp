//! HL7 v2.5 connector — MLLP (Minimal Lower Layer Protocol) over TCP.
//!
//! Healthcare facilities, hospital ships, military medical evac, and
//! mass-casualty triage tents all speak HL7 v2.x. ORP terminates an
//! MLLP listener and turns each parsed HL7 message into a `SourceEvent`.
//!
//! Wire format: each message is wrapped between SB (`\x0B`) and EB+CR
//! (`\x1C\r`); segments use `\r`, fields `|`, components `^`,
//! repetitions `~`, sub-components `&` per MSH-2.
//!
//! PHI safety: full message bodies are only emitted at `tracing::debug!`;
//! `tracing::info!` is reserved for counts and lifecycle events.

#![allow(dead_code)]

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const MLLP_SB: u8 = 0x0B;
const MLLP_EB: u8 = 0x1C;
const MLLP_CR: u8 = 0x0D;
const DEFAULT_BIND: &str = "0.0.0.0:2575";
const READ_BUFFER_SIZE: usize = 64 * 1024;
const MAX_BUFFER: usize = 4 * 1024 * 1024;

/// Outcome of attempting to extract a single MLLP frame.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameOutcome {
    /// Complete frame body (without SB/EB) and bytes consumed from input.
    Complete(Vec<u8>, usize),
    /// No complete frame yet — caller should read more bytes.
    Incomplete,
    /// Garbage before SB; bytes to discard from front of buffer.
    Resync(usize),
}

/// Try to extract one MLLP frame from `buf`.
pub fn extract_mllp_frame(buf: &[u8]) -> FrameOutcome {
    let sb_pos = match buf.iter().position(|&b| b == MLLP_SB) {
        Some(p) => p,
        None => {
            return if buf.is_empty() {
                FrameOutcome::Incomplete
            } else {
                FrameOutcome::Resync(buf.len())
            };
        }
    };
    if sb_pos > 0 {
        return FrameOutcome::Resync(sb_pos);
    }
    let after_sb = &buf[1..];
    let eb_rel = match after_sb.iter().position(|&b| b == MLLP_EB) {
        Some(p) => p,
        None => return FrameOutcome::Incomplete,
    };
    let eb_abs = 1 + eb_rel;
    let mut consumed = eb_abs + 1;
    if buf.get(eb_abs + 1) == Some(&MLLP_CR) {
        consumed += 1;
    }
    FrameOutcome::Complete(buf[1..eb_abs].to_vec(), consumed)
}

/// Extract every complete MLLP frame from `buf`; returns frames + bytes consumed.
pub fn extract_all_mllp_frames(buf: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let mut frames = Vec::new();
    let mut cursor = 0;
    loop {
        match extract_mllp_frame(&buf[cursor..]) {
            FrameOutcome::Complete(body, n) => {
                frames.push(body);
                cursor += n;
            }
            FrameOutcome::Resync(n) => cursor += n,
            FrameOutcome::Incomplete => break,
        }
    }
    (frames, cursor)
}

/// Wrap an HL7 message body in MLLP framing for transmission.
pub fn wrap_mllp(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 3);
    out.push(MLLP_SB);
    out.extend_from_slice(body);
    out.push(MLLP_EB);
    out.push(MLLP_CR);
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct EncodingChars {
    pub field_sep: char,
    pub component_sep: char,
    pub repetition_sep: char,
    pub escape_char: char,
    pub subcomponent_sep: char,
}

impl Default for EncodingChars {
    fn default() -> Self {
        Self {
            field_sep: '|',
            component_sep: '^',
            repetition_sep: '~',
            escape_char: '\\',
            subcomponent_sep: '&',
        }
    }
}

/// One parsed HL7 segment. `fields[0]` is HL7 field 1 (zero-based storage,
/// one-based access via [`Segment::field`]). For MSH only, `fields[0]` is
/// the field separator (MSH-1) and `fields[1]` is the encoding chars
/// (MSH-2), so MSH-3 onwards line up at index 2+.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub name: String,
    pub fields: Vec<String>,
}

impl Segment {
    /// One-based field access. Returns `""` if missing.
    pub fn field(&self, idx: usize) -> &str {
        if idx == 0 {
            return "";
        }
        self.fields.get(idx - 1).map(String::as_str).unwrap_or("")
    }
}

#[derive(Clone, Debug)]
pub struct Hl7Message {
    pub encoding: EncodingChars,
    pub segments: Vec<Segment>,
}

impl Hl7Message {
    pub fn first_segment(&self, name: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.name == name)
    }

    pub fn segments_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Segment> + 'a {
        self.segments.iter().filter(move |s| s.name == name)
    }
}

/// Parse an HL7 message body (post-MLLP).
pub fn parse_hl7(body: &[u8]) -> Result<Hl7Message, ConnectorError> {
    if body.len() < 8 {
        return Err(ConnectorError::ParseError("HL7 message too short".into()));
    }
    if !body.starts_with(b"MSH") {
        return Err(ConnectorError::ParseError(
            "HL7 message must begin with MSH".into(),
        ));
    }
    let text = std::str::from_utf8(body)
        .map_err(|e| ConnectorError::ParseError(format!("non-UTF8 HL7 body: {e}")))?;

    // MSH-1 = char at byte 3 (field separator); MSH-2 = next 4 chars.
    let mut chars_iter = text.chars();
    chars_iter.next();
    chars_iter.next();
    chars_iter.next();
    let field_sep = chars_iter
        .next()
        .ok_or_else(|| ConnectorError::ParseError("missing MSH-1 separator".into()))?;
    let encoding = EncodingChars {
        field_sep,
        component_sep: chars_iter.next().unwrap_or('^'),
        repetition_sep: chars_iter.next().unwrap_or('~'),
        escape_char: chars_iter.next().unwrap_or('\\'),
        subcomponent_sep: chars_iter.next().unwrap_or('&'),
    };

    let segments: Vec<Segment> = text
        .split(['\r', '\n'])
        .filter(|s| !s.is_empty())
        .map(|seg_str| split_segment(seg_str, &encoding))
        .collect::<Result<Vec<_>, _>>()?;

    if segments.is_empty() {
        return Err(ConnectorError::ParseError(
            "HL7 message contained no segments".into(),
        ));
    }
    Ok(Hl7Message { encoding, segments })
}

fn split_segment(seg_str: &str, enc: &EncodingChars) -> Result<Segment, ConnectorError> {
    let mut parts = seg_str.splitn(2, enc.field_sep);
    let name = parts
        .next()
        .ok_or_else(|| ConnectorError::ParseError("empty segment".into()))?
        .to_string();
    if name.len() < 2 {
        return Err(ConnectorError::ParseError(format!(
            "invalid segment name: {name:?}"
        )));
    }
    let rest = parts.next();
    let fields = match name.as_str() {
        "MSH" => {
            // MSH-1 = field separator itself, MSH-2 = encoding chars.
            let mut v = vec![enc.field_sep.to_string()];
            let r =
                rest.ok_or_else(|| ConnectorError::ParseError("MSH segment has no fields".into()))?;
            v.extend(r.split(enc.field_sep).map(str::to_string));
            v
        }
        _ => {
            let r = rest.ok_or_else(|| {
                ConnectorError::ParseError(format!("segment {name} missing field separator"))
            })?;
            r.split(enc.field_sep).map(str::to_string).collect()
        }
    };
    Ok(Segment { name, fields })
}

/// Decode HL7 datetime in YYYYMMDDHHMMSS[.ffff][+/-ZZZZ] format.
fn parse_hl7_ts(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let core: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let date_part = if let Some(dot) = core.find('.') {
        &core[..dot.min(14)]
    } else if core.len() >= 14 {
        &core[..14]
    } else {
        &core[..]
    };
    let len = date_part.len();
    let parsed = if len >= 14 {
        NaiveDateTime::parse_from_str(&date_part[..14], "%Y%m%d%H%M%S").ok()
    } else if len >= 12 {
        NaiveDateTime::parse_from_str(&format!("{}00", &date_part[..12]), "%Y%m%d%H%M%S").ok()
    } else if len >= 8 {
        NaiveDate::parse_from_str(&date_part[..8], "%Y%m%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
    } else {
        None
    };
    parsed.and_then(|ndt| Utc.from_local_datetime(&ndt).single())
}

fn insert_str(props: &mut HashMap<String, JsonValue>, key: &str, val: &str) {
    if !val.is_empty() {
        props.insert(key.to_string(), JsonValue::String(val.to_string()));
    }
}

/// Convert one parsed HL7 message into a `SourceEvent`.
pub fn message_to_event(msg: &Hl7Message, connector_id: &str) -> SourceEvent {
    let comp_sep = msg.encoding.component_sep;
    let rep_sep = msg.encoding.repetition_sep;
    let mut props: HashMap<String, JsonValue> = HashMap::new();
    let mut entity_id: Option<String> = None;
    let mut entity_type = "observation";
    let mut event_ts: Option<DateTime<Utc>> = None;
    // MSH: sending app/facility, message type, control id, timestamp.
    if let Some(msh) = msg.first_segment("MSH") {
        insert_str(&mut props, "msh.sending_app", msh.field(3));
        insert_str(&mut props, "msh.sending_facility", msh.field(4));
        insert_str(&mut props, "msh.message_type", msh.field(9));
        insert_str(&mut props, "msh.control_id", msh.field(10));
        let ts_raw = msh.field(7);
        if !ts_raw.is_empty() {
            insert_str(&mut props, "msh.timestamp", ts_raw);
            event_ts = parse_hl7_ts(ts_raw);
        }
    }
    if let Some(evn) = msg.first_segment("EVN") {
        insert_str(&mut props, "evn.trigger", evn.field(1));
        insert_str(&mut props, "evn.recorded", evn.field(2));
    }
    if let Some(pid) = msg.first_segment("PID") {
        entity_type = "patient";
        // PID-3: first repetition's first component.
        let pid3 = pid.field(3);
        let patient_id = pid3
            .split(rep_sep)
            .next()
            .unwrap_or(pid3)
            .split(comp_sep)
            .next()
            .unwrap_or("")
            .to_string();
        if !patient_id.is_empty() {
            entity_id = Some(patient_id.clone());
            props.insert("pid.patient_id".into(), JsonValue::String(patient_id));
        }
        // PID-5 family^given.
        let name_raw = pid.field(5);
        if !name_raw.is_empty() {
            let mut c = name_raw.split(comp_sep);
            let family = c.next().unwrap_or("").trim();
            let given = c.next().unwrap_or("").trim();
            let name = match (family.is_empty(), given.is_empty()) {
                (false, false) => format!("{given} {family}"),
                (false, true) => family.to_string(),
                (true, false) => given.to_string(),
                _ => String::new(),
            };
            insert_str(&mut props, "pid.name", &name);
        }
        insert_str(&mut props, "pid.dob", pid.field(7));
        insert_str(&mut props, "pid.sex", pid.field(8));
    }
    if let Some(pv1) = msg.first_segment("PV1") {
        insert_str(&mut props, "pv1.location", pv1.field(3));
        insert_str(&mut props, "pv1.attending_doctor", pv1.field(7));
    }
    if let Some(obr) = msg.first_segment("OBR") {
        let test_id = obr.field(4);
        insert_str(&mut props, "obr.test_id", test_id);
        let obs_dt = obr.field(7);
        if !obs_dt.is_empty() {
            insert_str(&mut props, "obr.observation_datetime", obs_dt);
            if event_ts.is_none() {
                event_ts = parse_hl7_ts(obs_dt);
            }
        }
        if entity_id.is_none() && !test_id.is_empty() {
            entity_id = Some(
                test_id
                    .split(comp_sep)
                    .next()
                    .unwrap_or(test_id)
                    .to_string(),
            );
        }
    }
    // OBX: one entry per result, aggregated under obx.<observation_identifier>.
    for obx in msg.segments_named("OBX") {
        let obs_ident = obx.field(3).split(comp_sep).next().unwrap_or("");
        if obs_ident.is_empty() {
            continue;
        }
        let mut entry = serde_json::Map::new();
        for (k, v) in [
            ("value", obx.field(5)),
            ("units", obx.field(6)),
            ("abnormal", obx.field(8)),
            ("status", obx.field(11)),
        ] {
            if !v.is_empty() {
                entry.insert(k.into(), JsonValue::String(v.to_string()));
            }
        }
        props.insert(format!("obx.{obs_ident}"), JsonValue::Object(entry));
    }
    let final_entity_id = entity_id.unwrap_or_else(|| {
        format!(
            "hl7-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        )
    });
    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id: final_entity_id,
        entity_type: entity_type.to_string(),
        properties: props,
        timestamp: event_ts.unwrap_or_else(Utc::now),
        latitude: None,
        longitude: None,
    }
}

/// Build an MLLP-framed ACK for an inbound HL7 message.
pub fn build_ack(msg: &Hl7Message, ack_code: &str) -> Vec<u8> {
    wrap_mllp(build_ack_body(msg, ack_code).as_bytes())
}

/// Build the ACK MSH+MSA body (without MLLP framing).
pub fn build_ack_body(msg: &Hl7Message, ack_code: &str) -> String {
    let enc = &msg.encoding;
    let fs = enc.field_sep;
    let msh = msg.first_segment("MSH");
    let in_app = msh.map(|m| m.field(3)).unwrap_or("");
    let in_fac = msh.map(|m| m.field(4)).unwrap_or("");
    let in_ctl = msh.map(|m| m.field(10)).unwrap_or("");
    let proc_id = msh.map(|m| m.field(11)).unwrap_or("P");
    let ver = msh.map(|m| m.field(12)).unwrap_or("2.5");
    let now = Utc::now().format("%Y%m%d%H%M%S").to_string();
    let new_ctl = format!("ACK{}", Utc::now().timestamp_millis());
    let ec = format!(
        "{}{}{}{}",
        enc.component_sep, enc.repetition_sep, enc.escape_char, enc.subcomponent_sep
    );
    // ACK MSH: swap sending/receiving (we are ORP), set MSH-9 to ACK.
    format!(
        "MSH{fs}{ec}{fs}ORP{fs}ORP{fs}{in_app}{fs}{in_fac}{fs}{now}{fs}{fs}ACK{fs}{new_ctl}{fs}{proc_id}{fs}{ver}\rMSA{fs}{ack_code}{fs}{in_ctl}\r",
    )
}

/// Parse a `mllp://host:port` URL into a SocketAddr.
pub fn parse_mllp_url(url: &str) -> Result<SocketAddr, ConnectorError> {
    let rest = url.strip_prefix("mllp://").ok_or_else(|| {
        ConnectorError::ConfigError(format!(
            "hl7 connector URL must use mllp:// scheme, got: {url}"
        ))
    })?;
    if rest.is_empty() {
        return Err(ConnectorError::ConfigError(
            "hl7 mllp:// URL is missing host:port".into(),
        ));
    }
    rest.parse::<SocketAddr>()
        .map_err(|e| ConnectorError::ConfigError(format!("invalid hl7 bind address {rest}: {e}")))
}

/// HL7 v2.5 over MLLP connector. Listens on TCP (default port 2575),
/// reads MLLP-framed messages, parses each into a `SourceEvent`, and
/// replies with an MLLP-framed ACK.
pub struct Hl7Connector {
    config: ConnectorConfig,
    bind_addr: SocketAddr,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl Hl7Connector {
    pub fn new(config: ConnectorConfig) -> Result<Self, ConnectorError> {
        let bind_addr = match config.url.as_deref() {
            Some(u) if u.starts_with("mllp://") => parse_mllp_url(u)?,
            Some(u) => {
                return Err(ConnectorError::ConfigError(format!(
                    "hl7 connector URL must use mllp:// scheme, got: {u}"
                )))
            }
            None => DEFAULT_BIND
                .parse::<SocketAddr>()
                .map_err(|e| ConnectorError::ConfigError(format!("default bind invalid: {e}")))?,
        };
        Ok(Self {
            config,
            bind_addr,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        })
    }
    /// Process a parsed HL7 message: build event + ACK bytes.
    pub fn process_message(msg: &Hl7Message, connector_id: &str) -> (SourceEvent, Vec<u8>) {
        (message_to_event(msg, connector_id), build_ack(msg, "AA"))
    }
}

#[async_trait]
impl Connector for Hl7Connector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let bind_addr = self.bind_addr;
        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let listener = TcpListener::bind(bind_addr).await.map_err(|e| {
            ConnectorError::ConnectionError(format!("hl7 MLLP bind failed on {bind_addr}: {e}"))
        })?;
        tracing::info!(connector_id = %connector_id, bind = %bind_addr, "Hl7Connector listening for MLLP");
        tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                match listener.accept().await {
                    Ok((socket, peer)) => {
                        tracing::debug!(connector_id = %connector_id, peer = %peer, "MLLP client connected");
                        tokio::spawn(handle_socket(
                            socket,
                            connector_id.clone(),
                            tx.clone(),
                            events_count.clone(),
                            errors_count.clone(),
                            running.clone(),
                        ));
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "MLLP accept error");
                        errors_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(connector_id = %self.config.connector_id, "Hl7Connector stopped");
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "Hl7Connector not running".into(),
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
            last_event_timestamp: None,
            uptime_seconds: 0,
        }
    }
}

async fn handle_socket(
    mut socket: tokio::net::TcpStream,
    connector_id: String,
    tx: tokio::sync::mpsc::Sender<SourceEvent>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
) {
    let mut buf: Vec<u8> = Vec::with_capacity(READ_BUFFER_SIZE);
    let mut tmp = vec![0u8; READ_BUFFER_SIZE];
    while running.load(Ordering::SeqCst) {
        let n = match socket.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                tracing::debug!(error = %e, "MLLP socket read error");
                errors_count.fetch_add(1, Ordering::Relaxed);
                break;
            }
        };
        buf.extend_from_slice(&tmp[..n]);
        let (frames, consumed) = extract_all_mllp_frames(&buf);
        if consumed > 0 {
            buf.drain(..consumed);
        }
        for body in frames {
            match parse_hl7(&body) {
                Ok(msg) => {
                    tracing::debug!(connector_id = %connector_id, bytes = body.len(), "parsed HL7 message");
                    let (event, ack) = Hl7Connector::process_message(&msg, &connector_id);
                    if let Err(e) = socket.write_all(&ack).await {
                        tracing::debug!(error = %e, "MLLP ACK write failed");
                        errors_count.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    if tx.send(event).await.is_err() {
                        return;
                    }
                    events_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "HL7 parse error");
                    errors_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        if buf.len() > MAX_BUFFER {
            tracing::debug!(buffered = buf.len(), "MLLP buffer overflow — closing");
            errors_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(id: &str, url: Option<&str>) -> ConnectorConfig {
        ConnectorConfig {
            connector_id: id.into(),
            connector_type: "hl7".into(),
            url: url.map(str::to_string),
            entity_type: "patient".into(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        }
    }
    fn frame(body: &[u8]) -> Vec<u8> {
        let mut v = vec![MLLP_SB];
        v.extend_from_slice(body);
        v.push(MLLP_EB);
        v.push(MLLP_CR);
        v
    }
    fn prop_str<'a>(e: &'a SourceEvent, k: &str) -> Option<&'a str> {
        e.properties.get(k).and_then(|v| v.as_str())
    }

    // 1. URL parse happy path.
    #[test]
    fn test_parse_mllp_url_happy() {
        assert_eq!(parse_mllp_url("mllp://0.0.0.0:2575").unwrap().port(), 2575);
        let c = Hl7Connector::new(make_config("hl7-1", None)).unwrap();
        assert_eq!(c.bind_addr.port(), 2575);
    }

    // 2. Bad scheme → ConfigError.
    #[test]
    fn test_bad_scheme_rejected() {
        assert!(matches!(
            parse_mllp_url("tcp://0.0.0.0:2575"),
            Err(ConnectorError::ConfigError(_))
        ));
        assert!(matches!(
            Hl7Connector::new(make_config("x", Some("tcp://0.0.0.0:2575"))),
            Err(ConnectorError::ConfigError(_))
        ));
    }

    // 3. MLLP framing: extract message body from \x0B...\x1C\r.
    #[test]
    fn test_extract_mllp_frame_single() {
        let f = frame(b"MSH|^~\\&|A|B");
        match extract_mllp_frame(&f) {
            FrameOutcome::Complete(body, n) => {
                assert_eq!(body, b"MSH|^~\\&|A|B".to_vec());
                assert_eq!(n, f.len());
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    // 4. MLLP framing: split multiple messages from one TCP read.
    #[test]
    fn test_extract_all_mllp_frames_multi() {
        let mut buf = frame(b"MSH|^~\\&|first");
        buf.extend(frame(b"MSH|^~\\&|second"));
        let (frames, consumed) = extract_all_mllp_frames(&buf);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], b"MSH|^~\\&|first".to_vec());
        assert_eq!(frames[1], b"MSH|^~\\&|second".to_vec());
        assert_eq!(consumed, buf.len());
    }

    // 5. Truncated frame (no end marker) → no panic, returns "incomplete".
    #[test]
    fn test_extract_truncated_and_resync() {
        let mut buf = vec![MLLP_SB];
        buf.extend_from_slice(b"MSH|^~\\&|truncated...");
        assert!(matches!(extract_mllp_frame(&buf), FrameOutcome::Incomplete));
        buf.extend_from_slice(b"more data");
        assert!(matches!(extract_mllp_frame(&buf), FrameOutcome::Incomplete));
        // Resync: garbage before SB.
        let mut g = b"junk-bytes-before".to_vec();
        g.extend(frame(b"MSH|^~\\&|x"));
        match extract_mllp_frame(&g) {
            FrameOutcome::Resync(n) => assert_eq!(n, b"junk-bytes-before".len()),
            other => panic!("expected Resync, got {other:?}"),
        }
    }

    // 6. PID segment parsing: extract patient_id, name, DOB.
    #[test]
    fn test_pid_parsing() {
        let body = b"MSH|^~\\&|HIS|HOSP|EHR|HOSP|20260101010101||ADT^A01|MSG001|P|2.5\rPID|1||PAT12345^^^MRN||DOE^JOHN||19800115|M";
        let msg = parse_hl7(body).unwrap();
        let pid = msg.first_segment("PID").unwrap();
        assert_eq!(pid.field(3), "PAT12345^^^MRN");
        assert_eq!(pid.field(5), "DOE^JOHN");
        assert_eq!(pid.field(7), "19800115");
        assert_eq!(pid.field(8), "M");
    }

    // 7. ADT^A01 message → SourceEvent with entity_type "patient".
    #[test]
    fn test_adt_a01_to_event() {
        let body = b"MSH|^~\\&|HIS|HOSP|EHR|HOSP|20260101010101||ADT^A01|MSG001|P|2.5\r\
                     EVN|A01|20260101010101\r\
                     PID|1||PAT12345^^^MRN||DOE^JOHN||19800115|M\r\
                     PV1|1|I|ICU^101^A|1|||DOC1^SMITH^ALICE";
        let event = message_to_event(&parse_hl7(body).unwrap(), "hl7-test");
        assert_eq!(event.connector_id, "hl7-test");
        assert_eq!(event.entity_type, "patient");
        assert_eq!(event.entity_id, "PAT12345");
        assert_eq!(prop_str(&event, "msh.message_type"), Some("ADT^A01"));
        assert_eq!(prop_str(&event, "evn.trigger"), Some("A01"));
        assert_eq!(prop_str(&event, "pid.name"), Some("JOHN DOE"));
        assert_eq!(prop_str(&event, "pid.dob"), Some("19800115"));
        assert_eq!(prop_str(&event, "pid.sex"), Some("M"));
        assert_eq!(prop_str(&event, "pv1.location"), Some("ICU^101^A"));
        assert!(event.properties.contains_key("pv1.attending_doctor"));
    }

    // 8. ORU^R01 message (lab result) → SourceEvent with OBX results.
    #[test]
    fn test_oru_r01_with_obx() {
        let body = b"MSH|^~\\&|LAB|HOSP|EHR|HOSP|20260101010101||ORU^R01|LAB001|P|2.5\r\
                     PID|1||PAT99999^^^MRN||SMITH^JANE||19751212|F\r\
                     OBR|1|ORDER1|FILLER1|GLU^Glucose||20260101005959|20260101010000\r\
                     OBX|1|NM|GLU^Glucose||95|mg/dL|70-99|N|||F\r\
                     OBX|2|NM|HGB^Hemoglobin||14.2|g/dL|12-16|N|||F";
        let event = message_to_event(&parse_hl7(body).unwrap(), "hl7-lab");
        assert_eq!(event.entity_type, "patient");
        assert_eq!(event.entity_id, "PAT99999");
        let glu = event.properties.get("obx.GLU").unwrap();
        assert_eq!(glu.get("value").and_then(|v| v.as_str()), Some("95"));
        assert_eq!(glu.get("units").and_then(|v| v.as_str()), Some("mg/dL"));
        let hgb = event.properties.get("obx.HGB").unwrap();
        assert_eq!(hgb.get("value").and_then(|v| v.as_str()), Some("14.2"));
        assert!(event.properties.contains_key("obr.test_id"));
    }

    #[test]
    fn test_oru_without_pid_is_observation() {
        let body = b"MSH|^~\\&|LAB|HOSP|EHR|HOSP|20260101010101||ORU^R01|LAB001|P|2.5\r\
                     OBR|1|ORDER1|FILLER1|GLU^Glucose||20260101005959|20260101010000\r\
                     OBX|1|NM|GLU^Glucose||95|mg/dL|70-99|N|||F";
        let event = message_to_event(&parse_hl7(body).unwrap(), "hl7-lab");
        assert_eq!(event.entity_type, "observation");
        assert_eq!(event.entity_id, "GLU");
    }

    // 9. ACK generation: builds correct ACK MSH given an inbound MSH.
    #[test]
    fn test_ack_generation() {
        let body = b"MSH|^~\\&|HIS|HOSP|EHR|HOSP|20260101010101||ADT^A01|MSG001|P|2.5\r\
                     PID|1||PAT1^^^MRN||DOE^JOHN||19800115|M";
        let ack = build_ack(&parse_hl7(body).unwrap(), "AA");
        assert_eq!(ack.first().copied(), Some(MLLP_SB));
        assert!(ack.windows(2).any(|w| w == [MLLP_EB, MLLP_CR]));
        let inner = std::str::from_utf8(&ack[1..ack.len() - 2]).unwrap();
        assert!(inner.starts_with("MSH|^~\\&|"));
        assert!(inner.contains("|ACK|"));
        assert!(inner.contains("MSA|AA|MSG001"));
        // Sender is ORP; receiver is inbound sender (HIS|HOSP).
        assert!(inner.contains("|ORP|ORP|HIS|HOSP|"));
    }

    // 10. Malformed segment (missing |) → errors_count++, no panic.
    #[test]
    fn test_malformed_inputs_do_not_panic() {
        let body = b"MSH|^~\\&|HIS|HOSP|EHR|HOSP|20260101010101||ADT^A01|MSG001|P|2.5\rPID";
        assert!(matches!(
            parse_hl7(body),
            Err(ConnectorError::ParseError(_))
        ));
        assert!(matches!(
            parse_hl7(b"MSH"),
            Err(ConnectorError::ParseError(_))
        ));
        assert!(matches!(
            parse_hl7(b"PID|1||PAT1^^^MRN||DOE^JOHN"),
            Err(ConnectorError::ParseError(_))
        ));
    }

    // 11. Real-world HL7 fixture (hand-crafted ADT^A01 from HL7 v2.5 spec).
    #[test]
    fn test_realistic_adt_a01_fixture() {
        // Sanitised v2.5 sample: admit EVERYMAN^ADAM, attending ATTEND^AARON.
        let body = b"MSH|^~\\&|REGADT|MCM|IFENG||20260315133000||ADT^A01^ADT_A01|MSG00001|P|2.5\r\
                     EVN|A01|20260315133000|||N1234^NURSE^MARY\r\
                     PID|1||10006579^^^1^MR^1||EVERYMAN^ADAM^A^III||19610615|M||C|1234 EAST^^TIBURON^CA^94920^USA||(415)555-0106|||M||10987654321|123456789\r\
                     NK1|1|EVERYMAN^EVE|SPO\r\
                     PV1|1|I|N&^&^&^&^^^^^|||||ATTEND^AARON^A^^^^MD|||SUR||||A|||ATTEND^AARON^A^^^^MD|S|S00001|MEDIC|||||||||||||||||GW|||||20260315133000\r\
                     OBX|1|NM|1010.1^Body Weight||72|kg|||||F\r\
                     AL1|1|FA|3000^Penicillin^99zzz|MO|Hives";
        let msg = parse_hl7(body).unwrap();
        let event = message_to_event(&msg, "hl7-fixture");
        assert_eq!(event.entity_type, "patient");
        assert_eq!(event.entity_id, "10006579");
        assert_eq!(prop_str(&event, "pid.name"), Some("ADAM EVERYMAN"));
        assert_eq!(prop_str(&event, "pid.dob"), Some("19610615"));
        assert_eq!(prop_str(&event, "evn.trigger"), Some("A01"));
        assert_eq!(prop_str(&event, "msh.sending_app"), Some("REGADT"));
        assert_eq!(
            prop_str(&event, "msh.message_type"),
            Some("ADT^A01^ADT_A01")
        );
        let obx = event.properties.get("obx.1010.1").unwrap();
        assert_eq!(obx.get("value").and_then(|v| v.as_str()), Some("72"));
        assert_eq!(obx.get("units").and_then(|v| v.as_str()), Some("kg"));
        assert_eq!(build_ack(&msg, "AA").first().copied(), Some(MLLP_SB));
    }

    // ── extra coverage ────────────────────────────────────────────────────

    #[test]
    fn test_wrap_and_ts_helpers() {
        let out = wrap_mllp(b"MSH|^~\\&");
        assert_eq!(out.first().copied(), Some(MLLP_SB));
        assert_eq!(out.last().copied(), Some(MLLP_CR));
        assert_eq!(out[out.len() - 2], MLLP_EB);
        let dt = parse_hl7_ts("20260315133000").unwrap();
        assert_eq!(
            dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-03-15 13:30:00"
        );
        assert!(parse_hl7_ts("20260315").is_some());
        assert!(parse_hl7_ts("").is_none());
    }

    #[tokio::test]
    async fn test_health_check_not_running() {
        let c = Hl7Connector::new(make_config("hl7-h", None)).unwrap();
        assert!(c.health_check().await.is_err());
    }

    #[test]
    fn test_lifecycle_basics() {
        let c = Hl7Connector::new(make_config("hl7-id", None)).unwrap();
        assert_eq!(c.connector_id(), "hl7-id");
        assert_eq!(c.stats().events_processed, 0);
        assert_eq!(c.stats().errors, 0);
    }
}
