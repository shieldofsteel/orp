use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// OPC-UA data model
// ---------------------------------------------------------------------------
// OPC-UA (OPC Unified Architecture) is the standard for industrial automation
// data exchange. A server exposes an address space of nodes. Clients subscribe
// to monitored items (nodes) and receive data change notifications.
//
// Key concepts:
//   - Node: identified by NodeId (namespace index + identifier)
//   - Variable: node that holds a value (the most common type for sensors)
//   - DataValue: value + status code + timestamps (source + server)
//   - MonitoredItem: subscription to a node's value changes
//
// This module implements the OPC-UA data model parsing and mapping to ORP
// entities. For the actual OPC-UA transport, a production deployment would
// use the `opcua` crate. This connector focuses on data normalization.

/// OPC-UA Node Identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId {
    pub namespace: u16,
    pub identifier: NodeIdentifier,
}

/// Node identifier variant.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeIdentifier {
    Numeric(u32),
    String(String),
    Guid(String),
    ByteString(Vec<u8>),
}

impl NodeId {
    pub fn numeric(namespace: u16, id: u32) -> Self {
        Self {
            namespace,
            identifier: NodeIdentifier::Numeric(id),
        }
    }

    pub fn string(namespace: u16, id: impl Into<String>) -> Self {
        Self {
            namespace,
            identifier: NodeIdentifier::String(id.into()),
        }
    }

    /// Format as OPC-UA standard string representation.
    pub fn to_string_id(&self) -> String {
        match &self.identifier {
            NodeIdentifier::Numeric(n) => format!("ns={};i={}", self.namespace, n),
            NodeIdentifier::String(s) => format!("ns={};s={}", self.namespace, s),
            NodeIdentifier::Guid(g) => format!("ns={};g={}", self.namespace, g),
            NodeIdentifier::ByteString(b) => {
                let hex: String = b.iter().map(|byte| format!("{:02X}", byte)).collect();
                format!("ns={};b={}", self.namespace, hex)
            }
        }
    }

    /// Parse from standard OPC-UA string representation.
    pub fn from_string(s: &str) -> Result<Self, ConnectorError> {
        let parts: Vec<&str> = s.split(';').collect();
        if parts.len() != 2 {
            return Err(ConnectorError::ParseError(format!(
                "OPC-UA: invalid NodeId format: {}",
                s
            )));
        }

        let namespace = parts[0]
            .strip_prefix("ns=")
            .ok_or_else(|| {
                ConnectorError::ParseError(format!(
                    "OPC-UA: NodeId missing ns= prefix: {}",
                    s
                ))
            })?
            .parse::<u16>()
            .map_err(|e| {
                ConnectorError::ParseError(format!(
                    "OPC-UA: invalid namespace index: {}",
                    e
                ))
            })?;

        let id_part = parts[1];
        if let Some(num_str) = id_part.strip_prefix("i=") {
            let num = num_str.parse::<u32>().map_err(|e| {
                ConnectorError::ParseError(format!(
                    "OPC-UA: invalid numeric id: {}",
                    e
                ))
            })?;
            Ok(NodeId::numeric(namespace, num))
        } else if let Some(str_id) = id_part.strip_prefix("s=") {
            Ok(NodeId::string(namespace, str_id))
        } else if let Some(guid) = id_part.strip_prefix("g=") {
            Ok(NodeId {
                namespace,
                identifier: NodeIdentifier::Guid(guid.to_string()),
            })
        } else {
            Err(ConnectorError::ParseError(format!(
                "OPC-UA: unknown identifier type in NodeId: {}",
                s
            )))
        }
    }
}

/// OPC-UA status code (subset of common codes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StatusCode {
    Good,
    Uncertain,
    Bad,
    BadNodeIdUnknown,
    BadTimeout,
    BadCommunicationError,
    GoodClamped,
    Raw(u32),
}

