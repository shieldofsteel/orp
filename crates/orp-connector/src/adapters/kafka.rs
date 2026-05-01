//! Kafka adapter — JSON envelopes pulled from Apache Kafka topics.
//!
//! Each consumed record's payload is a JSON object containing one ORP entity:
//!
//! ```json
//! { "entity_id": "vessel-7", "entity_type": "vessel",
//!   "lat": 54.123, "lon": 12.345, "speed_knots": 11.2 }
//! ```
//!
//! Reserved keys (`entity_id`, `entity_type`, `lat`, `lon`) map onto the
//! [`SourceEvent`] fields directly; everything else is forwarded into
//! `properties`. `entity_type` defaults to `"kafka_message"` when absent.
//!
//! URL: `kafka://broker1:9092,broker2:9092/TOPIC_NAME` — comma-separated
//! brokers, single topic. Connector properties:
//!
//! | Key                 | Default                  |
//! |---------------------|--------------------------|
//! | `group_id`          | `orp-{connector_id}`     |
//! | `security_protocol` | `PLAINTEXT`              |
//! | `sasl_mechanism`    | _(none)_                 |
//! | `sasl_username`     | _(none)_                 |
//! | `sasl_password`     | _(none)_                 |
//!
//! Malformed JSON bumps `errors_count` and logs at WARN with topic + offset;
//! the consumer keeps running. Records arrive from untrusted producers, so
//! there are **no** `.unwrap()` / `.expect()` calls outside `#[cfg(test)]`.

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// librdkafka session timeout. Pinned so tests can assert deterministic config.
const DEFAULT_SESSION_TIMEOUT_MS: &str = "10000";
/// `auto.offset.reset` — `latest` so a new connector doesn't replay history.
const DEFAULT_AUTO_OFFSET_RESET: &str = "latest";

/// Parsed broker list + topic from `kafka://...` URL. `brokers` is comma-joined
/// for direct use as `bootstrap.servers`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct KafkaTarget { brokers: String, topic: String }

/// Auth + transport security knobs pulled out of `ConnectorConfig.properties`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SecurityConfig {
    security_protocol: Option<String>,
    sasl_mechanism: Option<String>,
    sasl_username: Option<String>,
    sasl_password: Option<String>,
}

/// Connector that consumes JSON envelopes from a single Kafka topic and emits
/// [`SourceEvent`]s.
pub struct KafkaConnector {
    config: ConnectorConfig,
    target: KafkaTarget,
    group_id: String,
    security: SecurityConfig,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    last_event_ts: Arc<AtomicI64>,
    /// Current consumer task. `Some` between `start()` and `stop()`.
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl KafkaConnector {
    /// Build a connector by parsing the `kafka://` URL and reading auth /
    /// group properties. Returns [`ConnectorError::ConfigError`] on malformed
    /// input.
    pub fn from_connector_config(config: ConnectorConfig) -> Result<Self, ConnectorError> {
        let target = parse_kafka_url(config.url.as_deref())?;
        let group_id = group_id_from(&config);
        let security = SecurityConfig::from_properties(&config.properties);
        Ok(Self {
            config, target, group_id, security,
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
            last_event_ts: Arc::new(AtomicI64::new(0)),
            handle: Mutex::new(None),
        })
    }

    /// Infallible constructor kept for parity with other adapters: a missing
    /// or malformed URL produces a stub target, and `start()` will surface
    /// the real [`ConnectorError::ConfigError`].
    pub fn new(config: ConnectorConfig) -> Self {
        let target = config
            .url
            .as_deref()
            .and_then(|u| parse_kafka_url(Some(u)).ok())
            .unwrap_or_else(|| KafkaTarget { brokers: String::new(), topic: String::new() });
        let group_id = group_id_from(&config);
        let security = SecurityConfig::from_properties(&config.properties);
        Self {
            config, target, group_id, security,
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
            last_event_ts: Arc::new(AtomicI64::new(0)),
            handle: Mutex::new(None),
        }
    }

    /// Decode a single Kafka record payload into a [`SourceEvent`]. Returns
    /// `Err(String)` describing the failure on malformed JSON or a missing
    /// `entity_id`.
    pub fn decode_message(
        connector_id: &str,
        payload: &[u8],
        timestamp: DateTime<Utc>,
    ) -> Result<SourceEvent, String> {
        let value: JsonValue = serde_json::from_slice(payload)
            .map_err(|e| format!("invalid JSON envelope: {e}"))?;
        let obj = value
            .as_object()
            .ok_or_else(|| "JSON envelope must be a top-level object".to_string())?;

        let entity_id = obj
            .get("entity_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "envelope missing required string field 'entity_id'".to_string())?
            .to_string();

        let entity_type = obj
            .get("entity_type")
            .and_then(|v| v.as_str())
            .unwrap_or("kafka_message")
            .to_string();

        let latitude = obj.get("lat").and_then(|v| v.as_f64());
        let longitude = obj.get("lon").and_then(|v| v.as_f64());

        // Reserved keys are mapped onto SourceEvent fields directly. Everything
        // else is forwarded into `properties` so downstream queries can see
        // arbitrary producer-defined fields.
        let mut properties: HashMap<String, JsonValue> = HashMap::new();
        for (k, v) in obj {
            match k.as_str() {
                "entity_id" | "entity_type" | "lat" | "lon" => {}
                _ => {
                    properties.insert(k.clone(), v.clone());
                }
            }
        }

        Ok(SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id,
            entity_type,
            properties,
            timestamp,
            latitude,
            longitude,
        })
    }

    /// Build the librdkafka [`ClientConfig`] for this connector. Exposed so
    /// tests can assert SASL knobs reach the consumer config without booting
    /// a real broker.
    pub fn build_client_config(&self) -> ClientConfig {
        build_client_config(&self.target, &self.group_id, &self.security)
    }

    #[cfg(test)]
    fn target(&self) -> &KafkaTarget { &self.target }
    #[cfg(test)]
    fn group_id_str(&self) -> &str { &self.group_id }
}

/// Consumer-group fallback: `orp-{connector_id}` whenever the operator hasn't
/// pinned `group_id`. Per-connector groups keep offsets isolated so two
/// distinct ORP connectors don't share commits.
fn group_id_from(config: &ConnectorConfig) -> String {
    if let Some(JsonValue::String(s)) = config.properties.get("group_id") {
        if !s.trim().is_empty() {
            return s.clone();
        }
    }
    format!("orp-{}", config.connector_id)
}

impl SecurityConfig {
    fn from_properties(props: &HashMap<String, JsonValue>) -> Self {
        let pull = |key: &str| -> Option<String> {
            props
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        };
        Self {
            security_protocol: pull("security_protocol"),
            sasl_mechanism: pull("sasl_mechanism"),
            sasl_username: pull("sasl_username"),
            sasl_password: pull("sasl_password"),
        }
    }

