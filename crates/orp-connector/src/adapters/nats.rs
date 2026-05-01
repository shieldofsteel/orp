//! NATS / JetStream adapter — durable subject-based pub/sub.
//!
//! [NATS](https://nats.io) plus JetStream provides at-least-once delivery,
//! persistence, and durable consumers — the same role Kafka plays for many
//! ORP deployments but with a smaller operational footprint. This adapter
//! consumes events from either:
//!
//! * a **JetStream** stream via a durable pull consumer (resumable across
//!   reconnects), or
//! * a **plain core NATS** subject via best-effort fan-out (no ack, no replay).
//!
//! # URL scheme
//!
//! * `nats://host:4222/subject/<SUBJECT_PATTERN>` — core NATS subscribe.
//!   Wildcards (`foo.*`, `foo.>`) flow through unchanged.
//! * `nats://host:4222/stream/<STREAM_NAME>/subject/<SUBJECT_PATTERN>` —
//!   JetStream pull consumer. The consumer is named `orp-{connector_id}` so
//!   redeliveries resume from the last ack on reconnect.
//!
//! # Authentication (`properties`, first match wins)
//!
//! | Property                       | Auth method                |
//! |--------------------------------|----------------------------|
//! | `nats_token`                   | Static auth token          |
//! | `nats_user` + `nats_password`  | User / password            |
//! | `nats_creds_path`              | NSC `.creds` (JWT + nkey)  |
//!
//! No properties → anonymous connect.
//!
//! # Wire format
//!
//! Each payload is a JSON envelope of the same shape used by the Kafka
//! adapter: `{entity_id, entity_type, lat, lon, ...rest}`. Malformed JSON
//! increments `errors_count` and never panics.

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

const DEFAULT_NATS_PORT: u16 = 4222;

/// Decoded shape of a configured `nats://` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NatsTarget {
    pub(crate) server: String,
    pub(crate) stream: Option<String>,
    pub(crate) subject: String,
}

impl NatsTarget {
    pub(crate) fn is_jetstream(&self) -> bool {
        self.stream.is_some()
    }
}

/// Auth selection — token > user/pass > creds-file > anonymous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NatsAuth {
    Anonymous,
    Token(String),
    UserPass { user: String, pass: String },
    CredsFile(PathBuf),
}

impl NatsAuth {
    /// Read auth properties with strict token > user/pass > creds-path > anon
    /// precedence. Empty strings are treated as unset to keep config lenient.
    pub(crate) fn from_properties(props: &HashMap<String, JsonValue>) -> Self {
        let s = |k: &str| {
            props
                .get(k)
                .and_then(JsonValue::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        if let Some(token) = s("nats_token") {
            return NatsAuth::Token(token);
        }
        if let (Some(user), Some(pass)) = (s("nats_user"), s("nats_password")) {
            return NatsAuth::UserPass { user, pass };
        }
        if let Some(path) = s("nats_creds_path") {
            return NatsAuth::CredsFile(PathBuf::from(path));
        }
        NatsAuth::Anonymous
    }
}

/// JetStream-or-core NATS source connector. Spawns a single driver task on
/// `start()` and tears it down on `stop()`. Reconnects are handled inside the
/// `async_nats::Client`.
pub struct NatsConnector {
    config: ConnectorConfig,
    target: NatsTarget,
    auth: NatsAuth,
    running: Arc<AtomicBool>,
    /// Whether the driver believes its `Client` is connected. Read by `health_check`.
    connected: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    /// Epoch seconds of the most recent successful event. `0` when unset.
    last_event_epoch: Arc<AtomicI64>,
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for NatsConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NatsConnector")
            .field("connector_id", &self.config.connector_id)
            .field("target", &self.target)
            .field("running", &self.running.load(Ordering::Relaxed))
            .finish()
    }
}

impl NatsConnector {
    /// Build a connector by parsing `config.url` and any auth properties.
    pub fn from_connector_config(config: ConnectorConfig) -> Result<Self, ConnectorError> {
        let target = parse_url(config.url.as_deref())?;
        let auth = NatsAuth::from_properties(&config.properties);
        Ok(Self::with_target(config, target, auth))
    }