impl StatusCode {
    pub fn from_u32(code: u32) -> Self {
        match code {
            0x0000_0000 => StatusCode::Good,
            0x4000_0000 => StatusCode::Uncertain,
            0x8000_0000 => StatusCode::Bad,
            0x8034_0000 => StatusCode::BadNodeIdUnknown,
            0x800A_0000 => StatusCode::BadTimeout,
            0x8005_0000 => StatusCode::BadCommunicationError,
            0x0030_0000 => StatusCode::GoodClamped,
            _ => StatusCode::Raw(code),
        }
    }

    pub fn is_good(&self) -> bool {
        matches!(self, StatusCode::Good | StatusCode::GoodClamped)
    }

    pub fn as_str(&self) -> &str {
        match self {
            StatusCode::Good => "Good",
            StatusCode::Uncertain => "Uncertain",
            StatusCode::Bad => "Bad",
            StatusCode::BadNodeIdUnknown => "BadNodeIdUnknown",
            StatusCode::BadTimeout => "BadTimeout",
            StatusCode::BadCommunicationError => "BadCommunicationError",
            StatusCode::GoodClamped => "GoodClamped",
            StatusCode::Raw(_) => "Unknown",
        }
    }
}

/// OPC-UA variant value types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OpcValue {
    Boolean(bool),
    SByte(i8),
    Byte(u8),
    Int16(i16),
    UInt16(u16),
    Int32(i32),
    UInt32(u32),
    Int64(i64),
    UInt64(u64),
    Float(f32),
    Double(f64),
    String(String),
    DateTime(String),
    Null,
}

impl OpcValue {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            OpcValue::Boolean(v) => serde_json::json!(v),
            OpcValue::SByte(v) => serde_json::json!(v),
            OpcValue::Byte(v) => serde_json::json!(v),
            OpcValue::Int16(v) => serde_json::json!(v),
            OpcValue::UInt16(v) => serde_json::json!(v),
            OpcValue::Int32(v) => serde_json::json!(v),
            OpcValue::UInt32(v) => serde_json::json!(v),
            OpcValue::Int64(v) => serde_json::json!(v),
            OpcValue::UInt64(v) => serde_json::json!(v),
            OpcValue::Float(v) => serde_json::json!(v),
            OpcValue::Double(v) => serde_json::json!(v),
            OpcValue::String(v) => serde_json::json!(v),
            OpcValue::DateTime(v) => serde_json::json!(v),
            OpcValue::Null => serde_json::Value::Null,
        }
    }

    /// Auto-detect numeric value as f64 (for sensor readings).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            OpcValue::Float(v) => Some(*v as f64),
            OpcValue::Double(v) => Some(*v),
            OpcValue::Int16(v) => Some(*v as f64),
            OpcValue::UInt16(v) => Some(*v as f64),
            OpcValue::Int32(v) => Some(*v as f64),
            OpcValue::UInt32(v) => Some(*v as f64),
            OpcValue::Int64(v) => Some(*v as f64),
            OpcValue::UInt64(v) => Some(*v as f64),
            OpcValue::SByte(v) => Some(*v as f64),
            OpcValue::Byte(v) => Some(*v as f64),
            _ => None,
        }
    }
}

/// A data change notification from OPC-UA monitored item.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpcDataChange {
    pub node_id: NodeId,
    pub display_name: Option<String>,
    pub value: OpcValue,
    pub status: StatusCode,
    pub source_timestamp: Option<DateTime<Utc>>,
    pub server_timestamp: Option<DateTime<Utc>>,
}

/// OPC-UA node configuration for monitoring.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonitoredNode {
    pub node_id: NodeId,
    pub display_name: String,
    pub entity_type: String,
    pub sampling_interval_ms: u32,
    /// Optional physical location of the sensor.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// Auto-detect ORP entity type from OPC-UA node path or display name.