    fn apply(&self, cfg: &mut ClientConfig) {
        let proto = self
            .security_protocol
            .clone()
            .unwrap_or_else(|| "PLAINTEXT".to_string());
        cfg.set("security.protocol", &proto);
        if let Some(mech) = &self.sasl_mechanism {
            cfg.set("sasl.mechanism", mech);
        }
        if let Some(u) = &self.sasl_username {
            cfg.set("sasl.username", u);
        }
        if let Some(p) = &self.sasl_password {
            cfg.set("sasl.password", p);
        }
    }
}

/// Parse `kafka://broker1:9092,broker2:9092/TOPIC_NAME` into bootstrap +
/// topic. Produces a [`ConnectorError::ConfigError`] for any structural
/// problem (bad scheme, empty broker list, missing topic).
fn parse_kafka_url(url: Option<&str>) -> Result<KafkaTarget, ConnectorError> {
    let url = url
        .ok_or_else(|| ConnectorError::ConfigError("kafka connector requires a URL".into()))?
        .trim();
    let rest = url.strip_prefix("kafka://").ok_or_else(|| {
        ConnectorError::ConfigError(format!(
            "kafka connector URL must use kafka:// scheme, got: {url}"
        ))
    })?;
    if rest.is_empty() {
        return Err(ConnectorError::ConfigError(
            "kafka connector URL is empty after scheme".into(),
        ));
    }

    // Split on the first '/' — everything before is the broker list, after is
    // the topic. We deliberately reject empty segments either side.
    let (brokers_part, topic_part) = rest
        .split_once('/')
        .ok_or_else(|| ConnectorError::ConfigError(format!(
            "kafka connector URL is missing /TOPIC: {url}"
        )))?;

    if brokers_part.is_empty() {
        return Err(ConnectorError::ConfigError(
            "kafka connector URL has empty broker list".into(),
        ));
    }
    let topic = topic_part.trim_end_matches('/').to_string();
    if topic.is_empty() {
        return Err(ConnectorError::ConfigError(format!(
            "kafka connector URL has empty topic: {url}"
        )));
    }
    if topic.contains('/') {
        return Err(ConnectorError::ConfigError(format!(
            "kafka connector URL must contain a single topic, got: {topic}"
        )));
    }

    // Reject empty broker entries (e.g. "kafka://b1:9092,/topic").
    let mut brokers: Vec<String> = Vec::new();
    for b in brokers_part.split(',') {
        let b = b.trim();
        if b.is_empty() {
            return Err(ConnectorError::ConfigError(
                "kafka connector URL has empty broker entry".into(),
            ));
        }
        brokers.push(b.to_string());
    }

    Ok(KafkaTarget {
        brokers: brokers.join(","),
        topic,
    })
}

/// Build the librdkafka [`ClientConfig`] for a target. Pulled out so tests
/// can verify config without instantiating a connector.
fn build_client_config(
    target: &KafkaTarget,
    group_id: &str,
    security: &SecurityConfig,
) -> ClientConfig {
    let mut cfg = ClientConfig::new();
    cfg.set("bootstrap.servers", &target.brokers)
        .set("group.id", group_id)
        .set("session.timeout.ms", DEFAULT_SESSION_TIMEOUT_MS)
        .set("auto.offset.reset", DEFAULT_AUTO_OFFSET_RESET)
        .set("enable.auto.commit", "true");
    security.apply(&mut cfg);
    cfg
}

#[async_trait]
impl Connector for KafkaConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        if self.target.brokers.is_empty() || self.target.topic.is_empty() {
            return Err(ConnectorError::ConfigError(
                "kafka connector started without a valid kafka:// URL".into(),
            ));
        }

