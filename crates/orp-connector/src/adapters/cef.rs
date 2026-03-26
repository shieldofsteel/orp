use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CEF (Common Event Format) dedicated parser
// ---------------------------------------------------------------------------
// CEF is the dominant event format used by SIEM platforms: ArcSight, Splunk,
// IBM QRadar, and many security appliances (firewalls, IDS/IPS, WAF).
//
// Format:
//   CEF:Version|Device Vendor|Device Product|Device Version|Signature ID|Name|Severity|Extensions
//
// Rules:
//   - Fields are pipe-delimited (`|`)
//   - Pipes within field values are escaped as `\|`
//   - Backslashes are escaped as `\\`
//   - Newlines in values are escaped as `\n`
//   - Extensions are key=value pairs separated by spaces
//   - Severity: 0-3 Low, 4-6 Medium, 7-8 High, 9-10 Very-High
//     (can also be text: "Low", "Medium", "High", "Very-High")
//
// Common extension keys:
//   src, dst, spt, dpt, proto, act, msg, rt (receipt time),
//   cs1-cs6 (custom strings), cs1Label-cs6Label,
//   cn1-cn3 (custom numbers), cn1Label-cn3Label,
//   deviceCustomDate1, deviceCustomDate2,
//   deviceExternalId, dvchost, dvc (device address),
//   fname (file name), fsize (file size), request (URL),
//   suser (source user), duser (dest user),
//   cat (category), outcome, reason

/// CEF severity level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CefSeverity {
    Low,
    Medium,
    High,
    VeryHigh,
    Unknown,
}

impl CefSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            CefSeverity::Low => "Low",
            CefSeverity::Medium => "Medium",
            CefSeverity::High => "High",
            CefSeverity::VeryHigh => "Very-High",
            CefSeverity::Unknown => "Unknown",
        }
    }

    /// Numeric value (0–10) representative of the severity.
    pub fn numeric(&self) -> u8 {
        match self {
            CefSeverity::Low => 2,
            CefSeverity::Medium => 5,
            CefSeverity::High => 8,
            CefSeverity::VeryHigh => 10,
            CefSeverity::Unknown => 0,
        }
    }
}

/// CEF message header.
#[derive(Clone, Debug)]
pub struct CefHeader {
    pub version: u8,
    pub device_vendor: String,
    pub device_product: String,
    pub device_version: String,
    pub signature_id: String,
    pub name: String,
    pub severity: CefSeverity,
    pub severity_raw: String,
}

