use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// SparkplugB over MQTT parser
// ---------------------------------------------------------------------------
// SparkplugB is an industrial IoT protocol built on MQTT.
//
// Topic namespace:
//   spBv1.0/{group_id}/{message_type}/{edge_node_id}[/{device_id}]
//
// Message types:
//   NBIRTH  — Edge node birth certificate (metric definitions)
//   NDEATH  — Edge node death
//   NDATA   — Edge node data (metric updates)
//   NCMD    — Edge node command
//   DBIRTH  — Device birth certificate
//   DDEATH  — Device death
//   DDATA   — Device data (metric updates)
//   DCMD    — Device command
//   STATE   — SCADA host state
//
// Payload: Protobuf-encoded Payload message with:
//   - timestamp (uint64, epoch ms)
//   - metrics[] (name, alias, timestamp, datatype, value)
//   - seq (uint64)
//   - uuid (optional)
//
// Datatype codes:
//   1=Int8, 2=Int16, 3=Int32, 4=Int64, 5=UInt8, 6=UInt16, 7=UInt32, 8=UInt64,
//   9=Float, 10=Double, 11=Boolean, 12=String, 13=DateTime, 14=Text, 15=UUID,
//   16=DataSet, 17=Bytes, 18=File, 19=Template
//
// This module parses SparkplugB from JSON representations (as often exposed
// by MQTT-to-JSON bridges or decoded by ORP's MQTT connector).

/// SparkplugB message type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SparkplugMessageType {
    NBirth,
    NDeath,
    NData,
    NCmd,
    DBirth,
    DDeath,
    DData,
    DCmd,
    State,
    Unknown(String),
}

impl SparkplugMessageType {
    pub fn parse(s: &str) -> Self {
        match s {
            "NBIRTH" => SparkplugMessageType::NBirth,
            "NDEATH" => SparkplugMessageType::NDeath,
            "NDATA" => SparkplugMessageType::NData,
            "NCMD" => SparkplugMessageType::NCmd,
            "DBIRTH" => SparkplugMessageType::DBirth,
            "DDEATH" => SparkplugMessageType::DDeath,
            "DDATA" => SparkplugMessageType::DData,
            "DCMD" => SparkplugMessageType::DCmd,
            "STATE" => SparkplugMessageType::State,
            _ => SparkplugMessageType::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            SparkplugMessageType::NBirth => "NBIRTH",
            SparkplugMessageType::NDeath => "NDEATH",
            SparkplugMessageType::NData => "NDATA",
            SparkplugMessageType::NCmd => "NCMD",
            SparkplugMessageType::DBirth => "DBIRTH",
            SparkplugMessageType::DDeath => "DDEATH",
            SparkplugMessageType::DData => "DDATA",
            SparkplugMessageType::DCmd => "DCMD",
            SparkplugMessageType::State => "STATE",
            SparkplugMessageType::Unknown(s) => s,
        }
    }

    pub fn is_birth(&self) -> bool {
        matches!(
            self,
            SparkplugMessageType::NBirth | SparkplugMessageType::DBirth
        )
    }

    pub fn is_death(&self) -> bool {
        matches!(
            self,
            SparkplugMessageType::NDeath | SparkplugMessageType::DDeath
        )
    }

    pub fn is_data(&self) -> bool {
        matches!(
            self,
            SparkplugMessageType::NData | SparkplugMessageType::DData
        )
    }
}

/// SparkplugB data type code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SparkplugDataType {
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float,
    Double,
    Boolean,
    String,
    DateTime,
    Text,
    Unknown(u32),
}

impl SparkplugDataType {
    pub fn from_code(code: u32) -> Self {
        match code {
            1 => SparkplugDataType::Int8,
            2 => SparkplugDataType::Int16,
            3 => SparkplugDataType::Int32,
            4 => SparkplugDataType::Int64,
            5 => SparkplugDataType::UInt8,
            6 => SparkplugDataType::UInt16,
            7 => SparkplugDataType::UInt32,
            8 => SparkplugDataType::UInt64,
            9 => SparkplugDataType::Float,
            10 => SparkplugDataType::Double,
            11 => SparkplugDataType::Boolean,
            12 => SparkplugDataType::String,
            13 => SparkplugDataType::DateTime,
            14 => SparkplugDataType::Text,
            _ => SparkplugDataType::Unknown(code),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SparkplugDataType::Int8 => "Int8",
            SparkplugDataType::Int16 => "Int16",
            SparkplugDataType::Int32 => "Int32",
            SparkplugDataType::Int64 => "Int64",
            SparkplugDataType::UInt8 => "UInt8",
            SparkplugDataType::UInt16 => "UInt16",
            SparkplugDataType::UInt32 => "UInt32",
            SparkplugDataType::UInt64 => "UInt64",
            SparkplugDataType::Float => "Float",
            SparkplugDataType::Double => "Double",
            SparkplugDataType::Boolean => "Boolean",
            SparkplugDataType::String => "String",
            SparkplugDataType::DateTime => "DateTime",
            SparkplugDataType::Text => "Text",
            SparkplugDataType::Unknown(_) => "Unknown",
        }
    }
}

