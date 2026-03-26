use async_trait::async_trait;
use orp_proto::{Entity, Event, Relationship};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

// Re-export DataSource and StorageStats as canonical definitions for this crate.
// These match the spec (Section 4.1) exactly.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DataSource {
    pub source_id: String,
    pub source_name: String,
    pub source_type: String,
    pub trust_score: f32,
    pub events_ingested: u64,
    pub entities_provided: u64,
    pub error_count: u64,
    pub enabled: bool,
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    pub certificate_fingerprint: Option<String>,
}

impl From<orp_proto::DataSource> for DataSource {
    fn from(ds: orp_proto::DataSource) -> Self {
        Self {
            source_id: ds.source_id,
            source_name: ds.source_name,
            source_type: ds.source_type,
            trust_score: ds.trust_score as f32,
            events_ingested: ds.events_ingested,
            entities_provided: 0,
            error_count: 0,
            enabled: ds.enabled,
            last_heartbeat: None,
            certificate_fingerprint: None,
        }
    }
}

impl From<DataSource> for orp_proto::DataSource {
    fn from(ds: DataSource) -> Self {
        Self {
            source_id: ds.source_id,
            source_name: ds.source_name,
            source_type: ds.source_type,
            trust_score: ds.trust_score as f64,
            events_ingested: ds.events_ingested,
            enabled: ds.enabled,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StorageStats {
    pub total_entities: u64,
    pub total_events: u64,
    pub total_relationships: u64,
    pub database_size_bytes: u64,
    /// Count of audit log entries
    pub audit_log_entries: u64,
    /// Count of registered data sources
    pub data_sources: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("Entity not found: {0}")]
    EntityNotFound(String),
    #[error("Duplicate entity: {0}")]
    DuplicateEntity(String),
    #[error("Query error: {0}")]
    QueryError(String),
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Transaction error: {0}")]
    TransactionError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Not supported: {0}")]
    NotSupported(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

/// Abstract storage interface over DuckDB, Kuzu, and RocksDB backends.
/// All methods are async and Send + Sync to support multi-threaded execution.
#[async_trait]
pub trait Storage: Send + Sync {
    // ── ENTITY OPERATIONS ────────────────────────────────────────────────────

    async fn insert_entity(&self, entity: &Entity) -> StorageResult<()>;
    async fn get_entity(&self, entity_id: &str) -> StorageResult<Option<Entity>>;
    async fn get_entities_by_type(
        &self,
        entity_type: &str,
        limit: usize,
        offset: usize,
    ) -> StorageResult<Vec<Entity>>;
    async fn update_entity_property(
        &self,
        entity_id: &str,
        key: &str,
        value: JsonValue,
    ) -> StorageResult<()>;
    async fn delete_entity(&self, entity_id: &str) -> StorageResult<()>;
    async fn count_entities(&self) -> StorageResult<u64>;

    /// Set canonical entity ID (after entity resolution merges duplicates).
    async fn set_canonical_id(&self, entity_id: &str, canonical_id: &str) -> StorageResult<()>;

    // ── GEOSPATIAL QUERIES ───────────────────────────────────────────────────

    async fn get_entities_in_radius(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
        entity_type: Option<&str>,
    ) -> StorageResult<Vec<Entity>>;

    /// Query entities within a WKT polygon (e.g., "POLYGON((lon1 lat1, lon2 lat2, ...))").
    /// Falls back to bounding-box if spatial extension is unavailable.
    async fn get_entities_in_polygon(
        &self,
        polygon_wkt: &str,
        entity_type: Option<&str>,
    ) -> StorageResult<Vec<Entity>>;

    // ── EVENT OPERATIONS ─────────────────────────────────────────────────────

    async fn insert_event(&self, event: &Event) -> StorageResult<()>;
    async fn get_events_for_entity(
        &self,
        entity_id: &str,
        limit: usize,
    ) -> StorageResult<Vec<Event>>;

    /// Return all events that occurred within [start, end].
    async fn get_events_in_time_range(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> StorageResult<Vec<Event>>;

    /// Return events from entities within radius_km of (lat, lon) during [start, end].
    async fn get_events_in_region(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> StorageResult<Vec<Event>>;

    /// Global event log with optional filters (supports the GET /api/v1/events endpoint).
    #[allow(clippy::too_many_arguments)]
    async fn get_events_global(
        &self,
        entity_id: Option<&str>,
        entity_type: Option<&str>,
        event_type: Option<&str>,
        since: Option<chrono::DateTime<chrono::Utc>>,
        until: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
        offset: usize,
    ) -> StorageResult<Vec<Event>>;

    /// Count events matching the global filter (for pagination).
    async fn count_events_global(
        &self,
        entity_id: Option<&str>,
        entity_type: Option<&str>,
        event_type: Option<&str>,
        since: Option<chrono::DateTime<chrono::Utc>>,
        until: Option<chrono::DateTime<chrono::Utc>>,
    ) -> StorageResult<u64>;

    // ── RELATIONSHIP OPERATIONS ──────────────────────────────────────────────

    async fn insert_relationship(&self, rel: &Relationship) -> StorageResult<()>;
    async fn get_relationships_for_entity(
        &self,
        entity_id: &str,
    ) -> StorageResult<Vec<Relationship>>;

    /// Relationships where `source_entity_id` is the source.
    async fn get_outgoing_relationships(
        &self,
        source_entity_id: &str,
        rel_type: Option<&str>,
    ) -> StorageResult<Vec<Relationship>>;

    /// Relationships where `target_entity_id` is the target.
    async fn get_incoming_relationships(
        &self,
        target_entity_id: &str,
        rel_type: Option<&str>,
    ) -> StorageResult<Vec<Relationship>>;

    // ── GRAPH / PATH OPERATIONS ──────────────────────────────────────────────

    /// Execute a graph-style query (passthrough to Kuzu or DuckDB recursive SQL).
    async fn graph_query(
        &self,
        query_str: &str,
    ) -> StorageResult<Vec<HashMap<String, JsonValue>>>;

    /// BFS/DFS path search between two entity IDs up to `max_hops`.
    /// Returns all valid paths as sequences of Relationships.
    async fn path_query(
        &self,
        source_entity_id: &str,
        target_entity_id: &str,
        max_hops: usize,
    ) -> StorageResult<Vec<Vec<Relationship>>>;

    // ── AUDIT OPERATIONS ─────────────────────────────────────────────────────

    /// Append an entry to the immutable, hash-chained audit log.
    async fn log_audit(
        &self,
        operation: &str,
        entity_type: Option<&str>,
        entity_id: Option<&str>,
        user_id: Option<&str>,
        details: JsonValue,
    ) -> StorageResult<()>;

    // ── DATA SOURCE OPERATIONS ───────────────────────────────────────────────

    async fn register_data_source(&self, source: &orp_proto::DataSource) -> StorageResult<()>;
    async fn get_data_sources(&self) -> StorageResult<Vec<DataSource>>;
    async fn get_data_source(&self, source_id: &str) -> StorageResult<Option<DataSource>>;
    async fn update_data_source(&self, source: &DataSource) -> StorageResult<bool>;
    async fn delete_data_source(&self, source_id: &str) -> StorageResult<bool>;

    /// Record a successful heartbeat from a data source connector.
    async fn update_source_heartbeat(&self, source_id: &str) -> StorageResult<()>;

    // ── TRANSACTION OPERATIONS ───────────────────────────────────────────────

    async fn begin_transaction(&self) -> StorageResult<()>;
    async fn commit_transaction(&self) -> StorageResult<()>;
    async fn rollback_transaction(&self) -> StorageResult<()>;

    // ── SEARCH ───────────────────────────────────────────────────────────────

    async fn search_entities(
        &self,
        query: &str,
        entity_type: Option<&str>,
        limit: usize,
    ) -> StorageResult<Vec<Entity>>;

    // ── ADMINISTRATIVE ───────────────────────────────────────────────────────

    async fn health_check(&self) -> StorageResult<()>;
    async fn get_stats(&self) -> StorageResult<StorageStats>;
}
