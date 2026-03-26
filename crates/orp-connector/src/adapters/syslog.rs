//! Syslog / CEF (Common Event Format) connector.
//!
//! Receives syslog messages over UDP or TCP (RFC 5424 / RFC 3164) and
//! optionally parses the CEF extension format used by many security appliances
//! (firewalls, IDS/IPS, SIEM forwarders).
//!
//! Produced entity types:
//! - `"host"`           — any message carrying a source hostname/IP
//! - `"network_event"`  — firewall, routing, and NAT log lines
//! - `"threat"`         — IDS/IPS alert lines (CEF severity ≥ 7, or "alert" keyword)
//! - `"vulnerability"`  — vulnerability scanner results (e.g. Qualys, Nessus CEF)

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Utc};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{TcpListener, UdpSocket};

// ---------------------------------------------------------------------------
// Parsed data types
// ---------------------------------------------------------------------------

/// RFC 5424 / RFC 3164 syslog priority: facility + severity.
#[derive(Clone, Debug, PartialEq)]
pub struct SyslogPriority {
    pub facility: u8,
    pub severity: u8,
}

impl SyslogPriority {
    pub fn from_pri(pri: u8) -> Self {
        Self {
            facility: pri >> 3,
            severity: pri & 0x07,
        }
    }

    pub fn severity_name(&self) -> &'static str {
        match self.severity {
            0 => "emergency",
            1 => "alert",
            2 => "critical",
            3 => "error",
            4 => "warning",
            5 => "notice",
            6 => "informational",
            7 => "debug",
            _ => "unknown",
        }
    }
}

/// Parsed syslog message (RFC 5424 or RFC 3164 best-effort).
#[derive(Clone, Debug)]
pub struct SyslogMessage {
    pub priority: Option<SyslogPriority>,
    pub timestamp: DateTime<Utc>,
    pub hostname: Option<String>,
    pub app_name: Option<String>,
    pub proc_id: Option<String>,
    pub msg_id: Option<String>,
    pub message: String,
    /// Structured data fields (RFC 5424 SD-ELEMENT key=value pairs).
    pub structured_data: HashMap<String, String>,
    /// Original raw line.
    pub raw: String,
}

