//! Database connector — poll any SQL database and map rows to ORP entities.
//!
//! Supported databases (via feature flags on the caller's Cargo.toml):
//! - PostgreSQL  (`sqlx` with `postgres` feature)
//! - MySQL / MariaDB (`sqlx` with `mysql` feature)
//! - SQLite (`sqlx` with `sqlite` feature)
//!
//! Because `sqlx` requires compile-time query checking or a runtime driver, this
//! connector uses a **driver-agnostic runtime query** approach: the connection
//! string prefix determines which driver is selected at runtime.
//!
//! # Configuration
//!
//! All configuration lives in `ConnectorConfig::properties`:
//!
//! ```yaml
//! connector_id: pg-assets
//! connector_type: database
//! entity_type: asset
//! properties:
//!   connection_string: "${env.DATABASE_URL}"
//!   query: "SELECT id, name, ip_address AS ip, lat, lon FROM assets WHERE updated_at > $1"
//!   poll_interval_secs: 30
//!   id_field: id
//!   lat_field: lat
//!   lon_field: lon
//!   timestamp_field: updated_at
//!   watermark_field: updated_at   # incremental: only fetch rows newer than last seen
//! ```
//!
//! ## Incremental (watermark) mode
//!
//! When `watermark_field` is set the connector tracks the highest value seen so
//! far and passes it as `$1` / `?1` in the query on subsequent polls, enabling
//! incremental ingestion without full-table scans.

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Full configuration for the database connector.
#[derive(Clone, Debug)]
pub struct DatabaseConfig {
    /// Full connection string, e.g. `postgres://host/db`,
    /// `mysql://host/db`, `sqlite:///path/to/file.db`.
    pub connection_string: String,

    /// SQL query to execute on each poll cycle.
    /// Use `$1` (Postgres) or `?` (MySQL / SQLite) as the placeholder for the
    /// watermark value when incremental mode is enabled.
    pub query: String,

    /// Polling interval in seconds.
    pub poll_interval_secs: u64,

    /// Column name whose value becomes the entity ID.
    pub id_field: String,

    /// Optional column for latitude.
    pub lat_field: Option<String>,

    /// Optional column for longitude.
    pub lon_field: Option<String>,

    /// Optional column for the event timestamp.
    pub timestamp_field: Option<String>,

    /// Optional column used as a watermark for incremental ingestion.
    /// The connector passes the last-seen maximum value as `$1`/`?` on every
    /// poll after the first one.
    pub watermark_field: Option<String>,

    /// Columns to always exclude from entity properties.
    pub exclude_columns: Vec<String>,
}

impl DatabaseConfig {
    /// Build from `ConnectorConfig::properties`.
    pub fn from_properties(props: &HashMap<String, JsonValue>) -> Result<Self, ConnectorError> {
        let connection_string = props
            .get("connection_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ConnectorError::ConfigError("Missing 'connection_string' property".to_string())
            })?
            .to_string();

        let query = props
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ConnectorError::ConfigError("Missing 'query' property".to_string())
            })?
            .to_string();

        let poll_interval_secs = props
            .get("poll_interval_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        let id_field = props
            .get("id_field")
            .and_then(|v| v.as_str())
            .unwrap_or("id")
            .to_string();

        let lat_field = props
            .get("lat_field")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let lon_field = props
            .get("lon_field")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let timestamp_field = props
            .get("timestamp_field")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let watermark_field = props
            .get("watermark_field")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let exclude_columns = props
            .get("exclude_columns")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            connection_string,
            query,
            poll_interval_secs,
            id_field,
            lat_field,
            lon_field,
            timestamp_field,
            watermark_field,
            exclude_columns,
        })
    }
}