        let cfg = build_client_config(&self.target, &self.group_id, &self.security);
        let consumer: StreamConsumer = cfg.create().map_err(|e| {
            ConnectorError::ConnectionError(format!("kafka consumer create failed: {e}"))
        })?;
        consumer.subscribe(&[&self.target.topic]).map_err(|e| {
            ConnectorError::ConnectionError(format!(
                "kafka subscribe to '{}' failed: {e}",
                self.target.topic
            ))
        })?;

        tracing::info!(
            connector_id = %self.config.connector_id,
            brokers = %self.target.brokers,
            topic = %self.target.topic,
            group_id = %self.group_id,
            "Kafka connector starting"
        );

        let connector_id = self.config.connector_id.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let last_event_ts = self.last_event_ts.clone();

        let handle = tokio::spawn(async move {
            drive_consumer(
                consumer,
                connector_id,
                events_count,
                errors_count,
                last_event_ts,
                tx,
            )
            .await;
        });
        let mut slot = self.handle.lock().await;
        // If start() is called twice without stop(), abort the previous task to
        // avoid leaking it. Test-harness scenarios occasionally do this.
        if let Some(prev) = slot.replace(handle) {
            prev.abort();
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        let mut slot = self.handle.lock().await;
        if let Some(h) = slot.take() {
            h.abort();
        }
        tracing::info!(
            connector_id = %self.config.connector_id,
            "Kafka connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        let slot = self.handle.lock().await;
        match slot.as_ref() {
            Some(h) if !h.is_finished() => Ok(()),
            Some(_) => Err(ConnectorError::ConnectionError(
                "kafka consumer task ended unexpectedly".into(),
            )),
            None => Err(ConnectorError::ConnectionError(
                "kafka connector not running".into(),
            )),
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        let last_secs = self.last_event_ts.load(Ordering::Relaxed);
        let last_event_timestamp = if last_secs == 0 {
            None
        } else {
            // `timestamp_opt(secs, 0)` only returns None for out-of-range
            // values; we wrote epoch seconds ourselves, so this branch is
            // pure defensive plumbing rather than a hot path.
            Utc.timestamp_opt(last_secs, 0).single()
        };
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
            last_event_timestamp,
            uptime_seconds: 0,
        }
    }
}

/// Long-running consumer task. Pulls one record at a time, decodes the JSON
/// envelope, and pushes a `SourceEvent` onto the channel. Malformed records
/// bump `errors_count` and are logged but never crash the loop.
async fn drive_consumer(
    consumer: StreamConsumer,
    connector_id: String,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    last_event_ts: Arc<AtomicI64>,
    tx: tokio::sync::mpsc::Sender<SourceEvent>,
) {
    loop {
        let msg = match consumer.recv().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("kafka recv error: {e}");
                errors_count.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let payload = match msg.payload() {
            Some(p) => p,
            None => continue, // tombstones / control messages
        };
        match KafkaConnector::decode_message(&connector_id, payload, Utc::now()) {
            Ok(event) => {
                if tx.send(event).await.is_err() {
                    tracing::warn!("kafka downstream channel closed; ending consumer");
                    return;
                }
                events_count.fetch_add(1, Ordering::Relaxed);
                last_event_ts.store(Utc::now().timestamp(), Ordering::Relaxed);
            }
            Err(e) => {
                errors_count.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    topic = msg.topic(),
                    offset = msg.offset(),
                    "kafka envelope decode failed: {e}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdkafka::consumer::BaseConsumer;
    use serde_json::json;

    fn build_config(url: Option<&str>, props: HashMap<String, JsonValue>) -> ConnectorConfig {
        ConnectorConfig {
            connector_id: "kafka-test".into(),
            connector_type: "kafka".into(),
            url: url.map(str::to_string),
            entity_type: "kafka_message".into(),
            enabled: true,
            trust_score: 0.7,
            properties: props,
        }
    }

    /// Helper: build a connector with the canonical test URL and given props.
    fn make_conn(props: HashMap<String, JsonValue>) -> KafkaConnector {
        KafkaConnector::from_connector_config(build_config(
            Some("kafka://b1:9092/topic"),
            props,
        ))
        .expect("test config must parse")
    }

    fn assert_config_err<T: std::fmt::Debug>(r: Result<T, ConnectorError>, ctx: &str) {
        match r {
            Err(ConnectorError::ConfigError(_)) => {}
            other => panic!("expected ConfigError ({ctx}), got {other:?}"),
        }
    }

    #[test]
    fn url_parses_single_broker_and_topic() {
        let target = parse_kafka_url(Some("kafka://localhost:9092/orp-events")).unwrap();
        assert_eq!(target.brokers, "localhost:9092");
        assert_eq!(target.topic, "orp-events");
    }

    #[test]
    fn url_parses_multi_broker() {
        let target = parse_kafka_url(Some("kafka://b1:9092,b2:9092/orp")).unwrap();
        assert_eq!(target.brokers, "b1:9092,b2:9092");
        assert_eq!(target.topic, "orp");
        assert_eq!(target.brokers.split(',').count(), 2);
    }

    #[test]
    fn url_rejects_bad_scheme() {
        assert_config_err(parse_kafka_url(Some("http://localhost:9092/orp-events")), "bad scheme");
    }

    #[test]
    fn url_rejects_missing_topic() {
        assert_config_err(parse_kafka_url(Some("kafka://b1:9092")), "no slash");
        assert_config_err(parse_kafka_url(Some("kafka://b1:9092/")), "trailing slash");
    }

    #[test]
    fn url_rejects_empty_broker_entry() {
        assert_config_err(parse_kafka_url(Some("kafka://b1:9092,/orp")), "empty broker");
    }

    #[test]
    fn group_id_defaults_when_missing() {
        let conn = make_conn(HashMap::new());
        assert_eq!(conn.group_id_str(), "orp-kafka-test");
        assert_eq!(conn.connector_id(), "kafka-test");
    }

    #[test]
    fn group_id_honors_property_override() {
        let mut props = HashMap::new();
        props.insert("group_id".into(), json!("ops-team"));
        assert_eq!(make_conn(props).group_id_str(), "ops-team");
    }

    #[test]
    fn decode_envelope_happy_path() {
        let payload = br#"{
            "entity_id": "vessel-7",
            "entity_type": "vessel",
            "lat": 54.123,
            "lon": 12.345,
            "speed_knots": 11.2,
            "name": "MV Example"
        }"#;
        let now = Utc::now();
        let event = KafkaConnector::decode_message("kafka-test", payload, now).unwrap();
        assert_eq!(event.connector_id, "kafka-test");
        assert_eq!(event.entity_id, "vessel-7");
        assert_eq!(event.entity_type, "vessel");
        assert_eq!(event.latitude, Some(54.123));
        assert_eq!(event.longitude, Some(12.345));
        assert_eq!(event.properties["speed_knots"], json!(11.2));
        assert_eq!(event.properties["name"], json!("MV Example"));
        // Reserved keys must NOT leak into properties.
        assert!(!event.properties.contains_key("entity_id"));
        assert!(!event.properties.contains_key("entity_type"));
        assert!(!event.properties.contains_key("lat"));
        assert!(!event.properties.contains_key("lon"));
        assert_eq!(event.timestamp, now);
    }

    #[test]
    fn decode_default_entity_type_when_missing() {
        let payload = br#"{"entity_id":"x"}"#;
        let event = KafkaConnector::decode_message("k", payload, Utc::now()).unwrap();
        assert_eq!(event.entity_type, "kafka_message");
    }

    #[test]
    fn malformed_json_returns_err_no_panic() {
        let bad = b"this is { not :: json";
        let res = KafkaConnector::decode_message("k", bad, Utc::now());
        assert!(res.is_err());

        // Also exercise the in-task path so errors_count is bumped without panicking.
        let errors = Arc::new(AtomicU64::new(0));
        // Simulate the same logic the task uses.
        if KafkaConnector::decode_message("k", bad, Utc::now()).is_err() {
            errors.fetch_add(1, Ordering::Relaxed);
        }
        assert_eq!(errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn missing_entity_id_returns_err() {
        let payload = br#"{"entity_type":"vessel","lat":1.0}"#;
        let res = KafkaConnector::decode_message("k", payload, Utc::now());
        assert!(res.is_err());
    }

    #[test]
    fn lat_lon_absent_yields_none() {
        let payload = br#"{"entity_id":"x","entity_type":"sensor","temperature":21.5}"#;
        let event = KafkaConnector::decode_message("k", payload, Utc::now()).unwrap();
        assert_eq!(event.latitude, None);
        assert_eq!(event.longitude, None);
        assert_eq!(event.properties["temperature"], json!(21.5));
    }

    #[test]
    fn sasl_config_is_wired_into_client_config() {
        // SASL_SSL exercises the full property surface. librdkafka may not
        // have been compiled with OpenSSL on the test machine, so we only
        // assert the *config map* contains every key — not that
        // `create::<BaseConsumer>()` succeeds for SSL.
        let mut props = HashMap::new();
        props.insert("security_protocol".into(), json!("SASL_SSL"));
        props.insert("sasl_mechanism".into(), json!("SCRAM-SHA-256"));
        props.insert("sasl_username".into(), json!("alice"));
        props.insert("sasl_password".into(), json!("hunter2"));
        let cfg = make_conn(props).build_client_config();
        assert_eq!(cfg.get("bootstrap.servers"), Some("b1:9092"));
        assert_eq!(cfg.get("security.protocol"), Some("SASL_SSL"));
        assert_eq!(cfg.get("sasl.mechanism"), Some("SCRAM-SHA-256"));
        assert_eq!(cfg.get("sasl.username"), Some("alice"));
        assert_eq!(cfg.get("sasl.password"), Some("hunter2"));
        assert_eq!(cfg.get("group.id"), Some("orp-kafka-test"));
        assert_eq!(cfg.get("auto.offset.reset"), Some("latest"));
    }

    #[test]
    fn sasl_plaintext_config_round_trips_through_rdkafka() {
        // Smoke-test that librdkafka accepts our config map with a SASL
        // mechanism that doesn't require OpenSSL at build time.
        let mut props = HashMap::new();
        props.insert("security_protocol".into(), json!("SASL_PLAINTEXT"));
        props.insert("sasl_mechanism".into(), json!("PLAIN"));
        props.insert("sasl_username".into(), json!("svc"));
        props.insert("sasl_password".into(), json!("pw"));
        let cfg = make_conn(props).build_client_config();
        assert_eq!(cfg.get("security.protocol"), Some("SASL_PLAINTEXT"));
        assert_eq!(cfg.get("sasl.mechanism"), Some("PLAIN"));
        let _consumer: BaseConsumer = cfg.create().expect("rdkafka accepts config");
    }

    #[test]
    fn no_auth_defaults_to_plaintext() {
        let cfg = make_conn(HashMap::new()).build_client_config();
        assert_eq!(cfg.get("security.protocol"), Some("PLAINTEXT"));
        assert!(cfg.get("sasl.mechanism").is_none());
        assert!(cfg.get("sasl.username").is_none());
        assert!(cfg.get("sasl.password").is_none());
    }

    #[test]
    fn from_config_target_round_trip() {
        let conn = KafkaConnector::from_connector_config(build_config(
            Some("kafka://b1:9092,b2:9093/orp-events"),
            HashMap::new(),
        ))
        .unwrap();
        assert_eq!(conn.target().brokers, "b1:9092,b2:9093");
        assert_eq!(conn.target().topic, "orp-events");
    }

    #[test]
    fn stats_initial_state_has_no_last_timestamp() {
        let stats = make_conn(HashMap::new()).stats();
        assert_eq!(stats.events_processed, 0);
        assert_eq!(stats.errors, 0);
        assert!(stats.last_event_timestamp.is_none());
    }

    #[tokio::test]
    async fn health_check_reports_not_running_before_start() {
        let conn = make_conn(HashMap::new());
        match conn.health_check().await {
            Err(ConnectorError::ConnectionError(_)) => {}
            other => panic!("expected ConnectionError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stop_is_idempotent_when_never_started() {
        let conn = make_conn(HashMap::new());
        conn.stop().await.unwrap();
        conn.stop().await.unwrap(); // calling twice must not panic
    }
}
