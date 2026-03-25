use crate::traits::{Storage, StorageError, StorageResult};
use async_trait::async_trait;
use duckdb::{params, Connection};
use orp_proto::{DataSource, Entity, Event, GeoPoint, Relationship, StorageStats};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Mutex;

/// DuckDB-backed storage engine
pub struct DuckDbStorage {
    conn: Mutex<Connection>,
}

impl DuckDbStorage {
    /// Create a new in-memory DuckDB storage
    pub fn new_in_memory() -> StorageResult<Self> {
        let conn =
            Connection::open_in_memory().map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.initialize_schema()?;
        Ok(storage)
    }

    /// Create a new file-based DuckDB storage
    pub fn new_with_path(path: &str) -> StorageResult<Self> {
        // Create parent directories
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| StorageError::IoError(e.to_string()))?;
        }
        let conn =
            Connection::open(path).map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.initialize_schema()?;
        Ok(storage)
    }

    fn initialize_schema(&self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(DUCKDB_SCHEMA)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}

const DUCKDB_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS entities (
    entity_id VARCHAR PRIMARY KEY,
    entity_type VARCHAR NOT NULL,
    canonical_id VARCHAR,
    name VARCHAR,
    confidence FLOAT DEFAULT 1.0,
    is_active BOOLEAN DEFAULT true,
    latitude DOUBLE,
    longitude DOUBLE,
    altitude DOUBLE,
    properties JSON,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS events (
    event_id VARCHAR PRIMARY KEY,
    entity_id VARCHAR NOT NULL,
    event_type VARCHAR NOT NULL,
    event_timestamp TIMESTAMP NOT NULL,
    source_id VARCHAR NOT NULL,
    data JSON NOT NULL,
    confidence FLOAT DEFAULT 1.0,
    ingestion_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS relationships (
    relationship_id VARCHAR PRIMARY KEY,
    source_entity_id VARCHAR NOT NULL,
    target_entity_id VARCHAR NOT NULL,
    relationship_type VARCHAR NOT NULL,
    properties JSON,
    confidence FLOAT DEFAULT 1.0,
    is_active BOOLEAN DEFAULT true,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS data_sources (
    source_id VARCHAR PRIMARY KEY,
    source_name VARCHAR NOT NULL,
    source_type VARCHAR NOT NULL,
    trust_score FLOAT DEFAULT 0.8,
    events_ingested BIGINT DEFAULT 0,
    enabled BOOLEAN DEFAULT true,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS audit_log (
    sequence_number BIGINT PRIMARY KEY,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
    operation VARCHAR NOT NULL,
    entity_type VARCHAR,
    entity_id VARCHAR,
    user_id VARCHAR,
    previous_hash VARCHAR,
    content_hash VARCHAR NOT NULL,
    details JSON
);
"#;

#[async_trait]
impl Storage for DuckDbStorage {
    async fn insert_entity(&self, entity: &Entity) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        let props_json = serde_json::to_string(&entity.properties)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let (lat, lon, alt) = entity
            .geometry
            .as_ref()
            .map(|g| (Some(g.lat), Some(g.lon), g.alt))
            .unwrap_or((None, None, None));

        conn.execute(
            "INSERT OR REPLACE INTO entities (entity_id, entity_type, canonical_id, name, confidence, is_active, latitude, longitude, altitude, properties, last_updated) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
            params![
                entity.entity_id,
                entity.entity_type,
                entity.canonical_id,
                entity.name,
                entity.confidence,
                entity.is_active,
                lat,
                lon,
                alt,
                props_json,
            ],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn get_entity(&self, entity_id: &str) -> StorageResult<Option<Entity>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT entity_id, entity_type, canonical_id, name, confidence, is_active, latitude, longitude, altitude, properties, last_updated FROM entities WHERE entity_id = ?")
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query(params![entity_id])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            let lat: Option<f64> = row.get(6).ok();
            let lon: Option<f64> = row.get(7).ok();
            let alt: Option<f64> = row.get(8).ok();
            let props_str: String = row.get(9).unwrap_or_default();
            let properties: HashMap<String, JsonValue> =
                serde_json::from_str(&props_str).unwrap_or_default();
            let last_updated_str: String = row.get(10).unwrap_or_default();

            let geometry = match (lat, lon) {
                (Some(lat_v), Some(lon_v)) => Some(GeoPoint {
                    lat: lat_v,
                    lon: lon_v,
                    alt,
                }),
                _ => None,
            };

            Ok(Some(Entity {
                entity_id: row
                    .get(0)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
                entity_type: row
                    .get(1)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
                canonical_id: row.get(2).ok(),
                name: row.get(3).ok(),
                confidence: row.get(4).unwrap_or(1.0),
                is_active: row.get(5).unwrap_or(true),
                last_updated: chrono::DateTime::parse_from_rfc3339(&last_updated_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                geometry,
                properties,
            }))
        } else {
            Ok(None)
        }
    }

    async fn get_entities_by_type(
        &self,
        entity_type: &str,
        limit: usize,
        offset: usize,
    ) -> StorageResult<Vec<Entity>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT entity_id, entity_type, canonical_id, name, confidence, is_active, latitude, longitude, altitude, properties FROM entities WHERE entity_type = ? AND is_active = true LIMIT ? OFFSET ?")
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map(params![entity_type, limit as i64, offset as i64], |row| {
                let lat: Option<f64> = row.get(6).ok();
                let lon: Option<f64> = row.get(7).ok();
                let alt: Option<f64> = row.get(8).ok();
                let props_str: String = row.get(9).unwrap_or_default();

                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2).ok().flatten(),
                    row.get::<_, Option<String>>(3).ok().flatten(),
                    row.get::<_, f32>(4).unwrap_or(1.0),
                    row.get::<_, bool>(5).unwrap_or(true),
                    lat,
                    lon,
                    alt,
                    props_str,
                ))
            })
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut entities = Vec::new();
        for row in rows {
            let (id, etype, canonical, name, confidence, active, lat, lon, alt, props_str) =
                row.map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            let properties: HashMap<String, JsonValue> =
                serde_json::from_str(&props_str).unwrap_or_default();
            let geometry = match (lat, lon) {
                (Some(la), Some(lo)) => Some(GeoPoint {
                    lat: la,
                    lon: lo,
                    alt,
                }),
                _ => None,
            };
            entities.push(Entity {
                entity_id: id,
                entity_type: etype,
                canonical_id: canonical,
                name,
                confidence,
                is_active: active,
                last_updated: chrono::Utc::now(),
                geometry,
                properties,
            });
        }
        Ok(entities)
    }

    async fn update_entity_property(
        &self,
        entity_id: &str,
        key: &str,
        value: JsonValue,
    ) -> StorageResult<()> {
        // Get current entity, update property, write back
        if let Some(mut entity) = self.get_entity(entity_id).await? {
            entity.properties.insert(key.to_string(), value);
            self.insert_entity(&entity).await?;
            Ok(())
        } else {
            Err(StorageError::EntityNotFound(entity_id.to_string()))
        }
    }

    async fn delete_entity(&self, entity_id: &str) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE entities SET is_active = false WHERE entity_id = ?",
            params![entity_id],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn count_entities(&self) -> StorageResult<u64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entities WHERE is_active = true",
                [],
                |row| row.get(0),
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(count as u64)
    }

    async fn get_entities_in_radius(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
        entity_type: Option<&str>,
    ) -> StorageResult<Vec<Entity>> {
        // Haversine approximation: ~111km per degree
        let deg_range = radius_km / 111.0;
        let conn = self.conn.lock().unwrap();

        let query = if let Some(etype) = entity_type {
            format!(
                "SELECT entity_id, entity_type, canonical_id, name, confidence, is_active, latitude, longitude, altitude, properties
                 FROM entities
                 WHERE is_active = true AND entity_type = '{}'
                 AND latitude BETWEEN {} AND {}
                 AND longitude BETWEEN {} AND {}",
                etype,
                lat - deg_range,
                lat + deg_range,
                lon - deg_range,
                lon + deg_range,
            )
        } else {
            format!(
                "SELECT entity_id, entity_type, canonical_id, name, confidence, is_active, latitude, longitude, altitude, properties
                 FROM entities
                 WHERE is_active = true
                 AND latitude BETWEEN {} AND {}
                 AND longitude BETWEEN {} AND {}",
                lat - deg_range,
                lat + deg_range,
                lon - deg_range,
                lon + deg_range,
            )
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2).ok().flatten(),
                    row.get::<_, Option<String>>(3).ok().flatten(),
                    row.get::<_, f32>(4).unwrap_or(1.0),
                    row.get::<_, bool>(5).unwrap_or(true),
                    row.get::<_, Option<f64>>(6).ok().flatten(),
                    row.get::<_, Option<f64>>(7).ok().flatten(),
                    row.get::<_, Option<f64>>(8).ok().flatten(),
                    row.get::<_, String>(9).unwrap_or_default(),
                ))
            })
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut entities = Vec::new();
        for row_result in rows {
            let (id, etype, canonical, name, confidence, active, r_lat, r_lon, alt, props_str) =
                row_result.map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            let properties: HashMap<String, JsonValue> =
                serde_json::from_str(&props_str).unwrap_or_default();
            let geometry = match (r_lat, r_lon) {
                (Some(la), Some(lo)) => Some(GeoPoint {
                    lat: la,
                    lon: lo,
                    alt,
                }),
                _ => None,
            };
            entities.push(Entity {
                entity_id: id,
                entity_type: etype,
                canonical_id: canonical,
                name,
                confidence,
                is_active: active,
                last_updated: chrono::Utc::now(),
                geometry,
                properties,
            });
        }
        Ok(entities)
    }

    async fn insert_event(&self, event: &Event) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        let data_str = serde_json::to_string(&event.data)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO events (event_id, entity_id, event_type, event_timestamp, source_id, data, confidence) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                event.event_id,
                event.entity_id,
                event.event_type,
                event.event_timestamp.to_rfc3339(),
                event.source_id,
                data_str,
                event.confidence,
            ],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn get_events_for_entity(
        &self,
        entity_id: &str,
        limit: usize,
    ) -> StorageResult<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT event_id, entity_id, event_type, CAST(event_timestamp AS VARCHAR), source_id, data, confidence FROM events WHERE entity_id = ? ORDER BY event_timestamp DESC LIMIT ?")
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map(params![entity_id, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3).unwrap_or_default(),
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, f32>(6).unwrap_or(1.0),
                ))
            })
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut events = Vec::new();
        for row in rows {
            let (eid, entity_id, etype, ts_str, source, data_str, confidence) =
                row.map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            let data: JsonValue = serde_json::from_str(&data_str).unwrap_or(JsonValue::Null);
            events.push(Event {
                event_id: eid,
                entity_id,
                event_type: etype,
                event_timestamp: chrono::DateTime::parse_from_rfc3339(&ts_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| {
                        // Try parsing DuckDB timestamp format: "2026-03-26 14:30:00"
                        chrono::NaiveDateTime::parse_from_str(&ts_str, "%Y-%m-%d %H:%M:%S")
                            .or_else(|_| chrono::NaiveDateTime::parse_from_str(&ts_str, "%Y-%m-%d %H:%M:%S%.f"))
                            .map(|ndt| ndt.and_utc())
                            .unwrap_or_else(|_| chrono::Utc::now())
                    }),
                source_id: source,
                data,
                confidence,
            });
        }
        Ok(events)
    }

    async fn insert_relationship(&self, rel: &Relationship) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        let props_json = serde_json::to_string(&rel.properties)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO relationships (relationship_id, source_entity_id, target_entity_id, relationship_type, properties, confidence, is_active) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                rel.relationship_id,
                rel.source_entity_id,
                rel.target_entity_id,
                rel.relationship_type,
                props_json,
                rel.confidence,
                rel.is_active,
            ],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn get_relationships_for_entity(
        &self,
        entity_id: &str,
    ) -> StorageResult<Vec<Relationship>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT relationship_id, source_entity_id, target_entity_id, relationship_type, properties, confidence, is_active, created_at, updated_at FROM relationships WHERE (source_entity_id = ? OR target_entity_id = ?) AND is_active = true")
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map(params![entity_id, entity_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4).unwrap_or_default(),
                    row.get::<_, f32>(5).unwrap_or(1.0),
                    row.get::<_, bool>(6).unwrap_or(true),
                ))
            })
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rels = Vec::new();
        for row in rows {
            let (rid, src, tgt, rtype, props_str, confidence, active) =
                row.map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            let properties: HashMap<String, JsonValue> =
                serde_json::from_str(&props_str).unwrap_or_default();
            rels.push(Relationship {
                relationship_id: rid,
                source_entity_id: src,
                target_entity_id: tgt,
                relationship_type: rtype,
                properties,
                confidence,
                is_active: active,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            });
        }
        Ok(rels)
    }

    async fn register_data_source(&self, source: &DataSource) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO data_sources (source_id, source_name, source_type, trust_score, events_ingested, enabled) VALUES (?, ?, ?, ?, ?, ?)",
            params![
                source.source_id,
                source.source_name,
                source.source_type,
                source.trust_score,
                source.events_ingested as i64,
                source.enabled,
            ],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn get_data_sources(&self) -> StorageResult<Vec<DataSource>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT source_id, source_name, source_type, trust_score, events_ingested, enabled FROM data_sources")
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(DataSource {
                    source_id: row.get(0)?,
                    source_name: row.get(1)?,
                    source_type: row.get(2)?,
                    trust_score: row.get(3).unwrap_or(0.8),
                    events_ingested: row.get::<_, i64>(4).unwrap_or(0) as u64,
                    enabled: row.get(5).unwrap_or(true),
                })
            })
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut sources = Vec::new();
        for row in rows {
            sources.push(row.map_err(|e| StorageError::DatabaseError(e.to_string()))?);
        }
        Ok(sources)
    }

    async fn search_entities(
        &self,
        query: &str,
        entity_type: Option<&str>,
        limit: usize,
    ) -> StorageResult<Vec<Entity>> {
        let conn = self.conn.lock().unwrap();
        let search_pattern = format!("%{}%", query);

        let sql = if let Some(etype) = entity_type {
            format!(
                "SELECT entity_id, entity_type, canonical_id, name, confidence, is_active, latitude, longitude, altitude, properties
                 FROM entities
                 WHERE is_active = true AND entity_type = '{}'
                 AND (name LIKE '{}' OR entity_id LIKE '{}')
                 LIMIT {}",
                etype, search_pattern, search_pattern, limit,
            )
        } else {
            format!(
                "SELECT entity_id, entity_type, canonical_id, name, confidence, is_active, latitude, longitude, altitude, properties
                 FROM entities
                 WHERE is_active = true
                 AND (name LIKE '{}' OR entity_id LIKE '{}')
                 LIMIT {}",
                search_pattern, search_pattern, limit,
            )
        };

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2).ok().flatten(),
                    row.get::<_, Option<String>>(3).ok().flatten(),
                    row.get::<_, f32>(4).unwrap_or(1.0),
                    row.get::<_, bool>(5).unwrap_or(true),
                    row.get::<_, Option<f64>>(6).ok().flatten(),
                    row.get::<_, Option<f64>>(7).ok().flatten(),
                    row.get::<_, Option<f64>>(8).ok().flatten(),
                    row.get::<_, String>(9).unwrap_or_default(),
                ))
            })
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut entities = Vec::new();
        for row_result in rows {
            let (id, etype, canonical, name, confidence, active, lat, lon, alt, props_str) =
                row_result.map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            let properties: HashMap<String, JsonValue> =
                serde_json::from_str(&props_str).unwrap_or_default();
            let geometry = match (lat, lon) {
                (Some(la), Some(lo)) => Some(GeoPoint {
                    lat: la,
                    lon: lo,
                    alt,
                }),
                _ => None,
            };
            entities.push(Entity {
                entity_id: id,
                entity_type: etype,
                canonical_id: canonical,
                name,
                confidence,
                is_active: active,
                last_updated: chrono::Utc::now(),
                geometry,
                properties,
            });
        }
        Ok(entities)
    }

    async fn graph_query(
        &self,
        _query_str: &str,
    ) -> StorageResult<Vec<HashMap<String, JsonValue>>> {
        // Stub: graph queries will be routed to Kuzu in production
        Ok(vec![])
    }

    async fn get_stats(&self) -> StorageResult<StorageStats> {
        let conn = self.conn.lock().unwrap();

        let total_entities: i64 = conn
            .query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))
            .unwrap_or(0);
        let total_events: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap_or(0);
        let total_relationships: i64 = conn
            .query_row("SELECT COUNT(*) FROM relationships", [], |row| row.get(0))
            .unwrap_or(0);

        Ok(StorageStats {
            total_entities: total_entities as u64,
            total_events: total_events as u64,
            total_relationships: total_relationships as u64,
            database_size_bytes: 0,
        })
    }

    async fn health_check(&self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT 1", [], |_| Ok(()))
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_insert_and_get_entity() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let entity = Entity {
            entity_id: "ship-test-1".to_string(),
            entity_type: "ship".to_string(),
            name: Some("Test Ship".to_string()),
            geometry: Some(GeoPoint {
                lat: 51.92,
                lon: 4.47,
                alt: None,
            }),
            ..Entity::default()
        };

        storage.insert_entity(&entity).await.unwrap();

        let retrieved = storage.get_entity("ship-test-1").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.entity_type, "ship");
        assert_eq!(retrieved.name, Some("Test Ship".to_string()));
    }

    #[tokio::test]
    async fn test_entities_by_type() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        for i in 0..5 {
            let entity = Entity {
                entity_id: format!("ship-{}", i),
                entity_type: "ship".to_string(),
                name: Some(format!("Ship {}", i)),
                ..Entity::default()
            };
            storage.insert_entity(&entity).await.unwrap();
        }

        let ships = storage.get_entities_by_type("ship", 10, 0).await.unwrap();
        assert_eq!(ships.len(), 5);
    }

    #[tokio::test]
    async fn test_geospatial_query() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let entity = Entity {
            entity_id: "ship-near".to_string(),
            entity_type: "ship".to_string(),
            geometry: Some(GeoPoint {
                lat: 51.92,
                lon: 4.47,
                alt: None,
            }),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        let entity_far = Entity {
            entity_id: "ship-far".to_string(),
            entity_type: "ship".to_string(),
            geometry: Some(GeoPoint {
                lat: 35.0,
                lon: 139.0,
                alt: None,
            }),
            ..Entity::default()
        };
        storage.insert_entity(&entity_far).await.unwrap();

        let near = storage
            .get_entities_in_radius(51.92, 4.47, 50.0, Some("ship"))
            .await
            .unwrap();
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].entity_id, "ship-near");
    }

    #[tokio::test]
    async fn test_stats() {
        let storage = DuckDbStorage::new_in_memory().unwrap();
        let stats = storage.get_stats().await.unwrap();
        assert_eq!(stats.total_entities, 0);
    }

    #[tokio::test]
    async fn test_insert_and_get_events() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let entity = Entity {
            entity_id: "ship-1".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        for i in 0..5 {
            let event = Event {
                event_id: format!("evt-{}", i),
                entity_id: "ship-1".to_string(),
                event_type: "position_update".to_string(),
                event_timestamp: chrono::Utc::now(),
                source_id: "ais-1".to_string(),
                data: serde_json::json!({"speed": 10.0 + i as f64}),
                confidence: 0.95,
            };
            storage.insert_event(&event).await.unwrap();
        }

        let events = storage.get_events_for_entity("ship-1", 10).await.unwrap();
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn test_insert_and_get_relationships() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let ship = Entity {
            entity_id: "ship-1".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        let port = Entity {
            entity_id: "port-1".to_string(),
            entity_type: "port".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&ship).await.unwrap();
        storage.insert_entity(&port).await.unwrap();

        let rel = orp_proto::Relationship {
            relationship_id: "rel-1".to_string(),
            source_entity_id: "ship-1".to_string(),
            target_entity_id: "port-1".to_string(),
            relationship_type: "HEADING_TO".to_string(),
            properties: HashMap::new(),
            confidence: 0.9,
            is_active: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        storage.insert_relationship(&rel).await.unwrap();

        let rels = storage
            .get_relationships_for_entity("ship-1")
            .await
            .unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].relationship_type, "HEADING_TO");

        let rels2 = storage
            .get_relationships_for_entity("port-1")
            .await
            .unwrap();
        assert_eq!(rels2.len(), 1);
    }

    #[tokio::test]
    async fn test_data_sources() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let source = orp_proto::DataSource {
            source_id: "ais-demo".to_string(),
            source_name: "AIS Demo".to_string(),
            source_type: "ais".to_string(),
            trust_score: 0.95,
            events_ingested: 0,
            enabled: true,
        };
        storage.register_data_source(&source).await.unwrap();

        let sources = storage.get_data_sources().await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_name, "AIS Demo");
    }

    #[tokio::test]
    async fn test_search_entities() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let entity = Entity {
            entity_id: "ship-rotterdam-1".to_string(),
            entity_type: "ship".to_string(),
            name: Some("Rotterdam Express".to_string()),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        let results = storage
            .search_entities("Rotterdam", Some("ship"), 10)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].name.as_deref(), Some("Rotterdam Express"));
    }

    #[tokio::test]
    async fn test_delete_entity() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let entity = Entity {
            entity_id: "ship-del".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        assert!(storage.get_entity("ship-del").await.unwrap().is_some());

        storage.delete_entity("ship-del").await.unwrap();

        // Count should not include soft-deleted
        let count = storage.count_entities().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_update_entity_property() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let entity = Entity {
            entity_id: "ship-upd".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        storage
            .update_entity_property("ship-upd", "speed", serde_json::json!(15.0))
            .await
            .unwrap();

        let updated = storage.get_entity("ship-upd").await.unwrap().unwrap();
        assert_eq!(
            updated.properties.get("speed"),
            Some(&serde_json::json!(15.0))
        );
    }

    #[tokio::test]
    async fn test_health_check() {
        let storage = DuckDbStorage::new_in_memory().unwrap();
        assert!(storage.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_pagination() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        for i in 0..20 {
            let entity = Entity {
                entity_id: format!("ship-page-{}", i),
                entity_type: "ship".to_string(),
                name: Some(format!("Ship {}", i)),
                ..Entity::default()
            };
            storage.insert_entity(&entity).await.unwrap();
        }

        let page1 = storage.get_entities_by_type("ship", 5, 0).await.unwrap();
        assert_eq!(page1.len(), 5);

        let page2 = storage.get_entities_by_type("ship", 5, 5).await.unwrap();
        assert_eq!(page2.len(), 5);

        // No overlap
        let ids1: Vec<_> = page1.iter().map(|e| &e.entity_id).collect();
        let ids2: Vec<_> = page2.iter().map(|e| &e.entity_id).collect();
        for id in &ids1 {
            assert!(!ids2.contains(id));
        }
    }

    #[tokio::test]
    async fn test_geospatial_no_type_filter() {
        let storage = DuckDbStorage::new_in_memory().unwrap();

        let entity = Entity {
            entity_id: "ship-geo".to_string(),
            entity_type: "ship".to_string(),
            geometry: Some(GeoPoint {
                lat: 51.92,
                lon: 4.47,
                alt: None,
            }),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        let results = storage
            .get_entities_in_radius(51.92, 4.47, 10.0, None)
            .await
            .unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_file_based_storage() {
        let dir = std::env::temp_dir().join("orp_test_db");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.duckdb");
        let path_str = path.to_str().unwrap();

        let storage = DuckDbStorage::new_with_path(path_str).unwrap();
        let entity = Entity {
            entity_id: "persist-test".to_string(),
            entity_type: "ship".to_string(),
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        let result = storage.get_entity("persist-test").await.unwrap();
        assert!(result.is_some());

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