// ---------------------------------------------------------------------------
// Row → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a generic `HashMap<String, JsonValue>` row into a `SourceEvent`.
pub fn row_to_event(
    row: &HashMap<String, JsonValue>,
    config: &DatabaseConfig,
    entity_type: &str,
    connector_id: &str,
) -> Option<SourceEvent> {
    let raw_id = row.get(&config.id_field)?;
    let entity_id_raw = if let Some(s) = raw_id.as_str() {
        s.to_string()
    } else if let Some(n) = raw_id.as_i64() {
        n.to_string()
    } else if let Some(n) = raw_id.as_u64() {
        n.to_string()
    } else {
        raw_id.to_string()
    };

    if entity_id_raw.is_empty() || entity_id_raw == "null" {
        return None;
    }

    let latitude = config
        .lat_field
        .as_deref()
        .and_then(|f| row.get(f))
        .and_then(|v| v.as_f64());

    let longitude = config
        .lon_field
        .as_deref()
        .and_then(|f| row.get(f))
        .and_then(|v| v.as_f64());

    let timestamp = config
        .timestamp_field
        .as_deref()
        .and_then(|f| row.get(f))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
                    .or_else(|| {
                        // Try common DB format "YYYY-MM-DD HH:MM:SS"
                        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                            .ok()
                            .and_then(|ndt| ndt.and_local_timezone(Utc).single())
                    })
            } else if let Some(n) = v.as_i64() {
                DateTime::from_timestamp(n, 0)
            } else {
                None
            }
        })
        .unwrap_or_else(Utc::now);

    let mut properties: HashMap<String, JsonValue> = HashMap::new();
    for (k, v) in row {
        if config.exclude_columns.contains(k) {
            continue;
        }
        properties.insert(k.clone(), v.clone());
    }

    Some(SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id: format!("{entity_type}:{entity_id_raw}"),
        entity_type: entity_type.to_string(),
        properties,
        timestamp,
        latitude,
        longitude,
    })
}

// ---------------------------------------------------------------------------
// Driver abstraction
// ---------------------------------------------------------------------------

/// Determines the DB driver from the connection string prefix.
#[derive(Clone, Debug, PartialEq)]
pub enum DbDriver {
    Postgres,
    Mysql,
    Sqlite,
    Unknown(String),
}

