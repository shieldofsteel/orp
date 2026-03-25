use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceEvent {
    pub connector_id: String,
    pub entity_id: String,
    pub entity_type: String,
    pub properties: HashMap<String, JsonValue>,
    pub timestamp: DateTime<Utc>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectorConfig {
    pub connector_id: String,
    pub connector_type: String,
    pub url: Option<String>,
    pub entity_type: String,
    pub enabled: bool,
    pub trust_score: f32,
    pub properties: HashMap<String, JsonValue>,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectorStats {
    pub events_processed: u64,
    pub errors: u64,
    pub last_event_timestamp: Option<DateTime<Utc>>,
    pub uptime_seconds: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("Connection error: {0}")]
    ConnectionError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

#[async_trait]
pub trait Connector: Send + Sync {
    fn connector_id(&self) -> &str;
    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError>;
    async fn stop(&self) -> Result<(), ConnectorError>;
    async fn health_check(&self) -> Result<(), ConnectorError>;
    fn config(&self) -> &ConnectorConfig;
    fn stats(&self) -> ConnectorStats;
}