    /// Best-effort constructor that swallows URL errors so legacy callers that
    /// build connectors before validating URL aren't broken at construction
    /// time. `start()` will still return `ConfigError`.
    pub fn new(config: ConnectorConfig) -> Self {
        match parse_url(config.url.as_deref()) {
            Ok(target) => {
                let auth = NatsAuth::from_properties(&config.properties);
                Self::with_target(config, target, auth)
            }
            Err(_) => Self::with_target(
                config,
                NatsTarget {
                    server: String::new(),
                    stream: None,
                    subject: String::new(),
                },
                NatsAuth::Anonymous,
            ),
        }
    }

    fn with_target(config: ConnectorConfig, target: NatsTarget, auth: NatsAuth) -> Self {
        Self {
            config,
            target,
            auth,
            running: Arc::new(AtomicBool::new(false)),
            connected: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
            last_event_epoch: Arc::new(AtomicI64::new(0)),
            task: tokio::sync::Mutex::new(None),
        }
    }

    /// Decode a JSON-envelope payload into a [`SourceEvent`]. Returns an error
    /// string for malformed JSON / missing required fields so the caller can
    /// bump the error counter without aborting the loop.
    pub fn decode_envelope(
        payload: &[u8],
        connector_id: &str,
        default_entity_type: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<SourceEvent, String> {
        let value: JsonValue =
            serde_json::from_slice(payload).map_err(|e| format!("invalid JSON: {e}"))?;
        let obj = value
            .as_object()
            .ok_or_else(|| "envelope must be a JSON object".to_string())?;

        let entity_id = obj
            .get("entity_id")
            .or_else(|| obj.get("id"))
            .and_then(json_value_as_string)
            .ok_or_else(|| "envelope missing entity_id".to_string())?;
        let entity_type = obj
            .get("entity_type")
            .and_then(JsonValue::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| default_entity_type.to_string());

        let latitude = obj
            .get("lat")
            .or_else(|| obj.get("latitude"))
            .and_then(JsonValue::as_f64);
        let longitude = obj
            .get("lon")
            .or_else(|| obj.get("longitude"))
            .or_else(|| obj.get("lng"))
            .and_then(JsonValue::as_f64);

        let mut properties: HashMap<String, JsonValue> =
            HashMap::with_capacity(obj.len().saturating_sub(4));
        for (k, v) in obj {
            match k.as_str() {
                "entity_id" | "entity_type" | "id" | "lat" | "lon" | "lng" | "latitude"
                | "longitude" => {}
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

    #[cfg(test)]
    pub(crate) fn target(&self) -> &NatsTarget {
        &self.target
    }
    #[cfg(test)]
    pub(crate) fn auth(&self) -> &NatsAuth {
        &self.auth
    }
}

/// Convert a JSON value that might be a string OR a number into a string for
/// `entity_id`. Real-world producers send IDs both ways.
fn json_value_as_string(v: &JsonValue) -> Option<String> {
    match v {
        JsonValue::String(s) => Some(s.clone()),
        JsonValue::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Parse `nats://host:port[/subject/<subj>|/stream/<stream>/subject/<subj>]`.
fn parse_url(url: Option<&str>) -> Result<NatsTarget, ConnectorError> {
    let url = url
        .ok_or_else(|| ConnectorError::ConfigError("nats connector requires a URL".into()))?
        .trim();
    let rest = url.strip_prefix("nats://").ok_or_else(|| {
        ConnectorError::ConfigError(format!(
            "nats connector URL must use nats:// scheme, got: {url}"
        ))
    })?;

    let (host_port, path) = rest.split_once('/').unwrap_or((rest, ""));
    if host_port.is_empty() {
        return Err(ConnectorError::ConfigError(
            "nats connector URL is missing host:port".into(),
        ));
    }
    let server = if host_port.contains(':') {
        host_port.to_string()
    } else {
        format!("{host_port}:{DEFAULT_NATS_PORT}")
    };

    // Path is one of "subject/<subj>" or "stream/<stream>/subject/<subj>".
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (stream, subject) = match segments.as_slice() {
        ["subject", rest @ ..] if !rest.is_empty() => (None, rest.join("/")),
        ["stream", stream_name, "subject", rest @ ..]
            if !stream_name.is_empty() && !rest.is_empty() =>
        {
            (Some((*stream_name).to_string()), rest.join("/"))
        }
        _ => {
            return Err(ConnectorError::ConfigError(format!(
                "nats URL path must be `/subject/<subj>` or `/stream/<stream>/subject/<subj>`, got: /{path}"
            )));
        }
    };
    if subject.is_empty() {
        return Err(ConnectorError::ConfigError(
            "nats subject must not be empty".into(),
        ));
    }
    Ok(NatsTarget {
        server,
        stream,
        subject,
    })
}

/// Build `ConnectOptions` from the auth selection.
async fn build_options(auth: &NatsAuth) -> Result<async_nats::ConnectOptions, ConnectorError> {
    let opts = async_nats::ConnectOptions::new().retry_on_initial_connect();
    Ok(match auth {
        NatsAuth::Anonymous => opts,
        NatsAuth::Token(t) => opts.token(t.clone()),
        NatsAuth::UserPass { user, pass } => opts.user_and_password(user.clone(), pass.clone()),
        NatsAuth::CredsFile(path) => opts.credentials_file(path).await.map_err(|e| {
            ConnectorError::ConfigError(format!(
                "failed to read NATS creds file {}: {e}",
                path.display()
            ))
        })?,
    })
}

/// Stable durable consumer name. JetStream rejects `.`, `*`, `>`, ` ` etc.,
/// so anything dodgy is replaced with `_`.
fn durable_consumer_name(connector_id: &str) -> String {
    let mut s = String::with_capacity(connector_id.len() + 4);
    s.push_str("orp-");
    for ch in connector_id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            s.push(ch);
        } else {
            s.push('_');
        }
    }
    s
}

#[async_trait]
impl Connector for NatsConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        if self.target.server.is_empty() || self.target.subject.is_empty() {
            return Err(ConnectorError::ConfigError(
                "nats connector started without a valid URL".into(),
            ));
        }
        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let connected = self.connected.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let last_event_epoch = self.last_event_epoch.clone();
        let connector_id = self.config.connector_id.clone();
        let entity_type = self.config.entity_type.clone();
        let target = self.target.clone();
        let auth = self.auth.clone();

        tracing::info!(
            connector_id = %connector_id,
            server = %target.server,
            stream = ?target.stream,
            subject = %target.subject,
            "NATS connector starting"
        );

        let handle = tokio::spawn(async move {
            run_driver(
                target,
                auth,
                connector_id,
                entity_type,
                running,
                connected,
                events_count,
                errors_count,
                last_event_epoch,
                tx,
            )
            .await;
        });

        let mut slot = self.task.lock().await;
        if let Some(prev) = slot.take() {
            prev.abort();
        }
        *slot = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
        let mut slot = self.task.lock().await;
        if let Some(handle) = slot.take() {
            handle.abort();
        }
        tracing::info!(connector_id = %self.config.connector_id, "NATS connector stopped");
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ConnectorError::ConnectionError(
                "NATS connector not running".into(),
            ));
        }
        if !self.connected.load(Ordering::SeqCst) {
            return Err(ConnectorError::ConnectionError(
                "NATS client is not currently connected".into(),
            ));
        }
        Ok(())
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        let last_epoch = self.last_event_epoch.load(Ordering::Relaxed);
        let last_event_timestamp = if last_epoch == 0 {
            None
        } else {
            Utc.timestamp_opt(last_epoch, 0).single()
        };
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
            last_event_timestamp,
            uptime_seconds: 0,
        }
    }
}