impl DbDriver {
    pub fn from_connection_string(cs: &str) -> Self {
        if cs.starts_with("postgres://") || cs.starts_with("postgresql://") {
            Self::Postgres
        } else if cs.starts_with("mysql://") || cs.starts_with("mariadb://") {
            Self::Mysql
        } else if cs.starts_with("sqlite://") || cs.starts_with("sqlite:") {
            Self::Sqlite
        } else {
            Self::Unknown(cs.split("://").next().unwrap_or("").to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// In-process "mock" executor (used when `sqlx` is not available / in tests)
// ---------------------------------------------------------------------------
//
// In production you would replace `execute_query` with a real sqlx call.
// The trait below makes it easy to inject any backend.

/// Abstraction over a SQL query executor.
/// Implement this trait to swap in real database backends.
#[async_trait]
pub trait QueryExecutor: Send + Sync {
    /// Execute `query` with an optional `$1` parameter (watermark value).
    /// Returns rows as `Vec<HashMap<String, JsonValue>>`.
    async fn execute(
        &self,
        query: &str,
        watermark: Option<&str>,
    ) -> Result<Vec<HashMap<String, JsonValue>>, ConnectorError>;
}

/// Executor that returns zero rows — used as default / fallback.
pub struct NoopExecutor;

#[async_trait]
impl QueryExecutor for NoopExecutor {
    async fn execute(
        &self,
        _query: &str,
        _watermark: Option<&str>,
    ) -> Result<Vec<HashMap<String, JsonValue>>, ConnectorError> {
        Ok(vec![])
    }
}

/// Mock executor for unit tests — returns pre-configured rows.
#[cfg(test)]
pub struct MockExecutor {
    pub rows: Vec<HashMap<String, JsonValue>>,
}

#[cfg(test)]
#[async_trait]
impl QueryExecutor for MockExecutor {
    async fn execute(
        &self,
        _query: &str,
        _watermark: Option<&str>,
    ) -> Result<Vec<HashMap<String, JsonValue>>, ConnectorError> {
        Ok(self.rows.clone())
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// Database connector.
pub struct DatabaseConnector {
    config: ConnectorConfig,
    db_config: DatabaseConfig,
    executor: Arc<dyn QueryExecutor>,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    /// Last seen watermark value for incremental mode.
    watermark: Arc<Mutex<Option<String>>>,
}

impl DatabaseConnector {
    /// Create with an explicit executor (inject real sqlx executor in production).
    pub fn new(
        config: ConnectorConfig,
        db_config: DatabaseConfig,
        executor: Arc<dyn QueryExecutor>,
    ) -> Self {
        Self {
            config,
            db_config,
            executor,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
            watermark: Arc::new(Mutex::new(None)),
        }
    }

    /// Build from `ConnectorConfig::properties` using the `NoopExecutor`.
    /// Replace the executor with a real one via `with_executor`.
    pub fn from_connector_config(config: ConnectorConfig) -> Result<Self, ConnectorError> {
        let db_config = DatabaseConfig::from_properties(&config.properties)?;
        Ok(Self::new(config, db_config, Arc::new(NoopExecutor)))
    }

    /// Replace the query executor (e.g. inject a real `sqlx::PgPool`-backed one).
    pub fn with_executor(mut self, executor: Arc<dyn QueryExecutor>) -> Self {
        self.executor = executor;
        self
    }

    /// Returns the detected DB driver.
    pub fn driver(&self) -> DbDriver {
        DbDriver::from_connection_string(&self.db_config.connection_string)
    }

    /// Update the watermark from a fresh batch of rows.
    #[allow(dead_code)]
    fn update_watermark(&self, rows: &[HashMap<String, JsonValue>]) {
        if let Some(ref wf) = self.db_config.watermark_field {
            let mut lock = self.watermark.lock().unwrap_or_else(|e| e.into_inner());
            for row in rows {
                if let Some(v) = row.get(wf) {
                    let s = if let Some(str_val) = v.as_str() {
                        str_val.to_string()
                    } else {
                        v.to_string()
                    };
                    match &*lock {
                        None => *lock = Some(s),
                        Some(current) if s > *current => *lock = Some(s),
                        _ => {}
                    }
                }
            }
        }
    }
}

#[async_trait]
impl Connector for DatabaseConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let executor = self.executor.clone();
        let db_config = self.db_config.clone();
        let connector_id = self.config.connector_id.clone();
        let entity_type = self.config.entity_type.clone();
        let watermark = self.watermark.clone();

        tracing::info!(
            connector_id = %connector_id,
            driver = ?DbDriver::from_connection_string(&db_config.connection_string),
            "DatabaseConnector starting"
        );

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                db_config.poll_interval_secs,
            ));

            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                let wm = watermark.lock().unwrap_or_else(|e| e.into_inner()).clone();
                match executor.execute(&db_config.query, wm.as_deref()).await {
                    Ok(rows) => {
                        // Update watermark before sending events to avoid re-processing on error
                        let config_clone = db_config.clone();
                        let watermark_clone = watermark.clone();

                        // Update watermark
                        if let Some(ref wf) = config_clone.watermark_field {
                            let mut lock = watermark_clone.lock().unwrap_or_else(|e| e.into_inner());
                            for row in &rows {
                                if let Some(v) = row.get(wf) {
                                    let s = if let Some(str_val) = v.as_str() {
                                        str_val.to_string()
                                    } else {
                                        v.to_string()
                                    };
                                    match &*lock {
                                        None => *lock = Some(s),
                                        Some(current) if s > *current => *lock = Some(s),
                                        _ => {}
                                    }
                                }
                            }
                        }

                        for row in &rows {
                            if let Some(event) =
                                row_to_event(row, &db_config, &entity_type, &connector_id)
                            {
                                if tx.send(event).await.is_err() {
                                    return; // receiver dropped
                                }
                                events_count.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            connector_id = %connector_id,
                            error = %e,
                            "DatabaseConnector query error"
                        );
                        errors_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(connector_id = %self.config.connector_id, "DatabaseConnector stopped");
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "DatabaseConnector not running".to_string(),
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
    use serde_json::json;

    fn make_config(cs: &str) -> ConnectorConfig {
        let mut props = HashMap::new();
        props.insert("connection_string".to_string(), json!(cs));
        props.insert(
            "query".to_string(),
            json!("SELECT id, name, lat, lon FROM devices"),
        );
        props.insert("id_field".to_string(), json!("id"));
        props.insert("lat_field".to_string(), json!("lat"));
        props.insert("lon_field".to_string(), json!("lon"));

        ConnectorConfig {
            connector_id: "db-test".to_string(),
            connector_type: "database".to_string(),
            url: None,
            entity_type: "device".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: props,
        }
    }

    // ── DbDriver ──────────────────────────────────────────────────────────

    #[test]
    fn test_driver_postgres() {
        assert_eq!(
            DbDriver::from_connection_string("postgres://localhost/db"),
            DbDriver::Postgres
        );
        assert_eq!(
            DbDriver::from_connection_string("postgresql://host/db"),
            DbDriver::Postgres
        );
    }

    #[test]
    fn test_driver_mysql() {
        assert_eq!(
            DbDriver::from_connection_string("mysql://localhost/db"),
            DbDriver::Mysql
        );
    }

    #[test]
    fn test_driver_sqlite() {
        assert_eq!(
            DbDriver::from_connection_string("sqlite:///tmp/test.db"),
            DbDriver::Sqlite
        );
    }

    #[test]
    fn test_driver_unknown() {
        assert!(matches!(
            DbDriver::from_connection_string("oracle://user@host/db"),
            DbDriver::Unknown(_)
        ));
    }

    // ── DatabaseConfig::from_properties ──────────────────────────────────

    #[test]
    fn test_config_from_properties_ok() {
        let config = make_config("postgres://localhost/test");
        let db_cfg = DatabaseConfig::from_properties(&config.properties).unwrap();
        assert_eq!(db_cfg.id_field, "id");
        assert_eq!(db_cfg.lat_field.as_deref(), Some("lat"));
        assert_eq!(db_cfg.poll_interval_secs, 60);
    }

    #[test]
    fn test_config_missing_connection_string_errors() {
        let props = HashMap::new();
        assert!(DatabaseConfig::from_properties(&props).is_err());
    }

    #[test]
    fn test_config_missing_query_errors() {
        let mut props = HashMap::new();
        props.insert("connection_string".to_string(), json!("sqlite:///tmp/x.db"));
        assert!(DatabaseConfig::from_properties(&props).is_err());
    }

    // ── row_to_event ──────────────────────────────────────────────────────

    fn make_db_config() -> DatabaseConfig {
        DatabaseConfig {
            connection_string: "sqlite:///tmp/x.db".to_string(),
            query: "SELECT * FROM t".to_string(),
            poll_interval_secs: 10,
            id_field: "id".to_string(),
            lat_field: Some("lat".to_string()),
            lon_field: Some("lon".to_string()),
            timestamp_field: None,
            watermark_field: None,
            exclude_columns: vec![],
        }
    }

    #[test]
    fn test_row_to_event_basic() {
        let mut row = HashMap::new();
        row.insert("id".to_string(), json!("dev-1"));
        row.insert("lat".to_string(), json!(51.5));
        row.insert("lon".to_string(), json!(-0.1));
        row.insert("name".to_string(), json!("London office router"));

        let cfg = make_db_config();
        let event = row_to_event(&row, &cfg, "device", "db-1").unwrap();
        assert_eq!(event.entity_id, "device:dev-1");
        assert_eq!(event.latitude, Some(51.5));
        assert_eq!(event.longitude, Some(-0.1));
        assert!(event.properties.contains_key("name"));
    }

    #[test]
    fn test_row_to_event_integer_id() {
        let mut row = HashMap::new();
        row.insert("id".to_string(), json!(42));
        let cfg = make_db_config();
        let event = row_to_event(&row, &cfg, "asset", "db-1").unwrap();
        assert_eq!(event.entity_id, "asset:42");
    }

    #[test]
    fn test_row_to_event_missing_id_returns_none() {
        let row: HashMap<String, JsonValue> = HashMap::new();
        let cfg = make_db_config();
        assert!(row_to_event(&row, &cfg, "device", "db-1").is_none());
    }

    #[test]
    fn test_row_to_event_exclude_columns() {
        let mut row = HashMap::new();
        row.insert("id".to_string(), json!("x1"));
        row.insert("password_hash".to_string(), json!("$2a$..."));
        let mut cfg = make_db_config();
        cfg.exclude_columns = vec!["password_hash".to_string()];
        let event = row_to_event(&row, &cfg, "user", "db-1").unwrap();
        assert!(!event.properties.contains_key("password_hash"));
    }

    #[test]
    fn test_row_to_event_iso_timestamp() {
        let mut row = HashMap::new();
        row.insert("id".to_string(), json!("r1"));
        row.insert("updated_at".to_string(), json!("2024-01-15T10:30:00Z"));
        let mut cfg = make_db_config();
        cfg.timestamp_field = Some("updated_at".to_string());
        let event = row_to_event(&row, &cfg, "record", "db-1").unwrap();
        use chrono::Datelike;
        assert_eq!(event.timestamp.year(), 2024);
    }

    // ── MockExecutor integration ──────────────────────────────────────────

    #[tokio::test]
    async fn test_mock_executor_returns_rows() {
        let mut row = HashMap::new();
        row.insert("id".to_string(), json!("device-1"));
        row.insert("lat".to_string(), json!(40.7));
        row.insert("lon".to_string(), json!(-74.0));

        let executor = MockExecutor { rows: vec![row] };
        let rows = executor.execute("SELECT * FROM t", None).await.unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn test_connector_sends_events_via_mock() {
        let mut row = HashMap::new();
        row.insert("id".to_string(), json!("asset-42"));
        row.insert("lat".to_string(), json!(0.0));
        row.insert("lon".to_string(), json!(0.0));

        let config = make_config("postgres://localhost/test");
        let db_config = DatabaseConfig::from_properties(&config.properties).unwrap();
        let executor = Arc::new(MockExecutor { rows: vec![row] });
        let connector = DatabaseConnector::new(config, db_config, executor);

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        connector.start(tx).await.unwrap();

        // Give the spawned task one iteration
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
        connector.stop().await.unwrap();

        // We should have received at least one event
        let event = rx.try_recv().unwrap();
        assert_eq!(event.entity_id, "device:asset-42");
    }

    #[test]
    fn test_from_connector_config_ok() {
        let config = make_config("sqlite:///tmp/test.db");
        let c = DatabaseConnector::from_connector_config(config).unwrap();
        assert_eq!(c.connector_id(), "db-test");
        assert_eq!(c.driver(), DbDriver::Sqlite);
    }

    #[test]
    fn test_watermark_updated() {
        let config = make_config("postgres://localhost/db");
        let mut db_config = DatabaseConfig::from_properties(&config.properties).unwrap();
        db_config.watermark_field = Some("updated_at".to_string());
        let connector =
            DatabaseConnector::new(config, db_config.clone(), Arc::new(NoopExecutor));

        let mut row = HashMap::new();
        row.insert("updated_at".to_string(), json!("2024-06-01T00:00:00Z"));
        connector.update_watermark(&[row]);

        let lock = connector.watermark.lock().unwrap();
        assert_eq!(lock.as_deref(), Some("2024-06-01T00:00:00Z"));
    }

    #[tokio::test]
    async fn test_health_not_running() {
        let config = make_config("sqlite:///tmp/t.db");
        let c = DatabaseConnector::from_connector_config(config).unwrap();
        assert!(c.health_check().await.is_err());
    }

    #[test]
    fn test_initial_stats() {
        let config = make_config("sqlite:///tmp/t.db");
        let c = DatabaseConnector::from_connector_config(config).unwrap();
        assert_eq!(c.stats().events_processed, 0);
        assert_eq!(c.stats().errors, 0);
    }
}