/// A SparkplugB metric.
#[derive(Clone, Debug)]
pub struct SparkplugMetric {
    pub name: String,
    pub alias: Option<u64>,
    pub timestamp: Option<u64>,
    pub datatype: SparkplugDataType,
    pub value: JsonValue,
}

/// Parsed SparkplugB topic.
#[derive(Clone, Debug)]
pub struct SparkplugTopic {
    pub group_id: String,
    pub message_type: SparkplugMessageType,
    pub edge_node_id: String,
    pub device_id: Option<String>,
}

/// Parsed SparkplugB payload.
#[derive(Clone, Debug)]
pub struct SparkplugPayload {
    pub timestamp: Option<u64>,
    pub seq: Option<u64>,
    pub uuid: Option<String>,
    pub metrics: Vec<SparkplugMetric>,
}

/// Complete parsed SparkplugB message (topic + payload).
#[derive(Clone, Debug)]
pub struct SparkplugMessage {
    pub topic: SparkplugTopic,
    pub payload: SparkplugPayload,
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse a SparkplugB MQTT topic string.
pub fn parse_sparkplug_topic(topic: &str) -> Option<SparkplugTopic> {
    let parts: Vec<&str> = topic.split('/').collect();
    // spBv1.0/{group_id}/{message_type}/{edge_node_id}[/{device_id}]
    if parts.len() < 4 || parts[0] != "spBv1.0" {
        return None;
    }

    let group_id = parts[1].to_string();
    let message_type = SparkplugMessageType::parse(parts[2]);
    let edge_node_id = parts[3].to_string();
    let device_id = if parts.len() > 4 {
        Some(parts[4..].join("/"))
    } else {
        None
    };

    Some(SparkplugTopic {
        group_id,
        message_type,
        edge_node_id,
        device_id,
    })
}

/// Parse a SparkplugB JSON payload.
pub fn parse_sparkplug_payload_json(data: &str) -> Result<SparkplugPayload, ConnectorError> {
    let value: JsonValue = serde_json::from_str(data).map_err(|e| {
        ConnectorError::ParseError(format!("SparkplugB: invalid JSON: {}", e))
    })?;
    parse_sparkplug_payload_value(&value)
}

/// Parse a SparkplugB payload from a JSON Value.
pub fn parse_sparkplug_payload_value(
    value: &JsonValue,
) -> Result<SparkplugPayload, ConnectorError> {
    let timestamp = value.get("timestamp").and_then(|v| v.as_u64());
    let seq = value.get("seq").and_then(|v| v.as_u64());
    let uuid = value
        .get("uuid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let metrics = value
        .get("metrics")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(parse_metric_json)
                .collect()
        })
        .unwrap_or_default();

    Ok(SparkplugPayload {
        timestamp,
        seq,
        uuid,
        metrics,
    })
}

fn parse_metric_json(m: &JsonValue) -> Option<SparkplugMetric> {
    let name = m.get("name").and_then(|v| v.as_str())?.to_string();
    let alias = m.get("alias").and_then(|v| v.as_u64());
    let timestamp = m.get("timestamp").and_then(|v| v.as_u64());
    let datatype_code = m.get("datatype").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let datatype = SparkplugDataType::from_code(datatype_code);

    let value = extract_metric_value(m, &datatype);

    Some(SparkplugMetric {
        name,
        alias,
        timestamp,
        datatype,
        value,
    })
}

fn extract_metric_value(m: &JsonValue, _datatype: &SparkplugDataType) -> JsonValue {
    // SparkplugB JSON uses typed value fields:
    // intValue, longValue, floatValue, doubleValue, booleanValue, stringValue
    if let Some(v) = m.get("intValue").and_then(|v| v.as_i64()) {
        return json!(v);
    }
    if let Some(v) = m.get("longValue").and_then(|v| v.as_i64()) {
        return json!(v);
    }
    if let Some(v) = m.get("floatValue").and_then(|v| v.as_f64()) {
        return json!(v);
    }
    if let Some(v) = m.get("doubleValue").and_then(|v| v.as_f64()) {
        return json!(v);
    }
    if let Some(v) = m.get("booleanValue").and_then(|v| v.as_bool()) {
        return json!(v);
    }
    if let Some(v) = m.get("stringValue").and_then(|v| v.as_str()) {
        return json!(v);
    }
    // Generic fallback: look for "value" field
    if let Some(v) = m.get("value") {
        return v.clone();
    }
    JsonValue::Null
}