/// Connect, then dispatch to the JetStream or core-NATS message loop.
#[allow(clippy::too_many_arguments)]
async fn run_driver(
    target: NatsTarget,
    auth: NatsAuth,
    connector_id: String,
    entity_type: String,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    last_event_epoch: Arc<AtomicI64>,
    tx: tokio::sync::mpsc::Sender<SourceEvent>,
) {
    let opts = match build_options(&auth).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("NATS auth options build failed: {e:?}");
            errors_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    let client = match opts.connect(&target.server).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(server = %target.server, "NATS connect failed: {e}");
            errors_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    connected.store(true, Ordering::SeqCst);

    let result = if target.is_jetstream() {
        drive_jetstream(
            client,
            &target,
            &connector_id,
            &entity_type,
            &running,
            &events_count,
            &errors_count,
            &last_event_epoch,
            &tx,
        )
        .await
    } else {
        drive_core(
            client,
            &target,
            &connector_id,
            &entity_type,
            &running,
            &events_count,
            &errors_count,
            &last_event_epoch,
            &tx,
        )
        .await
    };
    connected.store(false, Ordering::SeqCst);
    if let Err(e) = result {
        tracing::warn!("NATS driver exited: {e}");
        errors_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Plain core NATS subscribe loop — no acks, no replay.
#[allow(clippy::too_many_arguments)]
async fn drive_core(
    client: async_nats::Client,
    target: &NatsTarget,
    connector_id: &str,
    entity_type: &str,
    running: &Arc<AtomicBool>,
    events_count: &Arc<AtomicU64>,
    errors_count: &Arc<AtomicU64>,
    last_event_epoch: &Arc<AtomicI64>,
    tx: &tokio::sync::mpsc::Sender<SourceEvent>,
) -> Result<(), String> {
    use futures_util::StreamExt;
    let mut sub = client
        .subscribe(target.subject.clone())
        .await
        .map_err(|e| format!("subscribe to {} failed: {e}", target.subject))?;

    while running.load(Ordering::SeqCst) {
        let next = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        let msg = match next {
            Ok(Some(m)) => m,
            Ok(None) => return Err("core NATS subscription closed by server".into()),
            Err(_) => continue,
        };
        process_payload(
            &msg.payload,
            connector_id,
            entity_type,
            events_count,
            errors_count,
            last_event_epoch,
            tx,
        )
        .await;
    }
    Ok(())
}

/// JetStream pull-consumer loop. Each successful channel send is followed by
/// `message.ack()` so the durable consumer advances its checkpoint. On decode
/// failure we deliberately don't ack — JetStream will redeliver up to
/// `max_deliver`, so a transient decoder bug doesn't drop data.
#[allow(clippy::too_many_arguments)]
async fn drive_jetstream(
    client: async_nats::Client,
    target: &NatsTarget,
    connector_id: &str,
    entity_type: &str,
    running: &Arc<AtomicBool>,
    events_count: &Arc<AtomicU64>,
    errors_count: &Arc<AtomicU64>,
    last_event_epoch: &Arc<AtomicI64>,
    tx: &tokio::sync::mpsc::Sender<SourceEvent>,
) -> Result<(), String> {
    use futures_util::StreamExt;
    let stream_name = target
        .stream
        .as_deref()
        .ok_or_else(|| "jetstream driver invoked without a stream name".to_string())?;

    let js = async_nats::jetstream::new(client);
    let stream = js
        .get_stream(stream_name)
        .await
        .map_err(|e| format!("jetstream get_stream({stream_name}) failed: {e}"))?;

    let durable = durable_consumer_name(connector_id);
    let consumer_config = async_nats::jetstream::consumer::pull::Config {
        durable_name: Some(durable.clone()),
        filter_subject: target.subject.clone(),
        ..Default::default()
    };
    let consumer: async_nats::jetstream::consumer::PullConsumer = stream
        .get_or_create_consumer(&durable, consumer_config)
        .await
        .map_err(|e| format!("jetstream consumer ({durable}) provision failed: {e}"))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|e| format!("jetstream messages stream open failed: {e}"))?;

    while running.load(Ordering::SeqCst) {
        let next =
            tokio::time::timeout(std::time::Duration::from_millis(500), messages.next()).await;
        let msg = match next {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => {
                tracing::debug!("jetstream message error: {e}");
                errors_count.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            Ok(None) => return Err("jetstream message stream closed".into()),
            Err(_) => continue,
        };

        let sent = process_payload(
            &msg.payload,
            connector_id,
            entity_type,
            events_count,
            errors_count,
            last_event_epoch,
            tx,
        )
        .await;
        if sent {
            if let Err(e) = msg.ack().await {
                tracing::debug!("jetstream ack error: {e}");
                errors_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    Ok(())
}

/// Decode a payload, send the resulting event, update counters. Returns
/// `true` when the event was forwarded — JetStream uses this to ack.
async fn process_payload(
    payload: &[u8],
    connector_id: &str,
    entity_type: &str,
    events_count: &Arc<AtomicU64>,
    errors_count: &Arc<AtomicU64>,
    last_event_epoch: &Arc<AtomicI64>,
    tx: &tokio::sync::mpsc::Sender<SourceEvent>,
) -> bool {
    let now = Utc::now();
    match NatsConnector::decode_envelope(payload, connector_id, entity_type, now) {
        Ok(event) => {
            if tx.send(event).await.is_err() {
                return false;
            }
            events_count.fetch_add(1, Ordering::Relaxed);
            last_event_epoch.store(now.timestamp(), Ordering::Relaxed);
            true
        }
        Err(e) => {
            tracing::debug!("NATS envelope decode failed: {e}");
            errors_count.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn build_config(url: Option<&str>, props: HashMap<String, JsonValue>) -> ConnectorConfig {
        ConnectorConfig {
            connector_id: "nats-test".into(),
            connector_type: "nats".into(),
            url: url.map(str::to_string),
            entity_type: "vehicle".into(),
            enabled: true,
            trust_score: 0.9,
            properties: props,
        }
    }

    #[test]
    fn parse_core_nats_url() {
        let conn = NatsConnector::from_connector_config(build_config(
            Some("nats://localhost:4222/subject/orp.events"),
            HashMap::new(),
        ))
        .expect("valid url");
        let target = conn.target();
        assert_eq!(target.server, "localhost:4222");
        assert_eq!(target.stream, None);
        assert_eq!(target.subject, "orp.events");
        assert!(!target.is_jetstream());
    }

    #[test]
    fn parse_jetstream_url() {
        let target = parse_url(Some("nats://localhost:4222/stream/EVENTS/subject/orp.>"))
            .expect("valid jetstream url");
        assert_eq!(target.server, "localhost:4222");
        assert_eq!(target.stream.as_deref(), Some("EVENTS"));
        assert_eq!(target.subject, "orp.>");
        assert!(target.is_jetstream());
    }

    #[test]
    fn parse_rejects_bad_scheme() {
        match NatsConnector::from_connector_config(build_config(
            Some("amqp://localhost:5672/subject/orp"),
            HashMap::new(),
        )) {
            Err(ConnectorError::ConfigError(_)) => {}
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_missing_url() {
        match NatsConnector::from_connector_config(build_config(None, HashMap::new())) {
            Err(ConnectorError::ConfigError(_)) => {}
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_path_without_subject() {
        assert!(matches!(
            parse_url(Some("nats://localhost:4222/")),
            Err(ConnectorError::ConfigError(_))
        ));
        assert!(matches!(
            parse_url(Some("nats://localhost:4222/stream/EVENTS")),
            Err(ConnectorError::ConfigError(_))
        ));
    }

    #[test]
    fn parse_defaults_port_when_missing() {
        let target = parse_url(Some("nats://broker/subject/orp.x")).expect("valid");
        assert_eq!(target.server, "broker:4222");
    }

    #[test]
    fn detects_both_modes_correctly() {
        let core = parse_url(Some("nats://h:4222/subject/foo.bar")).unwrap();
        let js = parse_url(Some("nats://h:4222/stream/S/subject/foo.bar")).unwrap();
        assert!(!core.is_jetstream());
        assert!(js.is_jetstream());
        assert_eq!(core.subject, "foo.bar");
        assert_eq!(js.subject, "foo.bar");
        assert_eq!(js.stream.as_deref(), Some("S"));
    }

    #[test]
    fn auth_precedence_token_over_user_pass_over_creds() {
        // Token wins over everything.
        let mut p = HashMap::new();
        p.insert("nats_token".into(), json!("tok"));
        p.insert("nats_user".into(), json!("alice"));
        p.insert("nats_password".into(), json!("hunter2"));
        p.insert("nats_creds_path".into(), json!("/etc/nats.creds"));
        assert_eq!(NatsAuth::from_properties(&p), NatsAuth::Token("tok".into()));

        // user/pass wins over creds-path.
        let mut p = HashMap::new();
        p.insert("nats_user".into(), json!("alice"));
        p.insert("nats_password".into(), json!("hunter2"));
        p.insert("nats_creds_path".into(), json!("/etc/nats.creds"));
        assert_eq!(
            NatsAuth::from_properties(&p),
            NatsAuth::UserPass {
                user: "alice".into(),
                pass: "hunter2".into()
            }
        );

        // creds-path alone.
        let mut p = HashMap::new();
        p.insert("nats_creds_path".into(), json!("/etc/nats.creds"));
        assert_eq!(
            NatsAuth::from_properties(&p),
            NatsAuth::CredsFile(PathBuf::from("/etc/nats.creds"))
        );

        // No auth properties → anonymous.
        assert_eq!(
            NatsAuth::from_properties(&HashMap::new()),
            NatsAuth::Anonymous
        );

        // Empty values are not treated as set.
        let mut p = HashMap::new();
        p.insert("nats_token".into(), json!(""));
        p.insert("nats_user".into(), json!(""));
        p.insert("nats_password".into(), json!(""));
        p.insert("nats_creds_path".into(), json!(""));
        assert_eq!(NatsAuth::from_properties(&p), NatsAuth::Anonymous);
    }

    #[test]
    fn decode_envelope_happy_path() {
        let payload = br#"{
            "entity_id": "vehicle-7", "entity_type": "truck",
            "lat": 47.6062, "lon": -122.3321,
            "speed_kph": 88, "fleet": "alpha"
        }"#;
        let event = NatsConnector::decode_envelope(payload, "conn", "vehicle", Utc::now()).unwrap();
        assert_eq!(event.connector_id, "conn");
        assert_eq!(event.entity_id, "vehicle-7");
        assert_eq!(event.entity_type, "truck");
        assert_eq!(event.latitude, Some(47.6062));
        assert_eq!(event.longitude, Some(-122.3321));
        assert_eq!(event.properties["speed_kph"], json!(88));
        assert_eq!(event.properties["fleet"], json!("alpha"));
        assert!(!event.properties.contains_key("entity_id"));
        assert!(!event.properties.contains_key("lat"));
    }

    #[test]
    fn decode_envelope_uses_default_entity_type_when_missing() {
        let payload = br#"{"entity_id":"id-1","latitude":1.0,"longitude":2.0}"#;
        let event = NatsConnector::decode_envelope(payload, "c", "boat", Utc::now()).unwrap();
        assert_eq!(event.entity_type, "boat");
        assert_eq!(event.latitude, Some(1.0));
        assert_eq!(event.longitude, Some(2.0));
    }

    #[test]
    fn decode_envelope_accepts_numeric_id() {
        let payload = br#"{"id":42,"lat":0.0,"lon":0.0}"#;
        let event = NatsConnector::decode_envelope(payload, "c", "thing", Utc::now()).unwrap();
        assert_eq!(event.entity_id, "42");
    }

    #[test]
    fn decode_envelope_rejects_malformed_json() {
        for bad in [
            &b"{not even json"[..],
            &b"[1, 2, 3]"[..],
            &b""[..],
            &br#"{"lat":1.0}"#[..], // missing entity_id
        ] {
            assert!(NatsConnector::decode_envelope(bad, "c", "t", Utc::now()).is_err());
        }
    }

    #[tokio::test]
    async fn malformed_payload_increments_errors_without_panic() {
        let events_count = Arc::new(AtomicU64::new(0));
        let errors_count = Arc::new(AtomicU64::new(0));
        let last_event_epoch = Arc::new(AtomicI64::new(0));
        let (tx, _rx) = tokio::sync::mpsc::channel::<SourceEvent>(8);
        let sent = process_payload(
            b"\xff\xff not json",
            "c",
            "t",
            &events_count,
            &errors_count,
            &last_event_epoch,
            &tx,
        )
        .await;
        assert!(!sent);
        assert_eq!(events_count.load(Ordering::Relaxed), 0);
        assert_eq!(errors_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn good_payload_increments_events_and_last_timestamp() {
        let events_count = Arc::new(AtomicU64::new(0));
        let errors_count = Arc::new(AtomicU64::new(0));
        let last_event_epoch = Arc::new(AtomicI64::new(0));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<SourceEvent>(8);
        let sent = process_payload(
            br#"{"entity_id":"x","lat":10.0,"lon":20.0}"#,
            "c",
            "t",
            &events_count,
            &errors_count,
            &last_event_epoch,
            &tx,
        )
        .await;
        assert!(sent);
        assert_eq!(events_count.load(Ordering::Relaxed), 1);
        assert_eq!(errors_count.load(Ordering::Relaxed), 0);
        assert!(last_event_epoch.load(Ordering::Relaxed) > 0);
        assert_eq!(rx.recv().await.expect("event sent").entity_id, "x");
    }

    #[test]
    fn subject_wildcards_preserved_through_url_parse() {
        // `>` (rest-of-subject wildcard) and `*` (single-token) are first-class
        // NATS syntax. Neither should be url-encoded or rewritten.
        for (url, expected) in [
            ("nats://h:4222/subject/orp.>", "orp.>"),
            ("nats://h:4222/subject/orp.*", "orp.*"),
            (
                "nats://h:4222/subject/orp.events.*.east",
                "orp.events.*.east",
            ),
            ("nats://h:4222/stream/S/subject/orp.>", "orp.>"),
            ("nats://h:4222/stream/S/subject/a.*.b.>", "a.*.b.>"),
        ] {
            let target = parse_url(Some(url)).unwrap_or_else(|e| panic!("{url}: {e:?}"));
            assert_eq!(target.subject, expected, "url = {url}");
        }
    }

    #[test]
    fn durable_name_sanitizes_special_chars() {
        assert_eq!(durable_consumer_name("abc"), "orp-abc");
        assert_eq!(durable_consumer_name("a.b.c"), "orp-a_b_c");
        assert_eq!(durable_consumer_name("with space"), "orp-with_space");
        assert_eq!(durable_consumer_name("ok-1_2"), "orp-ok-1_2");
    }

    #[test]
    fn from_connector_config_picks_up_user_pass_auth() {
        let mut props = HashMap::new();
        props.insert("nats_user".into(), json!("alice"));
        props.insert("nats_password".into(), json!("secret"));
        let conn = NatsConnector::from_connector_config(build_config(
            Some("nats://localhost:4222/subject/orp.x"),
            props,
        ))
        .unwrap();
        assert_eq!(
            conn.auth(),
            &NatsAuth::UserPass {
                user: "alice".into(),
                pass: "secret".into()
            }
        );
    }

    #[tokio::test]
    async fn stop_without_start_is_a_noop() {
        let conn = NatsConnector::from_connector_config(build_config(
            Some("nats://localhost:4222/subject/orp.x"),
            HashMap::new(),
        ))
        .unwrap();
        conn.stop().await.expect("stop ok");
        assert!(conn.health_check().await.is_err());
    }

    #[test]
    fn stats_exposes_zero_before_first_event() {
        let conn = NatsConnector::from_connector_config(build_config(
            Some("nats://localhost:4222/subject/orp.x"),
            HashMap::new(),
        ))
        .unwrap();
        let s = conn.stats();
        assert_eq!(s.events_processed, 0);
        assert_eq!(s.errors, 0);
        assert!(s.last_event_timestamp.is_none());
    }
}