pub fn auto_detect_entity_type(display_name: &str, node_id: &NodeId) -> String {
    let name_lower = display_name.to_lowercase();
    let id_str = node_id.to_string_id().to_lowercase();

    if name_lower.contains("temperature") || name_lower.contains("temp") {
        "temperature_sensor".to_string()
    } else if name_lower.contains("pressure") {
        "pressure_sensor".to_string()
    } else if name_lower.contains("flow") {
        "flow_sensor".to_string()
    } else if name_lower.contains("level") {
        "level_sensor".to_string()
    } else if name_lower.contains("valve") {
        "valve".to_string()
    } else if name_lower.contains("pump") {
        "pump".to_string()
    } else if name_lower.contains("motor") {
        "motor".to_string()
    } else if name_lower.contains("alarm") || name_lower.contains("alert") {
        "alarm".to_string()
    } else if name_lower.contains("speed") || name_lower.contains("rpm") {
        "speed_sensor".to_string()
    } else if name_lower.contains("humidity") {
        "humidity_sensor".to_string()
    } else if name_lower.contains("power") || name_lower.contains("watt") {
        "power_meter".to_string()
    } else if name_lower.contains("voltage") || name_lower.contains("volt") {
        "voltage_sensor".to_string()
    } else if name_lower.contains("current") || name_lower.contains("ampere") {
        "current_sensor".to_string()
    } else if id_str.contains("plc") || id_str.contains("controller") {
        "plc".to_string()
    } else {
        "sensor".to_string()
    }
}

// ---------------------------------------------------------------------------
// OpcDataChange → SourceEvent
// ---------------------------------------------------------------------------

impl OpcDataChange {
    /// Convert to ORP SourceEvent.
    pub fn to_source_event(
        &self,
        connector_id: &str,
        monitored: Option<&MonitoredNode>,
    ) -> SourceEvent {
        let entity_type = monitored
            .map(|m| m.entity_type.clone())
            .unwrap_or_else(|| {
                auto_detect_entity_type(
                    self.display_name.as_deref().unwrap_or(""),
                    &self.node_id,
                )
            });

        let mut properties: HashMap<String, serde_json::Value> = HashMap::new();
        properties.insert(
            "node_id".into(),
            serde_json::json!(self.node_id.to_string_id()),
        );
        properties.insert("value".into(), self.value.to_json());
        properties.insert(
            "status".into(),
            serde_json::json!(self.status.as_str()),
        );
        properties.insert(
            "status_good".into(),
            serde_json::json!(self.status.is_good()),
        );

        if let Some(ref name) = self.display_name {
            properties.insert("display_name".into(), serde_json::json!(name));
        }
        if let Some(numeric) = self.value.as_f64() {
            properties.insert(
                "numeric_value".into(),
                serde_json::json!(numeric),
            );
        }
        if let Some(ref ts) = self.source_timestamp {
            properties.insert(
                "source_timestamp".into(),
                serde_json::json!(ts.to_rfc3339()),
            );
        }

        let ts = self.source_timestamp.unwrap_or_else(Utc::now);
        let (lat, lon) = monitored
            .map(|m| (m.latitude, m.longitude))
            .unwrap_or((None, None));

        SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id: format!("opcua:{}", self.node_id.to_string_id()),
            entity_type,
            properties,
            timestamp: ts,
            latitude: lat,
            longitude: lon,
        }
    }
}

// ---------------------------------------------------------------------------
// JSON-based data ingest (for OPC-UA gateway/proxy mode)
// ---------------------------------------------------------------------------

