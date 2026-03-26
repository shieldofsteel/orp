use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Zeek (formerly Bro) log parser
// ---------------------------------------------------------------------------
// Zeek is a network security monitor that produces structured logs.
// Log formats:
//   - TSV (tab-separated): default, with header lines (#separator, #fields, #types, …)
//   - JSON: one JSON object per line (when LogAscii::use_json = T)
//
// Key log files:
//   conn.log    — TCP/UDP/ICMP connections
//   dns.log     — DNS queries and responses
//   http.log    — HTTP requests/responses
//   ssl.log     — SSL/TLS handshake info
//   files.log   — file analysis results
//   x509.log    — X.509 certificate info
//   notice.log  — Zeek notices / alerts
//   weird.log   — unusual / unexpected network behavior

/// Zeek log type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZeekLogType {
    Conn,
    Dns,
    Http,
    Ssl,
    Files,
    X509,
    Notice,
    Weird,
    Unknown(String),
}

impl ZeekLogType {
    pub fn from_path(path: &str) -> Self {
        let filename = path.rsplit('/').next().unwrap_or(path);
        let name = filename.trim_end_matches(".log").trim_end_matches(".json");
        match name {
            "conn" => ZeekLogType::Conn,
            "dns" => ZeekLogType::Dns,
            "http" => ZeekLogType::Http,
            "ssl" => ZeekLogType::Ssl,
            "files" => ZeekLogType::Files,
            "x509" => ZeekLogType::X509,
            "notice" => ZeekLogType::Notice,
            "weird" => ZeekLogType::Weird,
            _ => ZeekLogType::Unknown(name.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            ZeekLogType::Conn => "conn",
            ZeekLogType::Dns => "dns",
            ZeekLogType::Http => "http",
            ZeekLogType::Ssl => "ssl",
            ZeekLogType::Files => "files",
            ZeekLogType::X509 => "x509",
            ZeekLogType::Notice => "notice",
            ZeekLogType::Weird => "weird",
            ZeekLogType::Unknown(s) => s,
        }
    }
}

/// Parsed Zeek TSV header metadata.
#[derive(Clone, Debug)]
pub struct ZeekTsvHeader {
    pub separator: char,
    pub set_separator: String,
    pub empty_field: String,
    pub unset_field: String,
    pub path: String,
    pub fields: Vec<String>,
    pub types: Vec<String>,
}

/// A generic Zeek record (one log line).
#[derive(Clone, Debug)]
pub struct ZeekRecord {
    pub log_type: ZeekLogType,
    pub fields: HashMap<String, JsonValue>,
    pub timestamp: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// TSV parsing
// ---------------------------------------------------------------------------

/// Parse Zeek TSV header lines (lines starting with #).
pub fn parse_zeek_tsv_header(lines: &[&str]) -> Result<ZeekTsvHeader, ConnectorError> {
    let mut separator = '\t';
    let mut set_separator = ",".to_string();
    let mut empty_field = "(empty)".to_string();
    let mut unset_field = "-".to_string();
    let mut path = String::new();
    let mut fields = Vec::new();
    let mut types = Vec::new();

    for line in lines {
        if !line.starts_with('#') {
            continue;
        }
        let line = &line[1..]; // strip leading #
        if let Some(rest) = line.strip_prefix("separator ") {
            // Zeek encodes separator as \x09, etc.
            separator = parse_zeek_separator(rest);
        } else if let Some(rest) = line.strip_prefix("set_separator") {
            set_separator = rest.trim_start().to_string();
        } else if let Some(rest) = line.strip_prefix("empty_field") {
            empty_field = rest.trim_start().to_string();
        } else if let Some(rest) = line.strip_prefix("unset_field") {
            unset_field = rest.trim_start().to_string();
        } else if let Some(rest) = line.strip_prefix("path") {
            path = rest.trim_start().to_string();
        } else if let Some(rest) = line.strip_prefix("fields") {
            fields = rest
                .split(separator)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else if let Some(rest) = line.strip_prefix("types") {
            types = rest
                .split(separator)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    if fields.is_empty() {
        return Err(ConnectorError::ParseError(
            "Zeek TSV: no #fields header found".into(),
        ));
    }

    Ok(ZeekTsvHeader {
        separator,
        set_separator,
        empty_field,
        unset_field,
        path,
        fields,
        types,
    })
}

/// Decode Zeek separator encoding (e.g., `\x09` → tab).
fn parse_zeek_separator(s: &str) -> char {
    let s = s.trim();
    if s.starts_with("\\x") && s.len() >= 4 {
        if let Ok(byte) = u8::from_str_radix(&s[2..4], 16) {
            return byte as char;
        }
    }
    s.chars().next().unwrap_or('\t')
}

/// Parse a Zeek epoch timestamp string to DateTime<Utc>.
pub fn parse_zeek_timestamp(ts_str: &str) -> Option<DateTime<Utc>> {
    // Zeek timestamps are epoch floats: "1700000000.123456"
    let f: f64 = ts_str.parse().ok()?;
    let secs = f.trunc() as i64;
    let nanos = ((f.fract()) * 1_000_000_000.0) as u32;
    DateTime::from_timestamp(secs, nanos)
}

/// Parse a single TSV data line into a ZeekRecord.
pub fn parse_zeek_tsv_line(
    line: &str,
    header: &ZeekTsvHeader,
) -> Option<ZeekRecord> {
    if line.starts_with('#') {
        return None;
    }
    let values: Vec<&str> = line.split(header.separator).collect();
    if values.len() != header.fields.len() {
        return None;
    }

    let mut fields = HashMap::new();
    let mut timestamp = None;

    for (i, field_name) in header.fields.iter().enumerate() {
        let raw = values[i];
        if raw == header.unset_field || raw == header.empty_field {
            continue;
        }

        // Parse timestamp field
        if field_name == "ts" {
            timestamp = parse_zeek_timestamp(raw);
            fields.insert(field_name.clone(), json!(raw));
            continue;
        }

        // Determine the Zeek type and convert
        let zeek_type = header.types.get(i).map(|s| s.as_str()).unwrap_or("string");
        let value = zeek_field_to_json(raw, zeek_type, &header.set_separator);
        fields.insert(field_name.clone(), value);
    }

    let log_type = ZeekLogType::from_path(&header.path);

    Some(ZeekRecord {
        log_type,
        fields,
        timestamp,
    })
}

/// Convert a Zeek field value to JSON based on the Zeek type.
fn zeek_field_to_json(raw: &str, zeek_type: &str, set_separator: &str) -> JsonValue {
    match zeek_type {
        "count" | "int" => raw.parse::<i64>().map(JsonValue::from).unwrap_or(json!(raw)),
        "double" => raw.parse::<f64>().map(JsonValue::from).unwrap_or(json!(raw)),
        "bool" => match raw {
            "T" => json!(true),
            "F" => json!(false),
            _ => json!(raw),
        },
        "port" => raw.parse::<u16>().map(JsonValue::from).unwrap_or(json!(raw)),
        "time" | "interval" => raw.parse::<f64>().map(JsonValue::from).unwrap_or(json!(raw)),
        t if t.starts_with("set[") || t.starts_with("vector[") => {
            let items: Vec<JsonValue> = raw
                .split(set_separator)
                .map(|s| json!(s))
                .collect();
            json!(items)
        }
        _ => json!(raw),
    }
}

/// Parse the full content of a Zeek TSV log file.
pub fn parse_zeek_tsv(content: &str) -> Result<Vec<ZeekRecord>, ConnectorError> {
    let lines: Vec<&str> = content.lines().collect();

    // Collect header lines
    let header_lines: Vec<&str> = lines.iter().filter(|l| l.starts_with('#')).copied().collect();
    if header_lines.is_empty() {
        return Err(ConnectorError::ParseError(
            "Zeek TSV: no header lines found".into(),
        ));
    }

    let header = parse_zeek_tsv_header(&header_lines)?;
    let mut records = Vec::new();

    for line in &lines {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(record) = parse_zeek_tsv_line(line, &header) {
            records.push(record);
        }
    }

    Ok(records)
}

// ---------------------------------------------------------------------------
// JSON parsing
// ---------------------------------------------------------------------------

/// Parse a single Zeek JSON line.
pub fn parse_zeek_json_line(line: &str, log_type: &ZeekLogType) -> Option<ZeekRecord> {
    let obj: HashMap<String, JsonValue> = serde_json::from_str(line).ok()?;

    let timestamp = obj
        .get("ts")
        .and_then(|v| v.as_f64())
        .and_then(|f| {
            let secs = f.trunc() as i64;
            let nanos = ((f.fract()) * 1_000_000_000.0) as u32;
            DateTime::from_timestamp(secs, nanos)
        });

    Some(ZeekRecord {
        log_type: log_type.clone(),
        fields: obj,
        timestamp,
    })
}

/// Parse an entire Zeek JSON log file (one JSON object per line).
pub fn parse_zeek_json(content: &str, log_type: &ZeekLogType) -> Vec<ZeekRecord> {
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| parse_zeek_json_line(l, log_type))
        .collect()
}

// ---------------------------------------------------------------------------
// Record → SourceEvent conversion
// ---------------------------------------------------------------------------

/// Convert a ZeekRecord into a SourceEvent.
pub fn zeek_record_to_source_event(
    record: &ZeekRecord,
    connector_id: &str,
) -> SourceEvent {
    let entity_id = build_entity_id(record);

    let mut properties = record.fields.clone();
    properties.insert("zeek_log_type".into(), json!(record.log_type.as_str()));

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "network_event".into(),
        properties,
        timestamp: record.timestamp.unwrap_or_else(Utc::now),
        latitude: None,
        longitude: None,
    }
}

/// Build an entity ID from Zeek record fields.
fn build_entity_id(record: &ZeekRecord) -> String {
    // For conn/dns/http/ssl: use uid if present, else build from src/dst
    if let Some(uid) = record.fields.get("uid").and_then(|v| v.as_str()) {
        return format!("zeek:{}", uid);
    }

    let src = record
        .fields
        .get("id.orig_h")
        .or_else(|| record.fields.get("id_orig_h"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let dst = record
        .fields
        .get("id.resp_h")
        .or_else(|| record.fields.get("id_resp_h"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    format!("zeek:{}-{}", src, dst)
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct ZeekConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl ZeekConnector {
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
impl Connector for ZeekConnector {
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
                ConnectorError::ConfigError("Zeek: url (file path) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        // Detect format: TSV (starts with #) or JSON (starts with {)
        let first_line = content.lines().find(|l| !l.trim().is_empty());
        let records = match first_line {
            Some(l) if l.starts_with('#') => parse_zeek_tsv(&content).inspect_err(|_e| {
                errors.fetch_add(1, Ordering::Relaxed);
            })?,
            Some(l) if l.starts_with('{') => {
                let log_type = ZeekLogType::from_path(path);
                parse_zeek_json(&content, &log_type)
            }
            _ => {
                return Err(ConnectorError::ParseError(
                    "Zeek: unknown format (expected TSV with # headers or JSON)".into(),
                ));
            }
        };

        for record in &records {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let event = zeek_record_to_source_event(record, &connector_id);
            if tx.send(event).await.is_err() {
                break;
            }
            events_processed.fetch_add(1, Ordering::Relaxed);
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
                "Zeek connector is not running".into(),
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

    const CONN_LOG_TSV: &str = r#"#separator \x09
#set_separator	,
#empty_field	(empty)
#unset_field	-
#path	conn
#fields	ts	uid	id.orig_h	id.orig_p	id.resp_h	id.resp_p	proto	service	duration	orig_bytes	resp_bytes	conn_state	missed_bytes	history	orig_pkts	orig_ip_bytes	resp_pkts	resp_ip_bytes
#types	time	string	addr	port	addr	port	enum	string	interval	count	count	string	count	string	count	count	count	count
1700000000.123456	CYfGSH4BHpKEhGLpf	192.168.1.100	52134	10.0.0.1	80	tcp	http	1.234567	512	2048	SF	0	ShADad	6	824	4	2248
1700000001.654321	DAbCDe5FGhIjKlMnO	192.168.1.101	33456	10.0.0.2	443	tcp	ssl	5.678901	1024	4096	SF	0	ShADad	10	1544	8	4576"#;

    const DNS_LOG_TSV: &str = r#"#separator \x09
#set_separator	,
#empty_field	(empty)
#unset_field	-
#path	dns
#fields	ts	uid	id.orig_h	id.orig_p	id.resp_h	id.resp_p	proto	trans_id	rtt	query	qclass	qclass_name	qtype	qtype_name	rcode	rcode_name	AA	TC	RD	RA	Z	answers	TTLs	rejected
#types	time	string	addr	port	addr	port	enum	count	interval	string	count	string	count	string	count	string	bool	bool	bool	bool	count	set[string]	set[interval]	bool
1700000000.111111	ABC123	192.168.1.100	55555	8.8.8.8	53	udp	42	0.001234	example.com	1	C_INTERNET	1	A	0	NOERROR	F	F	T	T	0	93.184.216.34	86400.000000	F"#;

    const HTTP_LOG_TSV: &str = r#"#separator \x09
#set_separator	,
#empty_field	(empty)
#unset_field	-
#path	http
#fields	ts	uid	id.orig_h	id.orig_p	id.resp_h	id.resp_p	method	host	uri	user_agent	status_code	resp_mime_types
#types	time	string	addr	port	addr	port	string	string	string	string	count	set[string]
1700000000.222222	HTTP1234	192.168.1.100	45678	10.0.0.1	80	GET	example.com	/index.html	Mozilla/5.0	200	text/html"#;

    const SSL_LOG_TSV: &str = r#"#separator \x09
#set_separator	,
#empty_field	(empty)
#unset_field	-
#path	ssl
#fields	ts	uid	id.orig_h	id.orig_p	id.resp_h	id.resp_p	version	cipher	server_name	resumed	established
#types	time	string	addr	port	addr	port	string	string	string	bool	bool
1700000000.333333	SSL5678	192.168.1.100	44444	10.0.0.3	443	TLSv13	TLS_AES_256_GCM_SHA384	secure.example.com	F	T"#;

    #[test]
    fn test_parse_zeek_separator() {
        assert_eq!(parse_zeek_separator("\\x09"), '\t');
        assert_eq!(parse_zeek_separator("\\x2c"), ',');
        assert_eq!(parse_zeek_separator(","), ',');
    }

    #[test]
    fn test_parse_zeek_timestamp() {
        let ts = parse_zeek_timestamp("1700000000.123456").unwrap();
        assert_eq!(ts.timestamp(), 1700000000);
    }

    #[test]
    fn test_parse_conn_log() {
        let records = parse_zeek_tsv(CONN_LOG_TSV).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].log_type, ZeekLogType::Conn);
        assert_eq!(records[0].fields["uid"], json!("CYfGSH4BHpKEhGLpf"));
        assert_eq!(records[0].fields["id.orig_h"], json!("192.168.1.100"));
        assert_eq!(records[0].fields["id.resp_p"], json!(80));
        assert_eq!(records[0].fields["proto"], json!("tcp"));
    }

    #[test]
    fn test_parse_dns_log() {
        let records = parse_zeek_tsv(DNS_LOG_TSV).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].log_type, ZeekLogType::Dns);
        assert_eq!(records[0].fields["query"], json!("example.com"));
        assert_eq!(records[0].fields["qtype_name"], json!("A"));
        assert_eq!(records[0].fields["rcode_name"], json!("NOERROR"));
    }

    #[test]
    fn test_parse_http_log() {
        let records = parse_zeek_tsv(HTTP_LOG_TSV).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].log_type, ZeekLogType::Http);
        assert_eq!(records[0].fields["method"], json!("GET"));
        assert_eq!(records[0].fields["host"], json!("example.com"));
        assert_eq!(records[0].fields["status_code"], json!(200));
    }

    #[test]
    fn test_parse_ssl_log() {
        let records = parse_zeek_tsv(SSL_LOG_TSV).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].log_type, ZeekLogType::Ssl);
        assert_eq!(records[0].fields["version"], json!("TLSv13"));
        assert_eq!(records[0].fields["server_name"], json!("secure.example.com"));
        assert_eq!(records[0].fields["established"], json!(true));
    }

    #[test]
    fn test_parse_zeek_json_conn() {
        let json_line = r#"{"ts":1700000000.123456,"uid":"CYfGSH4BHpKEhGLpf","id.orig_h":"192.168.1.100","id.orig_p":52134,"id.resp_h":"10.0.0.1","id.resp_p":80,"proto":"tcp","service":"http","duration":1.234567,"orig_bytes":512,"resp_bytes":2048}"#;
        let records = parse_zeek_json(json_line, &ZeekLogType::Conn);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].log_type, ZeekLogType::Conn);
        assert!(records[0].timestamp.is_some());
    }