/// Parse a full SparkplugB message from topic + JSON payload.
pub fn parse_sparkplug_message(
    topic: &str,
    payload: &str,
) -> Result<SparkplugMessage, ConnectorError> {
    let topic_parsed = parse_sparkplug_topic(topic).ok_or_else(|| {
        ConnectorError::ParseError(format!("SparkplugB: invalid topic '{}'", topic))
    })?;
    let payload_parsed = parse_sparkplug_payload_json(payload)?;

    Ok(SparkplugMessage {
        topic: topic_parsed,
        payload: payload_parsed,
    })
}

// ---------------------------------------------------------------------------
// SparkplugMessage → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a SparkplugB message into SourceEvents (one per metric).
pub fn sparkplug_message_to_events(
    msg: &SparkplugMessage,
    connector_id: &str,
) -> Vec<SourceEvent> {
    let base_id = if let Some(ref did) = msg.topic.device_id {
        format!(
            "sparkplug:{}:{}:{}",
            msg.topic.group_id, msg.topic.edge_node_id, did
        )
    } else {
        format!(
            "sparkplug:{}:{}",
            msg.topic.group_id, msg.topic.edge_node_id
        )
    };

    let base_ts = msg
        .payload
        .timestamp
        .and_then(|t| DateTime::from_timestamp_millis(t as i64))
        .unwrap_or_else(Utc::now);

    msg.payload
        .metrics
        .iter()
        .map(|metric| {
            let entity_id = format!("{}:{}", base_id, metric.name);
            let ts = metric
                .timestamp
                .and_then(|t| DateTime::from_timestamp_millis(t as i64))
                .unwrap_or(base_ts);

            let mut properties = HashMap::new();
            properties.insert("metric_name".into(), json!(metric.name));
            properties.insert("metric_value".into(), metric.value.clone());
            properties.insert("datatype".into(), json!(metric.datatype.as_str()));
            properties.insert("group_id".into(), json!(msg.topic.group_id));
            properties.insert("edge_node_id".into(), json!(msg.topic.edge_node_id));
            properties.insert(
                "message_type".into(),
                json!(msg.topic.message_type.as_str()),
            );

            if let Some(ref did) = msg.topic.device_id {
                properties.insert("device_id".into(), json!(did));
            }
            if let Some(alias) = metric.alias {
                properties.insert("alias".into(), json!(alias));
            }
            if let Some(seq) = msg.payload.seq {
                properties.insert("seq".into(), json!(seq));
            }

            SourceEvent {
                connector_id: connector_id.to_string(),
                entity_id,
                entity_type: "sensor".into(),
                properties,
                timestamp: ts,
                latitude: None,
                longitude: None,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct SparkplugBConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl SparkplugBConnector {
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
impl Connector for SparkplugBConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        // In production this would subscribe to MQTT topics matching spBv1.0/#
        // For now, read from a file of JSON messages (one per line: topic\tpayload)
        let path = self
            .config
            .url
            .as_deref()
            .ok_or_else(|| {
                ConnectorError::ConfigError(
                    "SparkplugB: url (file path or MQTT broker) required".into(),
                )
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

            // Expect "topic\tpayload_json"
            if let Some(tab_pos) = line.find('\t') {
                let topic = &line[..tab_pos];
                let payload = &line[tab_pos + 1..];

                match parse_sparkplug_message(topic, payload) {
                    Ok(msg) => {
                        let events = sparkplug_message_to_events(&msg, &connector_id);
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
                "SparkplugB connector is not running".into(),
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
    fn test_parse_topic_device() {
        let topic = parse_sparkplug_topic("spBv1.0/Plant1/DDATA/EdgeNode1/Device1").unwrap();
        assert_eq!(topic.group_id, "Plant1");
        assert_eq!(topic.message_type, SparkplugMessageType::DData);
        assert_eq!(topic.edge_node_id, "EdgeNode1");
        assert_eq!(topic.device_id, Some("Device1".into()));
    }

    #[test]
    fn test_parse_topic_node() {
        let topic = parse_sparkplug_topic("spBv1.0/Factory/NBIRTH/Node42").unwrap();
        assert_eq!(topic.group_id, "Factory");
        assert_eq!(topic.message_type, SparkplugMessageType::NBirth);
        assert_eq!(topic.edge_node_id, "Node42");
        assert!(topic.device_id.is_none());
    }

    #[test]
    fn test_parse_invalid_topic() {
        assert!(parse_sparkplug_topic("invalid/topic").is_none());
        assert!(parse_sparkplug_topic("spBv1.0").is_none());
    }

    #[test]
    fn test_parse_payload_json() {
        let payload = r#"{
            "timestamp": 1700000000000,
            "seq": 42,
            "metrics": [
                {"name": "Temperature", "datatype": 10, "doubleValue": 25.5, "timestamp": 1700000000001},
                {"name": "Pressure", "datatype": 9, "floatValue": 101.3},
                {"name": "Running", "datatype": 11, "booleanValue": true},
                {"name": "Status", "datatype": 12, "stringValue": "OK"}
            ]
        }"#;
        let pl = parse_sparkplug_payload_json(payload).unwrap();
        assert_eq!(pl.timestamp, Some(1700000000000));
        assert_eq!(pl.seq, Some(42));
        assert_eq!(pl.metrics.len(), 4);
        assert_eq!(pl.metrics[0].name, "Temperature");
        assert_eq!(pl.metrics[0].datatype, SparkplugDataType::Double);
        assert_eq!(pl.metrics[0].value, json!(25.5));
        assert_eq!(pl.metrics[2].value, json!(true));
        assert_eq!(pl.metrics[3].value, json!("OK"));
    }

    #[test]
    fn test_parse_full_message() {
        let msg = parse_sparkplug_message(
            "spBv1.0/Plant1/DDATA/Node1/Sensor1",
            r#"{"timestamp": 1700000000000, "metrics": [{"name": "Temp", "datatype": 10, "doubleValue": 42.0}]}"#,
        )
        .unwrap();
        assert_eq!(msg.topic.group_id, "Plant1");
        assert_eq!(msg.topic.device_id, Some("Sensor1".into()));
        assert_eq!(msg.payload.metrics.len(), 1);
    }

    #[test]
    fn test_sparkplug_to_events() {
        let msg = parse_sparkplug_message(
            "spBv1.0/Factory/DDATA/Node1/PLC1",
            r#"{
                "timestamp": 1700000000000,
                "seq": 10,
                "metrics": [
                    {"name": "Temperature", "datatype": 10, "doubleValue": 85.5},
                    {"name": "RPM", "datatype": 7, "intValue": 3600}
                ]
            }"#,
        )
        .unwrap();
        let events = sparkplug_message_to_events(&msg, "spb-test");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_type, "sensor");
        assert_eq!(
            events[0].entity_id,
            "sparkplug:Factory:Node1:PLC1:Temperature"
        );
        assert_eq!(events[0].properties["metric_value"], json!(85.5));
        assert_eq!(events[0].properties["group_id"], json!("Factory"));
        assert_eq!(events[1].properties["metric_value"], json!(3600));
    }

    #[test]
    fn test_message_type_checks() {
        assert!(SparkplugMessageType::NBirth.is_birth());
        assert!(SparkplugMessageType::DBirth.is_birth());
        assert!(!SparkplugMessageType::NData.is_birth());
        assert!(SparkplugMessageType::NDeath.is_death());
        assert!(SparkplugMessageType::NData.is_data());
        assert!(SparkplugMessageType::DData.is_data());
    }

    #[test]
    fn test_datatype_codes() {
        assert_eq!(SparkplugDataType::from_code(1), SparkplugDataType::Int8);
        assert_eq!(SparkplugDataType::from_code(10), SparkplugDataType::Double);
        assert_eq!(SparkplugDataType::from_code(11), SparkplugDataType::Boolean);
        assert_eq!(SparkplugDataType::from_code(12), SparkplugDataType::String);
        assert_eq!(SparkplugDataType::from_code(99), SparkplugDataType::Unknown(99));
    }

    #[test]
    fn test_empty_metrics() {
        let payload = r#"{"timestamp": 1700000000000, "metrics": []}"#;
        let pl = parse_sparkplug_payload_json(payload).unwrap();
        assert_eq!(pl.metrics.len(), 0);
    }

    #[test]
    fn test_sparkplug_connector_id() {
        let config = ConnectorConfig {
            connector_id: "spb-1".to_string(),
            connector_type: "sparkplugb".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = SparkplugBConnector::new(config);
        assert_eq!(connector.connector_id(), "spb-1");
    }

    #[tokio::test]
    async fn test_sparkplug_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "spb-h".to_string(),
            connector_type: "sparkplugb".to_string(),
            url: None,
            entity_type: "sensor".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = SparkplugBConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_invalid_payload_json() {
        assert!(parse_sparkplug_payload_json("{invalid}").is_err());
    }
}