/// Parsed CEF message.
#[derive(Clone, Debug)]
pub struct CefMessage {
    pub header: CefHeader,
    pub extensions: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Unescape CEF field value: `\|` → `|`, `\\` → `\`, `\n` → newline.
pub fn unescape_cef_field(field: &str) -> String {
    let mut result = String::with_capacity(field.len());
    let mut chars = field.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek() {
                Some('|') => {
                    result.push('|');
                    chars.next();
                }
                Some('\\') => {
                    result.push('\\');
                    chars.next();
                }
                Some('n') => {
                    result.push('\n');
                    chars.next();
                }
                Some('r') => {
                    result.push('\r');
                    chars.next();
                }
                Some('=') => {
                    result.push('=');
                    chars.next();
                }
                _ => {
                    result.push('\\');
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Parse CEF severity from string (numeric or text).
pub fn parse_cef_severity(s: &str) -> CefSeverity {
    let s = s.trim();

    // Try numeric
    if let Ok(n) = s.parse::<u8>() {
        return match n {
            0..=3 => CefSeverity::Low,
            4..=6 => CefSeverity::Medium,
            7..=8 => CefSeverity::High,
            9..=10 => CefSeverity::VeryHigh,
            _ => CefSeverity::Unknown,
        };
    }

    // Try text
    match s.to_lowercase().as_str() {
        "low" => CefSeverity::Low,
        "medium" => CefSeverity::Medium,
        "high" => CefSeverity::High,
        "very-high" | "veryhigh" | "very high" | "critical" => CefSeverity::VeryHigh,
        _ => CefSeverity::Unknown,
    }
}

/// Split CEF header fields respecting `\|` escapes.
fn split_cef_header(s: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if chars.peek() == Some(&'|') {
                current.push('\\');
                current.push('|');
                chars.next();
            } else {
                current.push(ch);
            }
        } else if ch == '|' {
            fields.push(current.clone());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    // Last field (extensions) is everything after the last unescaped pipe
    fields.push(current);
    fields
}

/// Parse CEF key=value extension pairs.
///
/// Extensions format: `key1=value1 key2=value2 key3=value with spaces`
/// Values extend until the next `key=` pattern (where key is a known CEF key
/// or matches `[a-zA-Z0-9]+`).
pub fn parse_cef_extensions(ext_str: &str) -> HashMap<String, String> {
    let mut extensions = HashMap::new();
    let ext_str = ext_str.trim();
    if ext_str.is_empty() {
        return extensions;
    }

    // Strategy: find all `key=` positions, then extract values between them
    let mut key_positions: Vec<(usize, &str)> = Vec::new();
    let bytes = ext_str.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for a potential key: alphanumeric chars followed by '='
        if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i < len && bytes[i] == b'=' {
                let key = &ext_str[start..i];
                key_positions.push((i + 1, key)); // +1 to skip '='
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    // Extract values: each value extends from after '=' to the start of the
    // next key (minus the space before it)
    for (idx, &(value_start, key)) in key_positions.iter().enumerate() {
        let value_end = if idx + 1 < key_positions.len() {
            // Find the space before the next key
            let next_key_start = key_positions[idx + 1].0 - key_positions[idx + 1].1.len() - 1;
            // Walk backwards to skip whitespace
            let mut end = next_key_start;
            while end > value_start && ext_str.as_bytes()[end - 1] == b' ' {
                end -= 1;
            }
            end
        } else {
            ext_str.len()
        };

        if value_start <= value_end {
            let value = ext_str[value_start..value_end].trim();
            extensions.insert(key.to_string(), unescape_cef_field(value));
        }
    }

    extensions
}

/// Parse a CEF formatted line.
///
/// Format: `CEF:Version|Device Vendor|Device Product|Device Version|Signature ID|Name|Severity|Extensions`
pub fn parse_cef(line: &str) -> Result<CefMessage, ConnectorError> {
    let line = line.trim();

    // Must start with "CEF:"
    let cef_body = if let Some(body) = line.strip_prefix("CEF:") {
        body
    } else {
        return Err(ConnectorError::ParseError(
            "CEF: line must start with 'CEF:'".into(),
        ));
    };

    let fields = split_cef_header(cef_body);
    if fields.len() < 8 {
        return Err(ConnectorError::ParseError(format!(
            "CEF: expected 8 pipe-delimited fields, got {}",
            fields.len()
        )));
    }

    let version = fields[0].trim().parse::<u8>().unwrap_or(0);
    let device_vendor = unescape_cef_field(fields[1].trim());
    let device_product = unescape_cef_field(fields[2].trim());
    let device_version = unescape_cef_field(fields[3].trim());
    let signature_id = unescape_cef_field(fields[4].trim());
    let name = unescape_cef_field(fields[5].trim());
    let severity_raw = fields[6].trim().to_string();
    let severity = parse_cef_severity(&severity_raw);

    // Extensions: everything from field 7 onwards (rejoin in case pipe in extensions)
    let ext_str = if fields.len() > 7 {
        fields[7..].join("|")
    } else {
        String::new()
    };
    let extensions = parse_cef_extensions(&ext_str);

    Ok(CefMessage {
        header: CefHeader {
            version,
            device_vendor,
            device_product,
            device_version,
            signature_id,
            name,
            severity,
            severity_raw,
        },
        extensions,
    })
}

// ---------------------------------------------------------------------------
// CEF → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a CEF message to a SourceEvent.
pub fn cef_to_source_event(
    msg: &CefMessage,
    connector_id: &str,
) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert("cef_version".into(), json!(msg.header.version));
    properties.insert("device_vendor".into(), json!(msg.header.device_vendor));
    properties.insert("device_product".into(), json!(msg.header.device_product));
    properties.insert("device_version".into(), json!(msg.header.device_version));
    properties.insert("signature_id".into(), json!(msg.header.signature_id));
    properties.insert("event_name".into(), json!(msg.header.name));
    properties.insert("severity".into(), json!(msg.header.severity.as_str()));
    properties.insert("severity_numeric".into(), json!(msg.header.severity.numeric()));
    properties.insert("severity_raw".into(), json!(msg.header.severity_raw));

    // Copy all extensions into properties
    for (key, value) in &msg.extensions {
        properties.insert(format!("ext_{}", key), json!(value));
    }

    // Promote well-known extension keys
    if let Some(src) = msg.extensions.get("src") {
        properties.insert("source_address".into(), json!(src));
    }
    if let Some(dst) = msg.extensions.get("dst") {
        properties.insert("destination_address".into(), json!(dst));
    }
    if let Some(spt) = msg.extensions.get("spt") {
        properties.insert("source_port".into(), json!(spt));
    }
    if let Some(dpt) = msg.extensions.get("dpt") {
        properties.insert("destination_port".into(), json!(dpt));
    }
    if let Some(act) = msg.extensions.get("act") {
        properties.insert("action".into(), json!(act));
    }
    if let Some(msg_text) = msg.extensions.get("msg") {
        properties.insert("message".into(), json!(msg_text));
    }
    if let Some(cat) = msg.extensions.get("cat") {
        properties.insert("category".into(), json!(cat));
    }

    let entity_id = format!(
        "cef:{}:{}:{}",
        msg.header.device_vendor, msg.header.device_product, msg.header.signature_id
    );

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "security_event".to_string(),
        properties,
        timestamp: Utc::now(),
        latitude: None,
        longitude: None,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct CefConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl CefConnector {
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
impl Connector for CefConnector {
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
                ConnectorError::ConfigError("CEF: url (file path) required".into())
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

            match parse_cef(line) {
                Ok(msg) => {
                    let event = cef_to_source_event(&msg, &connector_id);
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
                "CEF connector is not running".into(),
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
    fn test_parse_cef_basic() {
        let line = "CEF:0|Security|Firewall|1.0|100|Connection blocked|5|src=10.0.0.1 dst=192.168.1.1";
        let msg = parse_cef(line).unwrap();
        assert_eq!(msg.header.version, 0);
        assert_eq!(msg.header.device_vendor, "Security");
        assert_eq!(msg.header.device_product, "Firewall");
        assert_eq!(msg.header.device_version, "1.0");
        assert_eq!(msg.header.signature_id, "100");
        assert_eq!(msg.header.name, "Connection blocked");
        assert_eq!(msg.header.severity, CefSeverity::Medium);
    }

    #[test]
    fn test_parse_cef_with_extensions() {
        let line = "CEF:0|Vendor|Product|1.0|SIG01|Test Event|3|src=10.0.0.1 dst=10.0.0.2 spt=1234 dpt=80 act=Allow";
        let msg = parse_cef(line).unwrap();
        assert_eq!(msg.extensions.get("src").unwrap(), "10.0.0.1");
        assert_eq!(msg.extensions.get("dst").unwrap(), "10.0.0.2");
        assert_eq!(msg.extensions.get("spt").unwrap(), "1234");
        assert_eq!(msg.extensions.get("dpt").unwrap(), "80");
        assert_eq!(msg.extensions.get("act").unwrap(), "Allow");
    }

    #[test]
    fn test_parse_cef_escaped_pipe() {
        let line = r"CEF:0|Vendor\|Inc|Product|1.0|100|Test|5|msg=Hello";
        let msg = parse_cef(line).unwrap();
        assert_eq!(msg.header.device_vendor, "Vendor|Inc");
    }

    #[test]
    fn test_parse_cef_escaped_backslash() {
        let line = r"CEF:0|Vendor|Product|1.0|100|Test\\Event|5|msg=test";
        let msg = parse_cef(line).unwrap();
        assert_eq!(msg.header.name, "Test\\Event");
    }

    #[test]
    fn test_parse_cef_severity_numeric() {
        assert_eq!(parse_cef_severity("0"), CefSeverity::Low);
        assert_eq!(parse_cef_severity("3"), CefSeverity::Low);
        assert_eq!(parse_cef_severity("4"), CefSeverity::Medium);
        assert_eq!(parse_cef_severity("6"), CefSeverity::Medium);
        assert_eq!(parse_cef_severity("7"), CefSeverity::High);
        assert_eq!(parse_cef_severity("8"), CefSeverity::High);
        assert_eq!(parse_cef_severity("9"), CefSeverity::VeryHigh);
        assert_eq!(parse_cef_severity("10"), CefSeverity::VeryHigh);
    }

    #[test]
    fn test_parse_cef_severity_text() {
        assert_eq!(parse_cef_severity("Low"), CefSeverity::Low);
        assert_eq!(parse_cef_severity("Medium"), CefSeverity::Medium);
        assert_eq!(parse_cef_severity("High"), CefSeverity::High);
        assert_eq!(parse_cef_severity("Very-High"), CefSeverity::VeryHigh);
        assert_eq!(parse_cef_severity("critical"), CefSeverity::VeryHigh);
        assert_eq!(parse_cef_severity("garbage"), CefSeverity::Unknown);
    }

    #[test]
    fn test_parse_cef_extensions_complex() {
        let ext = "src=10.0.0.1 msg=This is a longer message with spaces act=Block cs1=custom value cs1Label=MyLabel";
        let exts = parse_cef_extensions(ext);
        assert_eq!(exts.get("src").unwrap(), "10.0.0.1");
        assert_eq!(
            exts.get("msg").unwrap(),
            "This is a longer message with spaces"
        );
        assert_eq!(exts.get("act").unwrap(), "Block");
        assert_eq!(exts.get("cs1Label").unwrap(), "MyLabel");
    }

    #[test]
    fn test_parse_cef_extensions_empty() {
        let exts = parse_cef_extensions("");
        assert!(exts.is_empty());
    }

    #[test]
    fn test_cef_to_source_event() {
        let line = "CEF:0|Acme|WAF|2.0|XSS-01|Cross Site Scripting|8|src=10.0.0.1 dst=10.0.0.2 act=Block";
        let msg = parse_cef(line).unwrap();
        let event = cef_to_source_event(&msg, "cef-test");
        assert_eq!(event.entity_type, "security_event");
        assert_eq!(event.entity_id, "cef:Acme:WAF:XSS-01");
        assert_eq!(event.properties["severity"], json!("High"));
        assert_eq!(event.properties["source_address"], json!("10.0.0.1"));
        assert_eq!(event.properties["action"], json!("Block"));
    }

    #[test]
    fn test_cef_entity_type() {
        let line = "CEF:0|V|P|1|S|N|1|";
        let msg = parse_cef(line).unwrap();
        let event = cef_to_source_event(&msg, "test");
        assert_eq!(event.entity_type, "security_event");
    }

    #[test]
    fn test_parse_cef_invalid_format() {
        assert!(parse_cef("NOT CEF FORMAT").is_err());
        assert!(parse_cef("CEF:0|Only|Three|Fields").is_err());
    }

    #[test]
    fn test_parse_cef_version_0_and_1() {
        let v0 = "CEF:0|V|P|1.0|100|Test|5|src=1.2.3.4";
        let msg0 = parse_cef(v0).unwrap();
        assert_eq!(msg0.header.version, 0);

        let v1 = "CEF:1|V|P|1.0|100|Test|5|src=1.2.3.4";
        let msg1 = parse_cef(v1).unwrap();
        assert_eq!(msg1.header.version, 1);
    }

    #[test]
    fn test_cef_connector_id() {
        let config = ConnectorConfig {
            connector_id: "cef-1".to_string(),
            connector_type: "cef".to_string(),
            url: None,
            entity_type: "security_event".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = CefConnector::new(config);
        assert_eq!(connector.connector_id(), "cef-1");
    }

    #[tokio::test]
    async fn test_cef_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "cef-h".to_string(),
            connector_type: "cef".to_string(),
            url: None,
            entity_type: "security_event".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = CefConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_unescape_cef_field() {
        assert_eq!(unescape_cef_field(r"hello\|world"), "hello|world");
        assert_eq!(unescape_cef_field(r"back\\slash"), "back\\slash");
        assert_eq!(unescape_cef_field(r"new\nline"), "new\nline");
        assert_eq!(unescape_cef_field(r"equal\=sign"), "equal=sign");
        assert_eq!(unescape_cef_field("no escapes"), "no escapes");
    }

    #[test]
    fn test_cef_severity_methods() {
        assert_eq!(CefSeverity::Low.as_str(), "Low");
        assert_eq!(CefSeverity::Low.numeric(), 2);
        assert_eq!(CefSeverity::VeryHigh.as_str(), "Very-High");
        assert_eq!(CefSeverity::VeryHigh.numeric(), 10);
    }
}