    #[test]
    fn test_zeek_record_to_source_event() {
        let records = parse_zeek_tsv(CONN_LOG_TSV).unwrap();
        let event = zeek_record_to_source_event(&records[0], "zeek-test");
        assert_eq!(event.entity_type, "network_event");
        assert_eq!(event.entity_id, "zeek:CYfGSH4BHpKEhGLpf");
        assert_eq!(event.connector_id, "zeek-test");
        assert_eq!(event.properties["zeek_log_type"], json!("conn"));
    }

    #[test]
    fn test_entity_id_without_uid() {
        let record = ZeekRecord {
            log_type: ZeekLogType::Files,
            fields: {
                let mut m = HashMap::new();
                m.insert("id.orig_h".into(), json!("192.168.1.1"));
                m.insert("id.resp_h".into(), json!("10.0.0.1"));
                m
            },
            timestamp: None,
        };
        let event = zeek_record_to_source_event(&record, "zeek-test");
        assert_eq!(event.entity_id, "zeek:192.168.1.1-10.0.0.1");
    }

    #[test]
    fn test_zeek_log_type_from_path() {
        assert_eq!(ZeekLogType::from_path("/var/log/zeek/conn.log"), ZeekLogType::Conn);
        assert_eq!(ZeekLogType::from_path("dns.log"), ZeekLogType::Dns);
        assert_eq!(ZeekLogType::from_path("http"), ZeekLogType::Http);
        assert_eq!(ZeekLogType::from_path("ssl.log"), ZeekLogType::Ssl);
        assert_eq!(ZeekLogType::from_path("files.log"), ZeekLogType::Files);
        assert_eq!(ZeekLogType::from_path("weird.json"), ZeekLogType::Weird);
    }

