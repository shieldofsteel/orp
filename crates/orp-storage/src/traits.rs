use async_trait::async_trait;
use orp_proto::{DataSource, Entity, Event, Relationship, StorageStats};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

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
}

pub type StorageResult<T> = Result<T, StorageError>;

#[async_trait]
pub trait Storage: Send + Sync {
    // Entity operations
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

    // Geospatial queries
    async fn get_entities_in_radius(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
        entity_type: Option<&str>,
    ) -> StorageResult<Vec<Entity>>;

    // Event operations
    async fn insert_event(&self, event: &Event) -> StorageResult<()>;
    async fn get_events_for_entity(
        &self,
        entity_id: &str,
        limit: usize,
    ) -> StorageResult<Vec<Event>>;

    // Relationship operations
    async fn insert_relationship(&self, rel: &Relationship) -> StorageResult<()>;
    async fn get_relationships_for_entity(
        &self,
        entity_id: &str,
    ) -> StorageResult<Vec<Relationship>>;

    // Data source operations
    async fn register_data_source(&self, source: &DataSource) -> StorageResult<()>;
    async fn get_data_sources(&self) -> StorageResult<Vec<DataSource>>;

    // Search
    async fn search_entities(
        &self,
        query: &str,
        entity_type: Option<&str>,
        limit: usize,
    ) -> StorageResult<Vec<Entity>>;

    // Graph queries (passthrough to Kuzu or DuckDB relationships)
    async fn graph_query(
        &self,
        query_str: &str,
    ) -> StorageResult<Vec<HashMap<String, JsonValue>>>;

    // Stats
    async fn get_stats(&self) -> StorageResult<StorageStats>;

    // Health check
    async fn health_check(&self) -> StorageResult<()>;
}