/// Parsed CEF (ArcSight Common Event Format) extension.
#[derive(Clone, Debug)]
pub struct CefMessage {
    pub cef_version: u8,
    pub device_vendor: String,
    pub device_product: String,
    pub device_version: String,
    pub signature_id: String,
    pub name: String,
    pub severity: u8,
    pub extensions: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse a syslog PRI part: `<NNN>`.
fn parse_priority(s: &str) -> Option<(SyslogPriority, &str)> {
    if !s.starts_with('<') {
        return None;
    }
    let end = s.find('>')?;
    let pri_str = &s[1..end];
    let pri: u8 = pri_str.parse().ok()?;
    Some((SyslogPriority::from_pri(pri), &s[end + 1..]))
}

/// Very simple RFC 5424 / RFC 3164 parser. Returns a best-effort `SyslogMessage`.
///
/// RFC 5424 format:
/// `<PRI>VERSION TIMESTAMP HOSTNAME APP-NAME PROCID MSGID SD MSG`
///
/// RFC 3164 format:
/// `<PRI>TIMESTAMP HOSTNAME TAG: MSG`
pub fn parse_syslog(raw: &str) -> SyslogMessage {
    let raw = raw.trim_end_matches(['\n', '\r']);
    let (priority, rest) = parse_priority(raw)
        .map(|(p, r)| (Some(p), r))
        .unwrap_or((None, raw));

    // Try RFC 5424: next token is VERSION (single digit)
    let (timestamp, hostname, app_name, proc_id, msg_id, message, structured_data) =
        if rest.starts_with(|c: char| c.is_ascii_digit()) && rest.len() > 2 && &rest[1..2] == " "
        {
            parse_rfc5424(rest)
        } else {
            parse_rfc3164(rest)
        };

    SyslogMessage {
        priority,
        timestamp,
        hostname,
        app_name,
        proc_id,
        msg_id,
        message,
        structured_data,
        raw: raw.to_string(),
    }
}

/// Parsed syslog fields: (timestamp, hostname, app_name, proc_id, msg_id, message, structured_data).
type SyslogFields = (
    DateTime<Utc>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    HashMap<String, String>,
);

fn parse_rfc5424(s: &str) -> SyslogFields {
    let mut parts = s.splitn(7, ' ');
    let _version = parts.next().unwrap_or("1");
    let ts = parts.next().unwrap_or("-");
    let hostname = parts.next().filter(|&s| s != "-").map(str::to_string);
    let app_name = parts.next().filter(|&s| s != "-").map(str::to_string);
    let proc_id = parts.next().filter(|&s| s != "-").map(str::to_string);
    let msg_id = parts.next().filter(|&s| s != "-").map(str::to_string);
    let rest = parts.next().unwrap_or("");

    let timestamp = DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    // Parse structured data: `[sd_id key="value" ...]`
    let (structured_data, message) = parse_sd(rest);

    (timestamp, hostname, app_name, proc_id, msg_id, message.to_string(), structured_data)
}

fn parse_rfc3164(s: &str) -> SyslogFields {
    // Format: `MMM DD HH:MM:SS hostname tag: message`
    // We do a best-effort tokenisation.
    let mut parts = s.splitn(4, ' ');
    let month = parts.next().unwrap_or("");
    let day = parts.next().unwrap_or("");
    let time = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");

    // Build a rough timestamp string and try to parse; fall back to now.
    let ts_str = format!("{month} {day} {time}");
    let timestamp = chrono::NaiveDateTime::parse_from_str(&ts_str, "%b %d %H:%M:%S")
        .map(|ndt| {
            let now = Utc::now();
            ndt.and_local_timezone(Utc)
                .single()
                .unwrap_or(now)
                .with_year(now.year())
                .unwrap_or(now)
        })
        .unwrap_or_else(|_| Utc::now());

    let mut msg_parts = rest.splitn(2, ' ');
    let hostname = msg_parts.next().map(str::to_string);
    let message = msg_parts.next().unwrap_or(rest).to_string();

    (timestamp, hostname, None, None, None, message, HashMap::new())
}

/// Parse RFC 5424 structured-data section and return (sd_map, remaining_message).
fn parse_sd(s: &str) -> (HashMap<String, String>, &str) {
    let mut map = HashMap::new();
    if s.is_empty() || s.starts_with('-') {
        let msg = if let Some(stripped) = s.strip_prefix("- ") { stripped } else { s.trim_start_matches('-') };
        return (map, msg);
    }
    if !s.starts_with('[') {
        return (map, s);
    }
    // Find closing bracket (simplified — doesn't handle escaped brackets)
    if let Some(end) = s.find(']') {
        let sd_content = &s[1..end];
        let remaining = s[end + 1..].trim_start();
        // sd_content: `id key="val" key2="val2"`
        let mut tokens = sd_content.splitn(2, ' ');
        let _sd_id = tokens.next();
        if let Some(kv_str) = tokens.next() {
            for kv in kv_str.split_whitespace() {
                if let Some(eq) = kv.find('=') {
                    let k = kv[..eq].to_string();
                    let v = kv[eq + 1..].trim_matches('"').to_string();
                    map.insert(k, v);
                }
            }
        }
        return (map, remaining);
    }
    (map, s)
}

/// Parse a CEF log line.
///
/// Format:
/// `CEF:Version|DeviceVendor|DeviceProduct|DeviceVersion|SignatureID|Name|Severity|Extensions`
///
/// Returns `None` if the line is not a CEF message.
pub fn parse_cef(line: &str) -> Option<CefMessage> {
    // CEF lines may be embedded in a syslog message
    let cef_start = line.find("CEF:")?;
    let cef_part = &line[cef_start..];

    let mut parts = cef_part.splitn(8, '|');
    let version_str = parts.next()?; // "CEF:0"
    let cef_version: u8 = version_str
        .trim_start_matches("CEF:")
        .parse()
        .unwrap_or(0);

    let device_vendor = parts.next()?.to_string();
    let device_product = parts.next()?.to_string();
    let device_version = parts.next()?.to_string();
    let signature_id = parts.next()?.to_string();
    let name = parts.next()?.to_string();
    let severity_str = parts.next()?;
    let severity: u8 = severity_str.trim().parse().unwrap_or(0);
    let ext_str = parts.next().unwrap_or("");

    // Parse extensions: `key=value key2=value2` (values may contain spaces)
    let extensions = parse_cef_extensions(ext_str);

    Some(CefMessage {
        cef_version,
        device_vendor,
        device_product,
        device_version,
        signature_id,
        name,
        severity,
        extensions,
    })
}

fn parse_cef_extensions(ext_str: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    // CEF extension values can contain spaces; keys are separated from the
    // next key by a space before `key=`.
    // Strategy: split on ` key=` boundaries.
    let mut remaining = ext_str;
    while !remaining.is_empty() {
        let eq_pos = match remaining.find('=') {
            Some(p) => p,
            None => break,
        };
        let key = remaining[..eq_pos].trim().to_string();
        let after_eq = &remaining[eq_pos + 1..];
        // Find start of next key (a word ending in '=')
        let next_key_start = find_next_cef_key_start(after_eq);
        let value = after_eq[..next_key_start].trim_end().to_string();
        map.insert(key, value);
        remaining = &after_eq[next_key_start..];
    }
    map
}

fn find_next_cef_key_start(s: &str) -> usize {
    // Look for a pattern: ` word=`
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            // Look ahead for `word=`
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'=' && bytes[j] != b' ' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                return i; // cut value here
            }
        }
        i += 1;
    }
    s.len()
}