    #[test]
    fn test_parse_zeek_tsv_no_header() {
        let result = parse_zeek_tsv("no header line here");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_zeek_tsv_header_only() {
        let header_only = r#"#separator \x09
#set_separator	,
#empty_field	(empty)
#unset_field	-
#path	conn
#fields	ts	uid
#types	time	string"#;
        let records = parse_zeek_tsv(header_only).unwrap();
        assert_eq!(records.len(), 0);
    }

    #[test]
    fn test_unset_fields_skipped() {
        let content = r#"#separator \x09
#set_separator	,
#empty_field	(empty)
#unset_field	-
#path	conn
#fields	ts	uid	service
#types	time	string	string
1700000000.0	ABC123	-"#;
        let records = parse_zeek_tsv(content).unwrap();
        assert_eq!(records.len(), 1);
        // The unset "service" field should not be in the map
        assert!(!records[0].fields.contains_key("service"));
    }

    #[test]
    fn test_zeek_connector_id() {
        let config = ConnectorConfig {
            connector_id: "zeek-1".to_string(),
            connector_type: "zeek".to_string(),
            url: None,
            entity_type: "network_event".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = ZeekConnector::new(config);
        assert_eq!(connector.connector_id(), "zeek-1");
    }

    #[tokio::test]
    async fn test_zeek_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "zeek-health".to_string(),
            connector_type: "zeek".to_string(),
            url: None,
            entity_type: "network_event".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = ZeekConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_files_log_tsv() {
        let content = r#"#separator \x09
#set_separator	,
#empty_field	(empty)
#unset_field	-
#path	files
#fields	ts	fuid	source	filename	mime_type	total_bytes	md5	sha1
#types	time	string	string	string	string	count	string	string
1700000000.0	F12345	HTTP	report.pdf	application/pdf	102400	d41d8cd98f00b204e9800998ecf8427e	da39a3ee5e6b4b0d3255bfef95601890afd80709"#;
        let records = parse_zeek_tsv(content).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].log_type, ZeekLogType::Files);
        assert_eq!(records[0].fields["mime_type"], json!("application/pdf"));
        assert_eq!(records[0].fields["total_bytes"], json!(102400));
    }
}