/// Parse an OPC-UA data change from a JSON payload (e.g. from an OPC-UA
/// to MQTT/REST gateway like Prosys, Kepware, or Node-RED).
pub fn parse_opcua_json(json: &str) -> Result<OpcDataChange, ConnectorError> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        ConnectorError::ParseError(format!("OPC-UA JSON parse error: {}", e))
    })?;

    let node_id_str = v
        .get("NodeId")
        .or_else(|| v.get("nodeId"))
        .or_else(|| v.get("node_id"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| {
            ConnectorError::ParseError(
                "OPC-UA JSON: missing NodeId field".to_string(),
            )
        })?;

    let node_id = NodeId::from_string(node_id_str)?;

    let display_name = v
        .get("DisplayName")
        .or_else(|| v.get("displayName"))
        .or_else(|| v.get("display_name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    let value = parse_opc_value(&v);

    let status_raw = v
        .get("StatusCode")
        .or_else(|| v.get("statusCode"))
        .or_else(|| v.get("status"))
        .and_then(|s| s.as_u64())
        .unwrap_or(0) as u32;

    let source_ts = v
        .get("SourceTimestamp")
        .or_else(|| v.get("sourceTimestamp"))
        .or_else(|| v.get("source_timestamp"))
        .and_then(|s| s.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    let server_ts = v
        .get("ServerTimestamp")
        .or_else(|| v.get("serverTimestamp"))
        .and_then(|s| s.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Ok(OpcDataChange {
        node_id,
        display_name,
        value,
        status: StatusCode::from_u32(status_raw),
        source_timestamp: source_ts,
        server_timestamp: server_ts,
    })
}

fn parse_opc_value(v: &serde_json::Value) -> OpcValue {
    let val = v
        .get("Value")
        .or_else(|| v.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    match &val {
        serde_json::Value::Bool(b) => OpcValue::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                    OpcValue::Int32(i as i32)
                } else {
                    OpcValue::Int64(i)
                }
            } else if let Some(f) = n.as_f64() {
                OpcValue::Double(f)
            } else {
                OpcValue::Null
            }
        }
        serde_json::Value::String(s) => OpcValue::String(s.clone()),
        serde_json::Value::Null => OpcValue::Null,
        _ => OpcValue::String(val.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// OPC-UA connector — subscribes to OPC-UA server nodes or ingests from
/// OPC-UA gateway JSON feeds.
pub struct OpcuaConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl OpcuaConnector {
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
impl Connector for OpcuaConnector {
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
            "OPC-UA connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let connector_id = self.config.connector_id.clone();
        let url = self.config.url.clone();
        let props = self.config.properties.clone();

        tokio::spawn(async move {
            // If a REST/JSON gateway URL is configured, poll it
            if let Some(ref base_url) = url {
                if base_url.starts_with("http") {
                    let client = reqwest::Client::new();
                    let poll_secs = props
                        .get("poll_interval_secs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(5);

                    let mut interval = tokio::time::interval(
                        tokio::time::Duration::from_secs(poll_secs),
                    );

                    while running.load(Ordering::SeqCst) {
                        interval.tick().await;
                        match client.get(base_url.as_str()).send().await {
                            Ok(resp) => match resp.text().await {
                                Ok(body) => {
                                    // Try array of data changes
                                    if let Ok(changes) = serde_json::from_str::<
                                        Vec<serde_json::Value>,
                                    >(
                                        &body
                                    ) {
                                        for change_json in changes {
                                            let json_str = change_json.to_string();
                                            match parse_opcua_json(&json_str) {
                                                Ok(dc) => {
                                                    let event = dc.to_source_event(
                                                        &connector_id,
                                                        None,
                                                    );
                                                    if tx.send(event).await.is_err()
                                                    {
                                                        return;
                                                    }
                                                    events_count.fetch_add(
                                                        1,
                                                        Ordering::Relaxed,
                                                    );
                                                }
                                                Err(e) => {
                                                    tracing::warn!(
                                                        "OPC-UA parse error: {}",
                                                        e
                                                    );
                                                    errors_count.fetch_add(
                                                        1,
                                                        Ordering::Relaxed,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "OPC-UA response error: {}",
                                        e
                                    );
                                    errors_count
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                            },
                            Err(e) => {
                                tracing::warn!("OPC-UA request error: {}", e);
                                errors_count
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    return;
                }
            }

            // Demo mode: idle
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
            "OPC-UA connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "OPC-UA connector not running".to_string(),
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

    #[test]
    fn test_node_id_numeric() {
        let nid = NodeId::numeric(2, 1001);
        assert_eq!(nid.to_string_id(), "ns=2;i=1001");
    }

    #[test]
    fn test_node_id_string() {
        let nid = NodeId::string(3, "Temperature.PV");
        assert_eq!(nid.to_string_id(), "ns=3;s=Temperature.PV");
    }

    #[test]
    fn test_node_id_roundtrip_numeric() {
        let nid = NodeId::numeric(0, 85);
        let s = nid.to_string_id();
        let parsed = NodeId::from_string(&s).unwrap();
        assert_eq!(nid, parsed);
    }

    #[test]
    fn test_node_id_roundtrip_string() {
        let nid = NodeId::string(2, "Boiler.Temperature");
        let s = nid.to_string_id();
        let parsed = NodeId::from_string(&s).unwrap();
        assert_eq!(nid, parsed);
    }

    #[test]
    fn test_node_id_parse_guid() {
        let nid =
            NodeId::from_string("ns=1;g=550e8400-e29b-41d4-a716-446655440000")
                .unwrap();
        assert_eq!(nid.namespace, 1);
        assert!(matches!(nid.identifier, NodeIdentifier::Guid(_)));
    }

    #[test]
    fn test_node_id_parse_invalid() {
        assert!(NodeId::from_string("invalid").is_err());
        assert!(NodeId::from_string("ns=x;i=1").is_err());
        assert!(NodeId::from_string("ns=1;z=bad").is_err());
    }

    #[test]
    fn test_status_code_good() {
        assert!(StatusCode::from_u32(0).is_good());
        assert!(!StatusCode::from_u32(0x8000_0000).is_good());
    }

    #[test]
    fn test_status_code_strings() {
        assert_eq!(StatusCode::Good.as_str(), "Good");
        assert_eq!(StatusCode::BadTimeout.as_str(), "BadTimeout");
        assert_eq!(StatusCode::Raw(0xFFFF).as_str(), "Unknown");
    }

    #[test]
    fn test_opc_value_to_json() {
        assert_eq!(
            OpcValue::Double(23.5).to_json(),
            serde_json::json!(23.5)
        );
        assert_eq!(OpcValue::Boolean(true).to_json(), serde_json::json!(true));
        assert_eq!(
            OpcValue::String("test".into()).to_json(),
            serde_json::json!("test")
        );
        assert_eq!(OpcValue::Null.to_json(), serde_json::Value::Null);
    }

    #[test]
    fn test_opc_value_as_f64() {
        assert_eq!(OpcValue::Double(23.5).as_f64(), Some(23.5));
        assert_eq!(OpcValue::Int32(42).as_f64(), Some(42.0));
        assert_eq!(OpcValue::Float(3.25).as_f64(), Some(3.25));
        assert_eq!(OpcValue::Boolean(true).as_f64(), None);
        assert_eq!(OpcValue::Null.as_f64(), None);
    }

    #[test]
    fn test_parse_opcua_json() {
        let json = r#"{
            "NodeId": "ns=2;s=Temperature.PV",
            "DisplayName": "Boiler Temperature",
            "Value": 85.3,
            "StatusCode": 0,
            "SourceTimestamp": "2026-03-26T12:00:00Z"
        }"#;
        let dc = parse_opcua_json(json).unwrap();
        assert_eq!(dc.node_id, NodeId::string(2, "Temperature.PV"));
        assert_eq!(dc.display_name, Some("Boiler Temperature".into()));
        assert_eq!(dc.value, OpcValue::Double(85.3));
        assert!(dc.status.is_good());
        assert!(dc.source_timestamp.is_some());
    }

    #[test]
    fn test_parse_opcua_json_camelcase() {
        let json = r#"{
            "nodeId": "ns=2;i=1001",
            "displayName": "Pressure Sensor",
            "value": 101325,
            "statusCode": 0
        }"#;
        let dc = parse_opcua_json(json).unwrap();
        assert_eq!(dc.node_id, NodeId::numeric(2, 1001));
        assert_eq!(dc.value, OpcValue::Int32(101325));
    }

    #[test]
    fn test_parse_opcua_json_boolean_value() {
        let json = r#"{
            "NodeId": "ns=2;s=Valve.Open",
            "Value": true,
            "StatusCode": 0
        }"#;
        let dc = parse_opcua_json(json).unwrap();
        assert_eq!(dc.value, OpcValue::Boolean(true));
    }

    #[test]
    fn test_parse_opcua_json_missing_node_id() {
        let json = r#"{"Value": 42}"#;
        assert!(parse_opcua_json(json).is_err());
    }

    #[test]
    fn test_data_change_to_source_event() {
        let dc = OpcDataChange {
            node_id: NodeId::string(2, "Temperature.PV"),
            display_name: Some("Boiler Temperature".to_string()),
            value: OpcValue::Double(85.3),
            status: StatusCode::Good,
            source_timestamp: Some(Utc::now()),
            server_timestamp: None,
        };
        let event = dc.to_source_event("opcua-test", None);
        assert_eq!(event.entity_type, "temperature_sensor");
        assert_eq!(event.entity_id, "opcua:ns=2;s=Temperature.PV");
        assert!(event.properties.contains_key("numeric_value"));
    }

    #[test]
    fn test_data_change_with_monitored_node() {
        let dc = OpcDataChange {
            node_id: NodeId::numeric(2, 500),
            display_name: Some("Custom Sensor".to_string()),
            value: OpcValue::Double(42.0),
            status: StatusCode::Good,
            source_timestamp: None,
            server_timestamp: None,
        };
        let monitored = MonitoredNode {
            node_id: NodeId::numeric(2, 500),
            display_name: "Custom Sensor".to_string(),
            entity_type: "custom_device".to_string(),
            sampling_interval_ms: 1000,
            latitude: Some(51.5),
            longitude: Some(-0.1),
        };
        let event = dc.to_source_event("opcua-test", Some(&monitored));
        assert_eq!(event.entity_type, "custom_device");
        assert_eq!(event.latitude, Some(51.5));
        assert_eq!(event.longitude, Some(-0.1));
    }

    #[test]
    fn test_auto_detect_entity_type() {
        let nid = NodeId::numeric(0, 1);
        assert_eq!(
            auto_detect_entity_type("Boiler Temperature PV", &nid),
            "temperature_sensor"
        );
        assert_eq!(
            auto_detect_entity_type("Main Pressure Gauge", &nid),
            "pressure_sensor"
        );
        assert_eq!(
            auto_detect_entity_type("Flow Meter 1", &nid),
            "flow_sensor"
        );
        assert_eq!(
            auto_detect_entity_type("Tank Level", &nid),
            "level_sensor"
        );
        assert_eq!(
            auto_detect_entity_type("Safety Valve 3", &nid),
            "valve"
        );
        assert_eq!(
            auto_detect_entity_type("Feed Pump", &nid),
            "pump"
        );
        assert_eq!(
            auto_detect_entity_type("Generic Sensor XYZ", &nid),
            "sensor"
        );
    }

    #[test]
    fn test_auto_detect_power_voltage_current() {
        let nid = NodeId::numeric(0, 1);
        assert_eq!(
            auto_detect_entity_type("Power Meter", &nid),
            "power_meter"
        );
        assert_eq!(
            auto_detect_entity_type("Voltage Sensor", &nid),
            "voltage_sensor"
        );
        assert_eq!(
            auto_detect_entity_type("Current Draw Ampere", &nid),
            "current_sensor"
        );
    }

    #[test]
    fn test_opcua_connector_id() {
        let config = ConnectorConfig {
            connector_id: "opcua-1".to_string(),
            connector_type: "opcua".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = OpcuaConnector::new(config);
        assert_eq!(connector.connector_id(), "opcua-1");
    }

    #[tokio::test]
    async fn test_opcua_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "opcua-health".to_string(),
            connector_type: "opcua".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = OpcuaConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_bad_status_in_event() {
        let dc = OpcDataChange {
            node_id: NodeId::numeric(2, 100),
            display_name: Some("Sensor".to_string()),
            value: OpcValue::Null,
            status: StatusCode::BadCommunicationError,
            source_timestamp: None,
            server_timestamp: None,
        };
        let event = dc.to_source_event("opcua-test", None);
        assert_eq!(
            event.properties.get("status").unwrap(),
            &serde_json::json!("BadCommunicationError")
        );
        assert_eq!(
            event.properties.get("status_good").unwrap(),
            &serde_json::json!(false)
        );
    }
}