// ---------------------------------------------------------------------------
// Entity type classification
// ---------------------------------------------------------------------------

fn classify_entity_type(syslog: &SyslogMessage, cef: Option<&CefMessage>) -> &'static str {
    if let Some(cef) = cef {
        if cef.severity >= 7 {
            return "threat";
        }
        let product_lower = cef.device_product.to_lowercase();
        if product_lower.contains("ids")
            || product_lower.contains("ips")
            || product_lower.contains("snort")
            || product_lower.contains("suricata")
        {
            return "threat";
        }
        if product_lower.contains("vuln")
            || product_lower.contains("qualys")
            || product_lower.contains("nessus")
            || product_lower.contains("openvas")
        {
            return "vulnerability";
        }
        if product_lower.contains("firewall")
            || product_lower.contains("fw")
            || product_lower.contains("pix")
            || product_lower.contains("asa")
            || product_lower.contains("fortinet")
            || product_lower.contains("checkpoint")
        {
            return "network_event";
        }
    }

    // Check both the parsed message field and the raw line for classification keywords.
    let searchable = format!("{} {}", syslog.message, syslog.raw).to_lowercase();
    if searchable.contains("attack")
        || searchable.contains("intrusion")
        || searchable.contains("exploit")
        || searchable.contains("malware")
        || searchable.contains("trojan")
    {
        return "threat";
    }
    if searchable.contains("denied")
        || searchable.contains("blocked")
        || searchable.contains("drop")
        || searchable.contains("firewall")
        || searchable.contains("nat ")
    {
        return "network_event";
    }
    "host"
}

// ---------------------------------------------------------------------------
// SourceEvent construction
// ---------------------------------------------------------------------------

fn syslog_to_event(
    syslog: &SyslogMessage,
    cef: Option<&CefMessage>,
    connector_id: &str,
) -> SourceEvent {
    let entity_type = classify_entity_type(syslog, cef);

    // Entity ID: prefer CEF source address, then syslog hostname
    let entity_id = cef
        .and_then(|c| c.extensions.get("src").cloned())
        .or_else(|| syslog.hostname.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let mut properties: HashMap<String, JsonValue> = HashMap::new();

    // Syslog fields
    if let Some(ref h) = syslog.hostname {
        properties.insert("hostname".to_string(), serde_json::json!(h));
    }
    if let Some(ref app) = syslog.app_name {
        properties.insert("app_name".to_string(), serde_json::json!(app));
    }
    if let Some(ref pid) = syslog.proc_id {
        properties.insert("proc_id".to_string(), serde_json::json!(pid));
    }
    if let Some(ref p) = syslog.priority {
        properties.insert("facility".to_string(), serde_json::json!(p.facility));
        properties.insert("severity".to_string(), serde_json::json!(p.severity_name()));
    }
    properties.insert("message".to_string(), serde_json::json!(syslog.message));

    // CEF fields
    if let Some(cef) = cef {
        properties.insert("cef_vendor".to_string(), serde_json::json!(cef.device_vendor));
        properties.insert("cef_product".to_string(), serde_json::json!(cef.device_product));
        properties.insert("cef_signature_id".to_string(), serde_json::json!(cef.signature_id));
        properties.insert("cef_name".to_string(), serde_json::json!(cef.name));
        properties.insert("cef_severity".to_string(), serde_json::json!(cef.severity));
        for (k, v) in &cef.extensions {
            properties.insert(format!("cef_{k}"), serde_json::json!(v));
        }
    }

    // Structured data
    for (k, v) in &syslog.structured_data {
        properties.insert(format!("sd_{k}"), serde_json::json!(v));
    }

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id: format!("{entity_type}:{entity_id}"),
        entity_type: entity_type.to_string(),
        properties,
        timestamp: syslog.timestamp,
        latitude: None,
        longitude: None,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// Transport protocol for syslog reception.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SyslogTransport {
    #[default]
    Udp,
    Tcp,
    Both,
}

/// Syslog / CEF connector configuration.
#[derive(Clone, Debug)]
pub struct SyslogConfig {
    /// Bind address (e.g. `"0.0.0.0:514"`).
    pub bind_addr: String,
    /// Transport protocol.
    pub transport: SyslogTransport,
    /// Whether to attempt CEF parsing on each message.
    pub parse_cef: bool,
    /// Maximum UDP datagram size in bytes.
    pub udp_buffer_size: usize,
}

impl Default for SyslogConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:514".to_string(),
            transport: SyslogTransport::Udp,
            parse_cef: true,
            udp_buffer_size: 65_536,
        }
    }
}

/// ORP connector that receives syslog (RFC 5424 / RFC 3164) and CEF events.
pub struct SyslogConnector {
    config: ConnectorConfig,
    syslog_config: SyslogConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl SyslogConnector {
    pub fn new(config: ConnectorConfig, syslog_config: SyslogConfig) -> Self {
        Self {
            config,
            syslog_config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Build from `ConnectorConfig::properties`:
    /// - `bind_addr`   (string, default `"0.0.0.0:514"`)
    /// - `transport`   (`"udp"` | `"tcp"` | `"both"`, default `"udp"`)
    /// - `parse_cef`   (bool, default `true`)
    pub fn from_connector_config(config: ConnectorConfig) -> Self {
        let bind_addr = config
            .properties
            .get("bind_addr")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0.0:514")
            .to_string();
        let transport = match config
            .properties
            .get("transport")
            .and_then(|v| v.as_str())
            .unwrap_or("udp")
        {
            "tcp" => SyslogTransport::Tcp,
            "both" => SyslogTransport::Both,
            _ => SyslogTransport::Udp,
        };
        let parse_cef = config
            .properties
            .get("parse_cef")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let syslog_config = SyslogConfig {
            bind_addr,
            transport,
            parse_cef,
            ..Default::default()
        };
        Self::new(config, syslog_config)
    }

    /// Process a single raw line into a `SourceEvent`.
    pub fn process_line(
        line: &str,
        connector_id: &str,
        parse_cef_flag: bool,
    ) -> Option<SourceEvent> {
        if line.trim().is_empty() {
            return None;
        }
        let syslog = parse_syslog(line);
        let cef = if parse_cef_flag {
            parse_cef(line)
        } else {
            None
        };
        Some(syslog_to_event(&syslog, cef.as_ref(), connector_id))
    }
}

#[async_trait]
impl Connector for SyslogConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let syslog_cfg = self.syslog_config.clone();
        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();

        tracing::info!(
            connector_id = %connector_id,
            bind = %syslog_cfg.bind_addr,
            "SyslogConnector starting"
        );

        let addr: SocketAddr = syslog_cfg.bind_addr.parse().map_err(|e| {
            ConnectorError::ConfigError(format!("Invalid bind address: {e}"))
        })?;

        // ── UDP listener ──────────────────────────────────────────────────
        if matches!(
            syslog_cfg.transport,
            SyslogTransport::Udp | SyslogTransport::Both
        ) {
            let socket = UdpSocket::bind(addr).await.map_err(|e| {
                ConnectorError::ConnectionError(format!("UDP bind failed: {e}"))
            })?;

            let tx_udp = tx.clone();
            let running_udp = running.clone();
            let cid = connector_id.clone();
            let parse_cef_flag = syslog_cfg.parse_cef;
            let ec = events_count.clone();
            let err_c = errors_count.clone();
            let buf_size = syslog_cfg.udp_buffer_size;

            tokio::spawn(async move {
                let mut buf = vec![0u8; buf_size];
                while running_udp.load(Ordering::SeqCst) {
                    match socket.recv_from(&mut buf).await {
                        Ok((len, _peer)) => {
                            let line = String::from_utf8_lossy(&buf[..len]);
                            if let Some(event) =
                                SyslogConnector::process_line(&line, &cid, parse_cef_flag)
                            {
                                if tx_udp.send(event).await.is_err() {
                                    return;
                                }
                                ec.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("UDP recv error: {e}");
                            err_c.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }

        // ── TCP listener ──────────────────────────────────────────────────
        if matches!(
            syslog_cfg.transport,
            SyslogTransport::Tcp | SyslogTransport::Both
        ) {
            let tcp_addr = if matches!(syslog_cfg.transport, SyslogTransport::Both) {
                // TCP on port+1 to avoid conflict when both are active on same bind IP
                SocketAddr::new(addr.ip(), addr.port() + 1)
            } else {
                addr
            };

            let listener = TcpListener::bind(tcp_addr).await.map_err(|e| {
                ConnectorError::ConnectionError(format!("TCP bind failed: {e}"))
            })?;

            let tx_tcp = tx.clone();
            let running_tcp = running.clone();
            let cid = connector_id.clone();
            let parse_cef_flag = syslog_cfg.parse_cef;
            let ec = events_count.clone();
            let err_c = errors_count.clone();

            tokio::spawn(async move {
                while running_tcp.load(Ordering::SeqCst) {
                    match listener.accept().await {
                        Ok((stream, peer)) => {
                            tracing::debug!("Syslog TCP connection from {peer}");
                            let tx2 = tx_tcp.clone();
                            let cid2 = cid.clone();
                            let ec2 = ec.clone();
                            let err_c2 = err_c.clone();
                            tokio::spawn(async move {
                                let reader = BufReader::new(stream);
                                let mut lines = reader.lines();
                                while let Ok(Some(line)) = lines.next_line().await {
                                    if let Some(event) =
                                        SyslogConnector::process_line(&line, &cid2, parse_cef_flag)
                                    {
                                        if tx2.send(event).await.is_err() {
                                            return;
                                        }
                                        ec2.fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        err_c2.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!("TCP accept error: {e}");
                            err_c.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "SyslogConnector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "SyslogConnector not running".to_string(),
            ))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        use crate::traits::ConnectorStats;
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
    use std::collections::HashMap;

    fn make_config(id: &str) -> ConnectorConfig {
        ConnectorConfig {
            connector_id: id.to_string(),
            connector_type: "syslog".to_string(),
            url: None,
            entity_type: "host".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        }
    }

    // ── SyslogPriority ────────────────────────────────────────────────────

    #[test]
    fn test_priority_parsing() {
        let p = SyslogPriority::from_pri(13); // facility=1 (user), severity=5 (notice)
        assert_eq!(p.facility, 1);
        assert_eq!(p.severity, 5);
        assert_eq!(p.severity_name(), "notice");
    }

    #[test]
    fn test_priority_emergency() {
        let p = SyslogPriority::from_pri(0);
        assert_eq!(p.severity_name(), "emergency");
    }

    // ── parse_syslog ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_rfc5424() {
        let line = "<165>1 2023-10-01T12:00:00Z myhost myapp 1234 ID47 - Hello syslog";
        let msg = parse_syslog(line);
        assert_eq!(msg.hostname.as_deref(), Some("myhost"));
        assert_eq!(msg.app_name.as_deref(), Some("myapp"));
        assert!(msg.message.contains("Hello"));
        assert!(msg.priority.is_some());
    }

    #[test]
    fn test_parse_rfc3164() {
        let line = "<34>Oct  1 12:00:00 myhostname myapp: a test message";
        let msg = parse_syslog(line);
        assert!(msg.priority.is_some());
        assert_eq!(msg.priority.unwrap().severity_name(), "critical");
    }

    #[test]
    fn test_parse_no_priority() {
        let line = "Oct  1 12:00:00 hostname app: msg";
        let msg = parse_syslog(line);
        assert!(msg.priority.is_none());
    }

    #[test]
    fn test_parse_empty_line() {
        let msg = parse_syslog("");
        assert!(msg.message.is_empty() || msg.hostname.is_none());
    }

    // ── parse_cef ─────────────────────────────────────────────────────────

    #[test]
    fn test_parse_cef_basic() {
        let line = "CEF:0|Cisco|ASA|9.1|106023|Deny TCP|5|src=192.168.1.1 dst=10.0.0.1 dpt=443";
        let cef = parse_cef(line).unwrap();
        assert_eq!(cef.device_vendor, "Cisco");
        assert_eq!(cef.device_product, "ASA");
        assert_eq!(cef.signature_id, "106023");
        assert_eq!(cef.name, "Deny TCP");
        assert_eq!(cef.severity, 5);
        assert_eq!(cef.extensions.get("src").unwrap(), "192.168.1.1");
        assert_eq!(cef.extensions.get("dst").unwrap(), "10.0.0.1");
        assert_eq!(cef.extensions.get("dpt").unwrap(), "443");
    }

    #[test]
    fn test_parse_cef_in_syslog() {
        let line = "<165>1 2023-10-01T12:00:00Z fw01 asa - - CEF:0|Cisco|ASA|9.1|111001|User login|3|src=1.2.3.4";
        let cef = parse_cef(line).unwrap();
        assert_eq!(cef.device_vendor, "Cisco");
        assert_eq!(cef.extensions.get("src").unwrap(), "1.2.3.4");
    }

    #[test]
    fn test_parse_cef_not_cef() {
        assert!(parse_cef("not a cef line").is_none());
    }

    #[test]
    fn test_parse_cef_high_severity() {
        let line = "CEF:0|Snort|IDS|2.9|1001|SQL Injection|9|src=1.2.3.4";
        let cef = parse_cef(line).unwrap();
        assert_eq!(cef.severity, 9);
    }

    // ── classify_entity_type ─────────────────────────────────────────────

    #[test]
    fn test_classify_threat_from_cef_severity() {
        let syslog = parse_syslog("Some message");
        let cef = CefMessage {
            cef_version: 0,
            device_vendor: "Snort".to_string(),
            device_product: "IDS".to_string(),
            device_version: "2.9".to_string(),
            signature_id: "1001".to_string(),
            name: "Alert".to_string(),
            severity: 9,
            extensions: HashMap::new(),
        };
        assert_eq!(classify_entity_type(&syslog, Some(&cef)), "threat");
    }

    #[test]
    fn test_classify_network_event_from_firewall_keyword() {
        let syslog = parse_syslog("connection denied by firewall rule");
        assert_eq!(classify_entity_type(&syslog, None), "network_event");
    }

    #[test]
    fn test_classify_vulnerability() {
        let syslog = parse_syslog("some message");
        let cef = CefMessage {
            cef_version: 0,
            device_vendor: "Qualys".to_string(),
            device_product: "Qualys VM".to_string(),
            device_version: "1.0".to_string(),
            signature_id: "QID-123".to_string(),
            name: "SSL Vulnerability".to_string(),
            severity: 4,
            extensions: HashMap::new(),
        };
        assert_eq!(classify_entity_type(&syslog, Some(&cef)), "vulnerability");
    }

    #[test]
    fn test_classify_threat_from_message() {
        let syslog = parse_syslog("malware detected on host");
        assert_eq!(classify_entity_type(&syslog, None), "threat");
    }

    #[test]
    fn test_classify_default_host() {
        let syslog = parse_syslog("sshd: accepted password for user1 from 1.2.3.4");
        assert_eq!(classify_entity_type(&syslog, None), "host");
    }

    // ── process_line ──────────────────────────────────────────────────────

    #[test]
    fn test_process_line_produces_event() {
        let line = "<165>1 2023-10-01T12:00:00Z myhost app 123 - - test message";
        let event = SyslogConnector::process_line(line, "syslog-1", false).unwrap();
        assert_eq!(event.connector_id, "syslog-1");
        assert!(!event.entity_id.is_empty());
    }

    #[test]
    fn test_process_line_empty_returns_none() {
        assert!(SyslogConnector::process_line("", "syslog-1", false).is_none());
    }

    #[test]
    fn test_process_cef_line_entity_id_contains_src() {
        let line = "CEF:0|Cisco|ASA|9.1|106023|Deny|5|src=10.20.30.40 dst=8.8.8.8";
        let event = SyslogConnector::process_line(line, "syslog-1", true).unwrap();
        assert!(event.entity_id.contains("10.20.30.40"));
    }

    // ── connector construction ────────────────────────────────────────────

    #[test]
    fn test_from_connector_config_defaults() {
        let config = make_config("syslog-test");
        let c = SyslogConnector::from_connector_config(config);
        assert_eq!(c.syslog_config.bind_addr, "0.0.0.0:514");
        assert!(c.syslog_config.parse_cef);
    }

    #[test]
    fn test_connector_id() {
        let c = SyslogConnector::new(make_config("my-syslog"), SyslogConfig::default());
        assert_eq!(c.connector_id(), "my-syslog");
    }

    #[test]
    fn test_initial_stats() {
        let c = SyslogConnector::new(make_config("s1"), SyslogConfig::default());
        assert_eq!(c.stats().events_processed, 0);
    }

    #[tokio::test]
    async fn test_health_check_not_running() {
        let c = SyslogConnector::new(make_config("s1"), SyslogConfig::default());
        assert!(c.health_check().await.is_err());
    }
}
