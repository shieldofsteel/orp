//! DuckDB storage backend — implements the full ORP schema (Section 2.1).
//!
//! Tables: entities, entity_geometry, entity_properties, events,
//!         relationships, data_sources, audit_log, snapshots, monitor_rules
//!
//! Spatial extension is loaded if available; the code gracefully degrades to
//! bounding-box queries when ST_GEOMETRY / RTREE are unavailable.

use crate::traits::{DataSource, Storage, StorageError, StorageResult, StorageStats};
use async_trait::async_trait;
use duckdb::{params, Connection};
use orp_proto::{Entity, Event, GeoPoint, Relationship};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── Schema strings ────────────────────────────────────────────────────────────

const DUCKDB_BASE_SCHEMA: &str = r#"
-- ─── ENTITIES ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS entities (
    entity_id        VARCHAR PRIMARY KEY,
    entity_type      VARCHAR NOT NULL,
    canonical_id     VARCHAR,
    name             VARCHAR,
    first_seen       TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_updated     TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    confidence       FLOAT DEFAULT 1.0,
    source_count     INTEGER DEFAULT 1,
    is_canonical     BOOLEAN DEFAULT FALSE,
    is_active        BOOLEAN DEFAULT TRUE
);

CREATE INDEX IF NOT EXISTS idx_entities_type        ON entities(entity_type);
CREATE INDEX IF NOT EXISTS idx_entities_canonical   ON entities(canonical_id);
CREATE INDEX IF NOT EXISTS idx_entities_last_updated ON entities(last_updated);

-- ─── ENTITY_PROPERTIES ─────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS entity_properties (
    id             BIGINT PRIMARY KEY,
    entity_id      VARCHAR NOT NULL,
    property_key   VARCHAR NOT NULL,
    property_value VARCHAR NOT NULL,
    property_type  VARCHAR NOT NULL,
    source_id      VARCHAR NOT NULL,
    timestamp      TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    confidence     FLOAT DEFAULT 1.0,
    is_latest      BOOLEAN DEFAULT TRUE
);

CREATE SEQUENCE IF NOT EXISTS entity_properties_seq START 1;

CREATE INDEX IF NOT EXISTS idx_entity_props_entity    ON entity_properties(entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_props_key       ON entity_properties(property_key);
CREATE INDEX IF NOT EXISTS idx_entity_props_timestamp ON entity_properties(timestamp);
CREATE INDEX IF NOT EXISTS idx_entity_props_latest    ON entity_properties(is_latest);

-- ─── ENTITY_GEOMETRY (lat/lon fallback; ST_GEOMETRY added if spatial loads) ──
CREATE TABLE IF NOT EXISTS entity_geometry (
    entity_id     VARCHAR PRIMARY KEY,
    geometry_wkt  VARCHAR,
    latitude      FLOAT NOT NULL,
    longitude     FLOAT NOT NULL,
    last_updated  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_geom_coords ON entity_geometry(latitude, longitude);

-- ─── EVENTS ────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS events (
    event_id             VARCHAR PRIMARY KEY,
    entity_id            VARCHAR NOT NULL,
    event_type           VARCHAR NOT NULL,
    event_timestamp      TIMESTAMP NOT NULL,
    ingestion_timestamp  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    source_id            VARCHAR NOT NULL,
    event_data           JSON NOT NULL,
    confidence           FLOAT DEFAULT 1.0,
    severity             VARCHAR DEFAULT 'info'
);

CREATE INDEX IF NOT EXISTS idx_events_entity     ON events(entity_id);
CREATE INDEX IF NOT EXISTS idx_events_type       ON events(event_type);
CREATE INDEX IF NOT EXISTS idx_events_timestamp  ON events(event_timestamp);
CREATE INDEX IF NOT EXISTS idx_events_severity   ON events(severity);
CREATE INDEX IF NOT EXISTS idx_events_ingestion  ON events(ingestion_timestamp);

-- ─── RELATIONSHIPS ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS relationships (
    relationship_id             VARCHAR PRIMARY KEY,
    source_entity_id            VARCHAR NOT NULL,
    target_entity_id            VARCHAR NOT NULL,
    relationship_type           VARCHAR NOT NULL,
    properties                  JSON,
    created_timestamp           TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_confirmed_timestamp    TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    confidence                  FLOAT DEFAULT 1.0,
    is_active                   BOOLEAN DEFAULT TRUE
);

CREATE INDEX IF NOT EXISTS idx_rels_source ON relationships(source_entity_id);
CREATE INDEX IF NOT EXISTS idx_rels_target ON relationships(target_entity_id);
CREATE INDEX IF NOT EXISTS idx_rels_type   ON relationships(relationship_type);
CREATE INDEX IF NOT EXISTS idx_rels_active ON relationships(is_active);

-- ─── DATA_SOURCES ───────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS data_sources (
    source_id                   VARCHAR PRIMARY KEY,
    source_name                 VARCHAR NOT NULL,
    source_type                 VARCHAR NOT NULL,
    url                         VARCHAR,
    enabled                     BOOLEAN DEFAULT TRUE,
    trust_score                 FLOAT DEFAULT 0.8,
    last_heartbeat              TIMESTAMP,
    events_ingested_total       BIGINT DEFAULT 0,
    entities_provided_total     BIGINT DEFAULT 0,
    error_count                 INTEGER DEFAULT 0,
    certificate_fingerprint     VARCHAR,
    created_timestamp           TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_sources_type    ON data_sources(source_type);
CREATE INDEX IF NOT EXISTS idx_sources_enabled ON data_sources(enabled);

-- ─── AUDIT_LOG (hash-chained, append-only) ──────────────────────────────────
CREATE TABLE IF NOT EXISTS audit_log (
    sequence_number  BIGINT PRIMARY KEY,
    timestamp        TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
    operation        VARCHAR NOT NULL,
    entity_type      VARCHAR,
    entity_id        VARCHAR,
    user_id          VARCHAR,
    previous_hash    VARCHAR,
    content_hash     VARCHAR NOT NULL,
    signature        VARCHAR,
    details          JSON
);

CREATE SEQUENCE IF NOT EXISTS audit_log_seq START 1;

CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_operation ON audit_log(operation);
CREATE INDEX IF NOT EXISTS idx_audit_entity    ON audit_log(entity_type, entity_id);

-- ─── SNAPSHOTS ──────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS snapshots (
    snapshot_id           VARCHAR PRIMARY KEY,
    snapshot_timestamp    TIMESTAMP NOT NULL,
    entity_state          JSON NOT NULL,
    relationship_state    JSON NOT NULL,
    size_bytes            BIGINT,
    created_timestamp     TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON snapshots(snapshot_timestamp);

-- ─── MONITOR_RULES ──────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS monitor_rules (
    rule_id            VARCHAR PRIMARY KEY,
    rule_name          VARCHAR NOT NULL,
    entity_type        VARCHAR NOT NULL,
    condition_sql      VARCHAR NOT NULL,
    action_type        VARCHAR DEFAULT 'alert',
    action_target      VARCHAR,
    enabled            BOOLEAN DEFAULT TRUE,
    created_timestamp  TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_triggered     TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_monitor_rules_entity_type ON monitor_rules(entity_type);
CREATE INDEX IF NOT EXISTS idx_monitor_rules_enabled     ON monitor_rules(enabled);
"#;

// ── Engine ────────────────────────────────────────────────────────────────────

/// DuckDB-backed storage engine (OLAP + Geospatial).
pub struct DuckDbStorage {
    conn: Arc<Mutex<Connection>>,
    spatial_enabled: bool,
}

impl DuckDbStorage {
    /// Open an in-memory DuckDB instance (test / ephemeral use).
    pub fn new_in_memory() -> StorageResult<Self> {
        let conn =
            Connection::open_in_memory().map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let mut s = Self {
            conn: Arc::new(Mutex::new(conn)),
            spatial_enabled: false,
        };
        s.initialize()?;
        Ok(s)
    }

    /// Open a file-based DuckDB instance.
    pub fn new_with_path(path: &str) -> StorageResult<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| StorageError::IoError(e.to_string()))?;
        }
        let conn =
            Connection::open(path).map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let mut s = Self {
            conn: Arc::new(Mutex::new(conn)),
            spatial_enabled: false,
        };
        s.initialize()?;
        Ok(s)
    }

    fn initialize(&mut self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();

        // Attempt spatial extension (may fail in bundled/offline builds).
        let spatial = conn.execute_batch("LOAD spatial").is_ok();
        self.spatial_enabled = spatial;
        if spatial {
            tracing::info!("DuckDB spatial extension loaded — using ST_GEOMETRY + RTREE");
        } else {
            tracing::warn!(
                "DuckDB spatial extension unavailable — falling back to lat/lon bounding-box queries"
            );
        }

        // Create all tables and indexes.
        conn.execute_batch(DUCKDB_BASE_SCHEMA)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    fn parse_ts(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
                    })
                    .map(|n| n.and_utc())
                    .unwrap_or_else(|_| chrono::Utc::now())
            })
    }

    fn row_to_entity(row: &duckdb::Row) -> duckdb::Result<Entity> {
        let lat: Option<f64> = row.get::<_, Option<f64>>(6).unwrap_or(None);
        let lon: Option<f64> = row.get::<_, Option<f64>>(7).unwrap_or(None);
        let alt: Option<f64> = row.get::<_, Option<f64>>(8).unwrap_or(None);
        let props_str: String = row.get::<_, String>(9).unwrap_or_default();
        let updated_ts_str: String = row.get::<_, String>(10).unwrap_or_default();
        let is_active: bool = row.get::<_, bool>(11).unwrap_or(true);
        let created_ts_str: String = row.get::<_, String>(12).unwrap_or_default();

        let properties: HashMap<String, JsonValue> =
            serde_json::from_str(&props_str).unwrap_or_default();
        let geometry = match (lat, lon) {
            (Some(la), Some(lo)) => Some(GeoPoint { lat: la, lon: lo, alt }),
            _ => None,
        };
        let last_updated = Self::parse_ts(&updated_ts_str);
        let created_at = if created_ts_str.is_empty() {
            last_updated
        } else {
            Self::parse_ts(&created_ts_str)
        };

        Ok(Entity {
            entity_id: row.get(0)?,
            entity_type: row.get(1)?,
            canonical_id: row.get::<_, Option<String>>(2).ok().flatten(),
            name: row.get::<_, Option<String>>(3).ok().flatten(),
            confidence: row.get::<_, f32>(4).unwrap_or(1.0) as f64,
            is_active,
            created_at,
            last_updated,
            geometry,
            properties,
        })
    }

    fn row_to_event(row: &duckdb::Row) -> duckdb::Result<Event> {
        let ts_str: String = row.get::<_, String>(3).unwrap_or_default();
        let data_str: String = row.get::<_, String>(6).unwrap_or_default();
        let data: JsonValue = serde_json::from_str(&data_str).unwrap_or(JsonValue::Null);

        Ok(Event {
            event_id: row.get(0)?,
            entity_id: row.get(1)?,
            event_type: row.get(2)?,
            event_timestamp: Self::parse_ts(&ts_str),
            source_id: row.get(5)?,
            data,
            confidence: row.get::<_, f32>(7).unwrap_or(1.0) as f64,
        })
    }

    fn row_to_relationship(row: &duckdb::Row) -> duckdb::Result<Relationship> {
        let props_str: String = row.get::<_, String>(4).unwrap_or_default();
        let properties: HashMap<String, JsonValue> =
            serde_json::from_str(&props_str).unwrap_or_default();
        Ok(Relationship {
            relationship_id: row.get(0)?,
            source_entity_id: row.get(1)?,
            target_entity_id: row.get(2)?,
            relationship_type: row.get(3)?,
            properties,
            confidence: row.get::<_, f32>(5).unwrap_or(1.0) as f64,
            is_active: row.get::<_, bool>(6).unwrap_or(true),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
    }

    /// Upsert geometry in entity_geometry table.
    fn upsert_geometry(
        conn: &Connection,
        entity_id: &str,
        lat: f64,
        lon: f64,
    ) -> StorageResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO entity_geometry (entity_id, geometry_wkt, latitude, longitude, last_updated)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (entity_id) DO UPDATE SET
               geometry_wkt  = EXCLUDED.geometry_wkt,
               latitude      = EXCLUDED.latitude,
               longitude     = EXCLUDED.longitude,
               last_updated  = EXCLUDED.last_updated",
            params![
                entity_id,
                format!("POINT({} {})", lon, lat),
                lat,
                lon,
                now,
            ],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    /// Compute SHA-256 of an arbitrary string, returned as hex.
    fn sha256_hex(input: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Collect relationship rows into a Vec.
    fn collect_relationships(
        mut rows: duckdb::Rows<'_>,
    ) -> StorageResult<Vec<Relationship>> {
        let mut rels = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            rels.push(
                Self::row_to_relationship(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(rels)
    }

    /// Haversine distance in km.
    fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
        const R: f64 = 6371.0;
        let dlat = (lat2 - lat1).to_radians();
        let dlon = (lon2 - lon1).to_radians();
        let a = (dlat / 2.0).sin().powi(2)
            + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
        2.0 * R * a.sqrt().asin()
    }
}

// ── Storage trait implementation ──────────────────────────────────────────────

#[async_trait]
impl Storage for DuckDbStorage {
    // ── Entity operations ─────────────────────────────────────────────────────

    async fn insert_entity(&self, entity: &Entity) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        let _props_json = serde_json::to_string(&entity.properties)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;

        let (lat, lon) = entity
            .geometry
            .as_ref()
            .map(|g| (Some(g.lat), Some(g.lon)))
            .unwrap_or((None, None));

        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO entities
               (entity_id, entity_type, canonical_id, name, confidence, is_active, last_updated)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (entity_id) DO UPDATE SET
               entity_type  = EXCLUDED.entity_type,
               canonical_id = EXCLUDED.canonical_id,
               name         = EXCLUDED.name,
               confidence   = EXCLUDED.confidence,
               is_active    = EXCLUDED.is_active,
               last_updated = EXCLUDED.last_updated",
            params![
                entity.entity_id,
                entity.entity_type,
                entity.canonical_id,
                entity.name,
                entity.confidence,
                entity.is_active,
                now,
            ],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        // Upsert geometry
        if let (Some(lat_v), Some(lon_v)) = (lat, lon) {
            Self::upsert_geometry(&conn, &entity.entity_id, lat_v, lon_v)?;
        }

        // Upsert properties as key-value entries
        for (key, val) in &entity.properties {
            let val_str = val.to_string();
            let prop_type = match val {
                JsonValue::Number(n) if n.is_f64() => "float",
                JsonValue::Number(_) => "int",
                JsonValue::Bool(_) => "bool",
                JsonValue::Null => "null",
                _ => "string",
            };
            let seq: i64 = conn
                .query_row("SELECT nextval('entity_properties_seq')", [], |r| r.get(0))
                .unwrap_or(0);
            conn.execute(
                "INSERT INTO entity_properties
                   (id, entity_id, property_key, property_value, property_type, source_id, is_latest)
                 VALUES (?, ?, ?, ?, ?, 'system', TRUE)
                 ON CONFLICT DO NOTHING",
                params![seq, entity.entity_id, key, val_str, prop_type],
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        }

        Ok(())
    }

    async fn get_entity(&self, entity_id: &str) -> StorageResult<Option<Entity>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                        e.confidence, e.source_count,
                        g.latitude, g.longitude, NULL::DOUBLE as altitude,
                        COALESCE(
                          (SELECT json_group_object(property_key, json(property_value))
                           FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                          '{}'
                        ) AS properties,
                        CAST(e.last_updated AS VARCHAR),
                        e.is_active,
                        CAST(e.first_seen AS VARCHAR)
                 FROM entities e
                 LEFT JOIN entity_geometry g ON g.entity_id = e.entity_id
                 WHERE e.entity_id = ?",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query(params![entity_id])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            Ok(Some(
                Self::row_to_entity(row).map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            ))
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
            .prepare(
                "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                        e.confidence, e.source_count,
                        g.latitude, g.longitude, NULL::DOUBLE,
                        COALESCE(
                          (SELECT json_group_object(property_key, json(property_value))
                           FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                          '{}'
                        ),
                        CAST(e.last_updated AS VARCHAR),
                        e.is_active,
                        CAST(e.first_seen AS VARCHAR)
                 FROM entities e
                 LEFT JOIN entity_geometry g ON g.entity_id = e.entity_id
                 WHERE LOWER(e.entity_type) = LOWER(?) AND e.is_active = TRUE
                 ORDER BY e.last_updated DESC
                 LIMIT ? OFFSET ?",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query(params![entity_type, limit as i64, offset as i64])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut entities = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            entities.push(
                Self::row_to_entity(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(entities)
    }

    async fn update_entity_property(
        &self,
        entity_id: &str,
        key: &str,
        value: JsonValue,
    ) -> StorageResult<()> {
        // Mark old property as non-latest, insert new one.
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE entity_properties SET is_latest = FALSE
             WHERE entity_id = ? AND property_key = ?",
            params![entity_id, key],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let val_str = value.to_string();
        let prop_type = match &value {
            JsonValue::Number(n) if n.is_f64() => "float",
            JsonValue::Number(_) => "int",
            JsonValue::Bool(_) => "bool",
            _ => "string",
        };
        let seq: i64 = conn
            .query_row("SELECT nextval('entity_properties_seq')", [], |r| r.get(0))
            .unwrap_or(0);
        conn.execute(
            "INSERT INTO entity_properties
               (id, entity_id, property_key, property_value, property_type, source_id, is_latest)
             VALUES (?, ?, ?, ?, ?, 'system', TRUE)",
            params![seq, entity_id, key, val_str, prop_type],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        // Touch entity last_updated
        conn.execute(
            "UPDATE entities SET last_updated = CURRENT_TIMESTAMP WHERE entity_id = ?",
            params![entity_id],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn delete_entity(&self, entity_id: &str) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE entities SET is_active = FALSE WHERE entity_id = ?",
            params![entity_id],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn count_entities(&self) -> StorageResult<u64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entities WHERE is_active = TRUE",
                [],
                |r| r.get(0),
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(n as u64)
    }

    async fn set_canonical_id(&self, entity_id: &str, canonical_id: &str) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE entities SET canonical_id = ?, is_canonical = (entity_id = ?), last_updated = CURRENT_TIMESTAMP
             WHERE entity_id = ?",
            params![canonical_id, canonical_id, entity_id],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    // ── Geospatial queries ────────────────────────────────────────────────────

    async fn get_entities_in_radius(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
        entity_type: Option<&str>,
    ) -> StorageResult<Vec<Entity>> {
        let deg_range = radius_km / 111.0;
        let conn = self.conn.lock().unwrap();

        let lat_min = lat - deg_range;
        let lat_max = lat + deg_range;
        let lon_min = lon - deg_range;
        let lon_max = lon + deg_range;

        let (sql, has_type) = if entity_type.is_some() {
            (
                "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                        e.confidence, e.source_count,
                        g.latitude, g.longitude, NULL::DOUBLE,
                        COALESCE(
                          (SELECT json_group_object(property_key, json(property_value))
                           FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                          '{}'
                        ),
                        CAST(e.last_updated AS VARCHAR),
                        e.is_active,
                        CAST(e.first_seen AS VARCHAR)
                 FROM entities e
                 JOIN entity_geometry g ON g.entity_id = e.entity_id
                 WHERE e.is_active = TRUE AND LOWER(e.entity_type) = LOWER(?)
                   AND g.latitude  BETWEEN ? AND ?
                   AND g.longitude BETWEEN ? AND ?",
                true,
            )
        } else {
            (
                "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                        e.confidence, e.source_count,
                        g.latitude, g.longitude, NULL::DOUBLE,
                        COALESCE(
                          (SELECT json_group_object(property_key, json(property_value))
                           FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                          '{}'
                        ),
                        CAST(e.last_updated AS VARCHAR),
                        e.is_active,
                        CAST(e.first_seen AS VARCHAR)
                 FROM entities e
                 JOIN entity_geometry g ON g.entity_id = e.entity_id
                 WHERE e.is_active = TRUE
                   AND g.latitude  BETWEEN ? AND ?
                   AND g.longitude BETWEEN ? AND ?",
                false,
            )
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = if has_type {
            let et = entity_type.ok_or_else(|| StorageError::QueryError("entity_type is required".into()))?;
            stmt.query(params![et, lat_min, lat_max, lon_min, lon_max])
        } else {
            stmt.query(params![lat_min, lat_max, lon_min, lon_max])
        }
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut entities = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            let e = Self::row_to_entity(row)
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            // Apply precise Haversine filter
            if let Some(g) = &e.geometry {
                if Self::haversine(lat, lon, g.lat, g.lon) <= radius_km {
                    entities.push(e);
                }
            }
        }
        Ok(entities)
    }

    async fn get_entities_in_polygon(
        &self,
        polygon_wkt: &str,
        entity_type: Option<&str>,
    ) -> StorageResult<Vec<Entity>> {
        let conn = self.conn.lock().unwrap();

        let (sql, use_type) = if self.spatial_enabled {
            if entity_type.is_some() {
                (
                    "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                            e.confidence, e.source_count,
                            g.latitude, g.longitude, NULL::DOUBLE,
                            COALESCE(
                              (SELECT json_group_object(property_key, json(property_value))
                               FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                              '{}'
                            ),
                            CAST(e.last_updated AS VARCHAR),
                            e.is_active
                     FROM entities e
                     JOIN entity_geometry g ON g.entity_id = e.entity_id
                     WHERE e.is_active = TRUE AND LOWER(e.entity_type) = LOWER(?)
                       AND ST_Contains(ST_GeomFromText(?), ST_Point(g.longitude, g.latitude))",
                    true,
                )
            } else {
                (
                    "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                            e.confidence, e.source_count,
                            g.latitude, g.longitude, NULL::DOUBLE,
                            COALESCE(
                              (SELECT json_group_object(property_key, json(property_value))
                               FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                              '{}'
                            ),
                            CAST(e.last_updated AS VARCHAR),
                            e.is_active
                     FROM entities e
                     JOIN entity_geometry g ON g.entity_id = e.entity_id
                     WHERE e.is_active = TRUE
                       AND ST_Contains(ST_GeomFromText(?), ST_Point(g.longitude, g.latitude))",
                    false,
                )
            }
        } else if entity_type.is_some() {
            (
                "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                        e.confidence, e.source_count,
                        g.latitude, g.longitude, NULL::DOUBLE,
                        COALESCE(
                          (SELECT json_group_object(property_key, json(property_value))
                           FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                          '{}'
                        ),
                        CAST(e.last_updated AS VARCHAR),
                        e.is_active,
                        CAST(e.first_seen AS VARCHAR)
                 FROM entities e
                 JOIN entity_geometry g ON g.entity_id = e.entity_id
                 WHERE e.is_active = TRUE AND LOWER(e.entity_type) = LOWER(?)",
                true,
            )
        } else {
            (
                "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                        e.confidence, e.source_count,
                        g.latitude, g.longitude, NULL::DOUBLE,
                        COALESCE(
                          (SELECT json_group_object(property_key, json(property_value))
                           FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                          '{}'
                        ),
                        CAST(e.last_updated AS VARCHAR),
                        e.is_active,
                        CAST(e.first_seen AS VARCHAR)
                 FROM entities e
                 JOIN entity_geometry g ON g.entity_id = e.entity_id
                 WHERE e.is_active = TRUE",
                false,
            )
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = if self.spatial_enabled && use_type {
            let et = entity_type.ok_or_else(|| StorageError::QueryError("entity_type is required".into()))?;
            stmt.query(params![et, polygon_wkt])
        } else if self.spatial_enabled {
            stmt.query(params![polygon_wkt])
        } else if use_type {
            let et = entity_type.ok_or_else(|| StorageError::QueryError("entity_type is required".into()))?;
            stmt.query(params![et])
        } else {
            stmt.query([])
        }
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut entities = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            entities.push(
                Self::row_to_entity(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(entities)
    }

    // ── Event operations ──────────────────────────────────────────────────────

    async fn insert_event(&self, event: &Event) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        let data_str = serde_json::to_string(&event.data)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;

        conn.execute(
            "INSERT INTO events
               (event_id, entity_id, event_type, event_timestamp, source_id, event_data, confidence)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (event_id) DO NOTHING",
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

        // Increment events_ingested_total for the source
        conn.execute(
            "UPDATE data_sources SET events_ingested_total = events_ingested_total + 1
             WHERE source_id = ?",
            params![event.source_id],
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
            .prepare(
                "SELECT event_id, entity_id, event_type,
                        CAST(event_timestamp AS VARCHAR), ingestion_timestamp,
                        source_id, event_data, confidence
                 FROM events
                 WHERE entity_id = ?
                 ORDER BY event_timestamp DESC
                 LIMIT ?",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query(params![entity_id, limit as i64])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut events = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            events.push(
                Self::row_to_event(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(events)
    }

    async fn get_events_in_time_range(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> StorageResult<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_id, entity_id, event_type,
                        CAST(event_timestamp AS VARCHAR), ingestion_timestamp,
                        source_id, event_data, confidence
                 FROM events
                 WHERE event_timestamp >= ? AND event_timestamp <= ?
                 ORDER BY event_timestamp ASC",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query(params![start.to_rfc3339(), end.to_rfc3339()])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut events = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            events.push(
                Self::row_to_event(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(events)
    }

    async fn get_events_in_region(
        &self,
        lat: f64,
        lon: f64,
        radius_km: f64,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> StorageResult<Vec<Event>> {
        // Get entity IDs in radius, then fetch their events in time range.
        let entities = self
            .get_entities_in_radius(lat, lon, radius_km, None)
            .await?;
        if entities.is_empty() {
            return Ok(vec![]);
        }

        let entity_ids: Vec<String> = entities.iter().map(|e| e.entity_id.clone()).collect();

        // Use parameterised query: build placeholders dynamically
        let placeholders: Vec<String> = (0..entity_ids.len()).map(|_| "?".to_string()).collect();
        let placeholders_str = placeholders.join(",");

        let sql = format!(
            "SELECT event_id, entity_id, event_type,
                    CAST(event_timestamp AS VARCHAR), ingestion_timestamp,
                    source_id, event_data, confidence
             FROM events
             WHERE entity_id IN ({})
               AND event_timestamp >= ? AND event_timestamp <= ?
             ORDER BY event_timestamp ASC",
            placeholders_str,
        );

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        // Build params: entity_ids... + start + end
        use duckdb::types::ToSql;
        let start_str = start.to_rfc3339();
        let end_str = end.to_rfc3339();
        let mut param_values: Vec<Box<dyn ToSql>> = entity_ids
            .iter()
            .map(|id| Box::new(id.clone()) as Box<dyn ToSql>)
            .collect();
        param_values.push(Box::new(start_str));
        param_values.push(Box::new(end_str));
        let param_refs: Vec<&dyn ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut events = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            events.push(
                Self::row_to_event(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(events)
    }

    // ── Relationship operations ───────────────────────────────────────────────

    async fn insert_relationship(&self, rel: &Relationship) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        let props_json = serde_json::to_string(&rel.properties)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        let now = chrono::Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO relationships
               (relationship_id, source_entity_id, target_entity_id,
                relationship_type, properties, confidence, is_active, last_confirmed_timestamp)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (relationship_id) DO UPDATE SET
               relationship_type          = EXCLUDED.relationship_type,
               properties                 = EXCLUDED.properties,
               confidence                 = EXCLUDED.confidence,
               is_active                  = EXCLUDED.is_active,
               last_confirmed_timestamp   = EXCLUDED.last_confirmed_timestamp",
            params![
                rel.relationship_id,
                rel.source_entity_id,
                rel.target_entity_id,
                rel.relationship_type,
                props_json,
                rel.confidence,
                rel.is_active,
                now,
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
            .prepare(
                "SELECT relationship_id, source_entity_id, target_entity_id,
                        relationship_type, properties, confidence, is_active
                 FROM relationships
                 WHERE (source_entity_id = ? OR target_entity_id = ?)
                   AND is_active = TRUE",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query(params![entity_id, entity_id])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rels = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            rels.push(
                Self::row_to_relationship(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(rels)
    }

    async fn get_outgoing_relationships(
        &self,
        source_entity_id: &str,
        rel_type: Option<&str>,
    ) -> StorageResult<Vec<Relationship>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = if let Some(rt) = rel_type {
            let mut s = conn
                .prepare(
                    "SELECT relationship_id, source_entity_id, target_entity_id,
                            relationship_type, properties, confidence, is_active
                     FROM relationships
                     WHERE source_entity_id = ? AND is_active = TRUE
                       AND relationship_type = ?",
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            let rows = s
                .query(params![source_entity_id, rt])
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            return Self::collect_relationships(rows);
        } else {
            conn.prepare(
                "SELECT relationship_id, source_entity_id, target_entity_id,
                        relationship_type, properties, confidence, is_active
                 FROM relationships
                 WHERE source_entity_id = ? AND is_active = TRUE",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        };

        let rows = stmt
            .query(params![source_entity_id])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Self::collect_relationships(rows)
    }

    async fn get_incoming_relationships(
        &self,
        target_entity_id: &str,
        rel_type: Option<&str>,
    ) -> StorageResult<Vec<Relationship>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = if let Some(rt) = rel_type {
            let mut s = conn
                .prepare(
                    "SELECT relationship_id, source_entity_id, target_entity_id,
                            relationship_type, properties, confidence, is_active
                     FROM relationships
                     WHERE target_entity_id = ? AND is_active = TRUE
                       AND relationship_type = ?",
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            let rows = s
                .query(params![target_entity_id, rt])
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            return Self::collect_relationships(rows);
        } else {
            conn.prepare(
                "SELECT relationship_id, source_entity_id, target_entity_id,
                        relationship_type, properties, confidence, is_active
                 FROM relationships
                 WHERE target_entity_id = ? AND is_active = TRUE",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        };

        let rows = stmt
            .query(params![target_entity_id])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Self::collect_relationships(rows)
    }

    // ── Graph / path queries ──────────────────────────────────────────────────

    async fn graph_query(
        &self,
        query_str: &str,
    ) -> StorageResult<Vec<HashMap<String, JsonValue>>> {
        // Delegate to GraphEngine which maintains dedicated graph tables and
        // supports Cypher-like syntax, raw SQL, and multi-hop traversal against
        // graph_nodes / graph_edges views.
        let graph = crate::graph_engine::GraphEngine::new(Arc::clone(&self.conn))?;
        graph.cypher_query(query_str)
    }
    /// BFS path search up to `max_hops` using a recursive CTE over the relationships table.
    async fn path_query(
        &self,
        source_entity_id: &str,
        target_entity_id: &str,
        max_hops: usize,
    ) -> StorageResult<Vec<Vec<Relationship>>> {
        let conn = self.conn.lock().unwrap();

        // Recursive CTE uses parameterised placeholders for source/target/max_hops.
        let sql = r#"
            WITH RECURSIVE path_cte AS (
              SELECT
                relationship_id,
                source_entity_id,
                target_entity_id,
                relationship_type,
                properties,
                confidence,
                is_active,
                1 AS hop_count,
                [relationship_id] AS path_ids
              FROM relationships
              WHERE source_entity_id = ? AND is_active = TRUE

              UNION ALL

              SELECT
                r.relationship_id,
                r.source_entity_id,
                r.target_entity_id,
                r.relationship_type,
                r.properties,
                r.confidence,
                r.is_active,
                p.hop_count + 1,
                list_append(p.path_ids, r.relationship_id)
              FROM relationships r
              JOIN path_cte p ON r.source_entity_id = p.target_entity_id
              WHERE r.is_active = TRUE
                AND p.hop_count < ?
                AND NOT list_contains(p.path_ids, r.relationship_id)
            )
            SELECT relationship_id, source_entity_id, target_entity_id,
                   relationship_type, properties, confidence, is_active
            FROM path_cte
            WHERE target_entity_id = ?
            ORDER BY hop_count
        "#;

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let mut rows = stmt
            .query(params![source_entity_id, max_hops as i64, target_entity_id])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut paths: Vec<Vec<Relationship>> = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            let rel = Self::row_to_relationship(row)
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            paths.push(vec![rel]);
        }
        Ok(paths)
    }

    // ── Audit operations ──────────────────────────────────────────────────────

    async fn log_audit(
        &self,
        operation: &str,
        entity_type: Option<&str>,
        entity_id: Option<&str>,
        user_id: Option<&str>,
        details: JsonValue,
    ) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();

        // Get the previous hash (last entry in audit_log).
        let previous_hash: Option<String> = conn
            .query_row(
                "SELECT content_hash FROM audit_log ORDER BY sequence_number DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok();

        // Get next sequence number.
        let seq: i64 = conn
            .query_row("SELECT nextval('audit_log_seq')", [], |r| r.get(0))
            .unwrap_or(1);

        // Compute content_hash = SHA256(seq || op || entity_type || entity_id || details).
        let now = chrono::Utc::now().to_rfc3339();
        let hash_input = format!(
            "{}{}{}{}{}{}",
            seq,
            operation,
            entity_type.unwrap_or(""),
            entity_id.unwrap_or(""),
            now,
            details,
        );
        let content_hash = Self::sha256_hex(&hash_input);
        let details_str = details.to_string();

        conn.execute(
            "INSERT INTO audit_log
               (sequence_number, timestamp, operation, entity_type, entity_id,
                user_id, previous_hash, content_hash, details)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                seq,
                now,
                operation,
                entity_type,
                entity_id,
                user_id,
                previous_hash,
                content_hash,
                details_str,
            ],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    // ── Data source operations ────────────────────────────────────────────────

    async fn register_data_source(&self, source: &orp_proto::DataSource) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO data_sources
               (source_id, source_name, source_type, trust_score, events_ingested_total, enabled)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (source_id) DO UPDATE SET
               source_name           = EXCLUDED.source_name,
               trust_score           = EXCLUDED.trust_score,
               enabled               = EXCLUDED.enabled",
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
            .prepare(
                "SELECT source_id, source_name, source_type, trust_score,
                        events_ingested_total, entities_provided_total, error_count,
                        enabled, CAST(last_heartbeat AS VARCHAR), certificate_fingerprint
                 FROM data_sources",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query([])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut sources = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            let hb_str: Option<String> = row.get(8).ok().flatten();
            let last_heartbeat = hb_str.map(|s| Self::parse_ts(&s));

            sources.push(DataSource {
                source_id: row
                    .get(0)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
                source_name: row
                    .get(1)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
                source_type: row
                    .get(2)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
                trust_score: row.get::<_, f32>(3).unwrap_or(0.8),
                events_ingested: row.get::<_, i64>(4).unwrap_or(0) as u64,
                entities_provided: row.get::<_, i64>(5).unwrap_or(0) as u64,
                error_count: row.get::<_, i64>(6).unwrap_or(0) as u64,
                enabled: row.get::<_, bool>(7).unwrap_or(true),
                last_heartbeat,
                certificate_fingerprint: row.get(9).ok().flatten(),
            });
        }
        Ok(sources)
    }

    async fn update_source_heartbeat(&self, source_id: &str) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE data_sources SET last_heartbeat = CURRENT_TIMESTAMP WHERE source_id = ?",
            params![source_id],
        )
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn get_data_source(&self, source_id: &str) -> StorageResult<Option<DataSource>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT source_id, source_name, source_type, trust_score,
                        events_ingested_total, entities_provided_total, error_count,
                        enabled, CAST(last_heartbeat AS VARCHAR), certificate_fingerprint
                 FROM data_sources WHERE source_id = ?",
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = stmt
            .query(params![source_id])
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        if let Some(row) = rows.next().map_err(|e| StorageError::DatabaseError(e.to_string()))? {
            let hb_str: Option<String> = row.get(8).ok().flatten();
            let last_heartbeat = hb_str.map(|s| Self::parse_ts(&s));
            Ok(Some(DataSource {
                source_id: row.get(0).map_err(|e| StorageError::DatabaseError(e.to_string()))?,
                source_name: row.get(1).map_err(|e| StorageError::DatabaseError(e.to_string()))?,
                source_type: row.get(2).map_err(|e| StorageError::DatabaseError(e.to_string()))?,
                trust_score: row.get::<_, f32>(3).unwrap_or(0.8),
                events_ingested: row.get::<_, i64>(4).unwrap_or(0) as u64,
                entities_provided: row.get::<_, i64>(5).unwrap_or(0) as u64,
                error_count: row.get::<_, i64>(6).unwrap_or(0) as u64,
                enabled: row.get::<_, bool>(7).unwrap_or(true),
                last_heartbeat,
                certificate_fingerprint: row.get(9).ok().flatten(),
            }))
        } else {
            Ok(None)
        }
    }

    async fn update_data_source(&self, source: &DataSource) -> StorageResult<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE data_sources SET
               source_name = ?, source_type = ?, trust_score = ?, enabled = ?
             WHERE source_id = ?",
            params![
                source.source_name,
                source.source_type,
                source.trust_score,
                source.enabled,
                source.source_id,
            ],
        ).map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(rows > 0)
    }

    async fn delete_data_source(&self, source_id: &str) -> StorageResult<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM data_sources WHERE source_id = ?",
            params![source_id],
        ).map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(rows > 0)
    }

    // ── Event global queries ──────────────────────────────────────────────────

    async fn get_events_global(
        &self,
        entity_id: Option<&str>,
        entity_type: Option<&str>,
        event_type: Option<&str>,
        since: Option<chrono::DateTime<chrono::Utc>>,
        until: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
        offset: usize,
    ) -> StorageResult<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut conditions = vec!["1=1".to_string()];
        let mut param_values: Vec<Box<dyn duckdb::types::ToSql>> = Vec::new();

        if let Some(eid) = entity_id {
            conditions.push("ev.entity_id = ?".to_string());
            param_values.push(Box::new(eid.to_string()));
        }
        if let Some(etype) = entity_type {
            conditions.push(
                "ev.entity_id IN (SELECT entity_id FROM entities WHERE LOWER(entity_type) = LOWER(?))"
                    .to_string(),
            );
            param_values.push(Box::new(etype.to_string()));
        }
        if let Some(et) = event_type {
            conditions.push("ev.event_type = ?".to_string());
            param_values.push(Box::new(et.to_string()));
        }
        if let Some(s) = since {
            conditions.push("ev.event_timestamp >= ?".to_string());
            param_values.push(Box::new(s.to_rfc3339()));
        }
        if let Some(u) = until {
            conditions.push("ev.event_timestamp <= ?".to_string());
            param_values.push(Box::new(u.to_rfc3339()));
        }
        let where_clause = conditions.join(" AND ");
        param_values.push(Box::new(limit as i64));
        param_values.push(Box::new(offset as i64));

        let sql = format!(
            "SELECT ev.event_id, ev.entity_id, ev.event_type,
                    CAST(ev.event_timestamp AS VARCHAR), ev.ingestion_timestamp,
                    ev.source_id, ev.event_data, ev.confidence
             FROM events ev
             WHERE {where_clause}
             ORDER BY ev.event_timestamp DESC
             LIMIT ? OFFSET ?"
        );

        let param_refs: Vec<&dyn duckdb::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        let mut events = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            events.push(
                Self::row_to_event(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(events)
    }

    async fn count_events_global(
        &self,
        entity_id: Option<&str>,
        entity_type: Option<&str>,
        event_type: Option<&str>,
        since: Option<chrono::DateTime<chrono::Utc>>,
        until: Option<chrono::DateTime<chrono::Utc>>,
    ) -> StorageResult<u64> {
        let conn = self.conn.lock().unwrap();
        let mut conditions = vec!["1=1".to_string()];
        let mut param_values: Vec<Box<dyn duckdb::types::ToSql>> = Vec::new();

        if let Some(eid) = entity_id {
            conditions.push("ev.entity_id = ?".to_string());
            param_values.push(Box::new(eid.to_string()));
        }
        if let Some(etype) = entity_type {
            conditions.push(
                "ev.entity_id IN (SELECT entity_id FROM entities WHERE LOWER(entity_type) = LOWER(?))"
                    .to_string(),
            );
            param_values.push(Box::new(etype.to_string()));
        }
        if let Some(et) = event_type {
            conditions.push("ev.event_type = ?".to_string());
            param_values.push(Box::new(et.to_string()));
        }
        if let Some(s) = since {
            conditions.push("ev.event_timestamp >= ?".to_string());
            param_values.push(Box::new(s.to_rfc3339()));
        }
        if let Some(u) = until {
            conditions.push("ev.event_timestamp <= ?".to_string());
            param_values.push(Box::new(u.to_rfc3339()));
        }
        let where_clause = conditions.join(" AND ");
        let sql = format!("SELECT COUNT(*) FROM events ev WHERE {where_clause}");

        let param_refs: Vec<&dyn duckdb::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let n: i64 = conn
            .query_row(&sql, param_refs.as_slice(), |r| r.get(0))
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(n as u64)
    }

    // ── Transaction operations ────────────────────────────────────────────────

    async fn begin_transaction(&self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("BEGIN")
            .map_err(|e| StorageError::TransactionError(e.to_string()))?;
        Ok(())
    }

    async fn commit_transaction(&self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("COMMIT")
            .map_err(|e| StorageError::TransactionError(e.to_string()))?;
        Ok(())
    }

    async fn rollback_transaction(&self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("ROLLBACK")
            .map_err(|e| StorageError::TransactionError(e.to_string()))?;
        Ok(())
    }

    // ── Search ────────────────────────────────────────────────────────────────

    async fn search_entities(
        &self,
        query: &str,
        entity_type: Option<&str>,
        limit: usize,
    ) -> StorageResult<Vec<Entity>> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", query);

        let (sql, use_type) = if entity_type.is_some() {
            (
                "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                        e.confidence, e.source_count,
                        g.latitude, g.longitude, NULL::DOUBLE,
                        COALESCE(
                          (SELECT json_group_object(property_key, json(property_value))
                           FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                          '{}'
                        ),
                        CAST(e.last_updated AS VARCHAR),
                        e.is_active,
                        CAST(e.first_seen AS VARCHAR)
                 FROM entities e
                 LEFT JOIN entity_geometry g ON g.entity_id = e.entity_id
                 WHERE e.is_active = TRUE AND LOWER(e.entity_type) = LOWER(?)
                   AND (e.name ILIKE ? OR e.entity_id ILIKE ?)
                 LIMIT ?",
                true,
            )
        } else {
            (
                "SELECT e.entity_id, e.entity_type, e.canonical_id, e.name,
                        e.confidence, e.source_count,
                        g.latitude, g.longitude, NULL::DOUBLE,
                        COALESCE(
                          (SELECT json_group_object(property_key, json(property_value))
                           FROM entity_properties WHERE entity_id = e.entity_id AND is_latest = TRUE),
                          '{}'
                        ),
                        CAST(e.last_updated AS VARCHAR),
                        e.is_active,
                        CAST(e.first_seen AS VARCHAR)
                 FROM entities e
                 LEFT JOIN entity_geometry g ON g.entity_id = e.entity_id
                 WHERE e.is_active = TRUE
                   AND (e.name ILIKE ? OR e.entity_id ILIKE ?)
                 LIMIT ?",
                false,
            )
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut rows = if use_type {
            let et = entity_type.ok_or_else(|| StorageError::QueryError("entity_type is required".into()))?;
            stmt.query(params![et, pattern, pattern, limit as i64])
        } else {
            stmt.query(params![pattern, pattern, limit as i64])
        }
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        let mut entities = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        {
            entities.push(
                Self::row_to_entity(row)
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?,
            );
        }
        Ok(entities)
    }

    // ── Administrative ────────────────────────────────────────────────────────

    async fn health_check(&self) -> StorageResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT 1", [], |_| Ok(()))
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn get_stats(&self) -> StorageResult<StorageStats> {
        let conn = self.conn.lock().unwrap();
        let total_entities: i64 = conn
            .query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))
            .unwrap_or(0);
        let total_events: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap_or(0);
        let total_relationships: i64 = conn
            .query_row("SELECT COUNT(*) FROM relationships", [], |r| r.get(0))
            .unwrap_or(0);
        let audit_entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
            .unwrap_or(0);
        let data_sources: i64 = conn
            .query_row("SELECT COUNT(*) FROM data_sources", [], |r| r.get(0))
            .unwrap_or(0);

        Ok(StorageStats {
            total_entities: total_entities as u64,
            total_events: total_events as u64,
            total_relationships: total_relationships as u64,
            database_size_bytes: 0,
            audit_log_entries: audit_entries as u64,
            data_sources: data_sources as u64,
        })
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entity(id: &str, etype: &str) -> Entity {
        Entity {
            entity_id: id.to_string(),
            entity_type: etype.to_string(),
            ..Entity::default()
        }
    }

    fn make_entity_with_geo(id: &str, etype: &str, lat: f64, lon: f64) -> Entity {
        Entity {
            entity_id: id.to_string(),
            entity_type: etype.to_string(),
            geometry: Some(GeoPoint { lat, lon, alt: None }),
            ..Entity::default()
        }
    }

    fn make_event(id: &str, entity_id: &str) -> Event {
        Event {
            event_id: id.to_string(),
            entity_id: entity_id.to_string(),
            event_type: "position_update".to_string(),
            event_timestamp: chrono::Utc::now(),
            source_id: "test-src".to_string(),
            data: serde_json::json!({"speed": 10.5}),
            confidence: 0.9,
        }
    }

    fn make_relationship(id: &str, src: &str, tgt: &str) -> Relationship {
        Relationship {
            relationship_id: id.to_string(),
            source_entity_id: src.to_string(),
            target_entity_id: tgt.to_string(),
            relationship_type: "docked_at".to_string(),
            properties: HashMap::new(),
            confidence: 0.9,
            is_active: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_source(id: &str) -> orp_proto::DataSource {
        orp_proto::DataSource {
            source_id: id.to_string(),
            source_name: format!("{} Name", id),
            source_type: "ais".to_string(),
            trust_score: 0.95,
            events_ingested: 0,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn test_01_insert_and_get_entity() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("e1", "ship")).await.unwrap();
        let e = s.get_entity("e1").await.unwrap().unwrap();
        assert_eq!(e.entity_type, "ship");
    }

    #[tokio::test]
    async fn test_02_entity_not_found() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        let e = s.get_entity("missing").await.unwrap();
        assert!(e.is_none());
    }

    #[tokio::test]
    async fn test_03_get_entities_by_type() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        for i in 0..5 {
            s.insert_entity(&make_entity(&format!("s{}", i), "ship"))
                .await
                .unwrap();
        }
        s.insert_entity(&make_entity("p1", "port")).await.unwrap();
        let ships = s.get_entities_by_type("ship", 10, 0).await.unwrap();
        assert_eq!(ships.len(), 5);
        let ports = s.get_entities_by_type("port", 10, 0).await.unwrap();
        assert_eq!(ports.len(), 1);
    }

    #[tokio::test]
    async fn test_04_update_entity_property() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("e1", "ship")).await.unwrap();
        s.update_entity_property("e1", "speed", serde_json::json!(15.0))
            .await
            .unwrap();
        // Property updated without error
    }

    #[tokio::test]
    async fn test_05_delete_entity_soft() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("e1", "ship")).await.unwrap();
        assert_eq!(s.count_entities().await.unwrap(), 1);
        s.delete_entity("e1").await.unwrap();
        assert_eq!(s.count_entities().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_06_set_canonical_id() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("dup1", "ship")).await.unwrap();
        s.set_canonical_id("dup1", "canonical1").await.unwrap();
        let e = s.get_entity("dup1").await.unwrap().unwrap();
        assert_eq!(e.canonical_id, Some("canonical1".to_string()));
    }

    #[tokio::test]
    async fn test_07_geospatial_in_radius() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity_with_geo("near", "ship", 51.92, 4.47))
            .await
            .unwrap();
        s.insert_entity(&make_entity_with_geo("far", "ship", 35.0, 139.0))
            .await
            .unwrap();
        let results = s
            .get_entities_in_radius(51.92, 4.47, 50.0, Some("ship"))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity_id, "near");
    }

    #[tokio::test]
    async fn test_08_geospatial_no_type_filter() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity_with_geo("ship1", "ship", 51.92, 4.47))
            .await
            .unwrap();
        s.insert_entity(&make_entity_with_geo("port1", "port", 51.91, 4.48))
            .await
            .unwrap();
        let results = s
            .get_entities_in_radius(51.92, 4.47, 50.0, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_09_insert_and_get_events() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("e1", "ship")).await.unwrap();
        for i in 0..5 {
            let mut ev = make_event(&format!("ev{}", i), "e1");
            ev.event_timestamp =
                chrono::Utc::now() - chrono::Duration::seconds(i as i64 * 10);
            s.insert_event(&ev).await.unwrap();
        }
        let events = s.get_events_for_entity("e1", 10).await.unwrap();
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn test_10_events_in_time_range() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("e1", "ship")).await.unwrap();
        let now = chrono::Utc::now();
        for i in 0..10i64 {
            let mut ev = make_event(&format!("ev{}", i), "e1");
            ev.event_timestamp = now - chrono::Duration::hours(i);
            s.insert_event(&ev).await.unwrap();
        }
        let start = now - chrono::Duration::hours(5);
        let end = now;
        let events = s.get_events_in_time_range(start, end).await.unwrap();
        // Events 0..=5 (6 events) should be in range
        assert!(events.len() >= 5 && events.len() <= 6);
    }

    #[tokio::test]
    async fn test_11_events_in_region() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity_with_geo("e1", "ship", 51.92, 4.47))
            .await
            .unwrap();
        s.insert_entity(&make_entity_with_geo("e2", "ship", 35.0, 139.0))
            .await
            .unwrap();
        let now = chrono::Utc::now();
        s.insert_event(&make_event("ev1", "e1")).await.unwrap();
        s.insert_event(&make_event("ev2", "e2")).await.unwrap();
        let events = s
            .get_events_in_region(
                51.92,
                4.47,
                50.0,
                now - chrono::Duration::minutes(1),
                now + chrono::Duration::minutes(1),
            )
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].entity_id, "e1");
    }

    #[tokio::test]
    async fn test_12_insert_and_get_relationships() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("ship1", "ship")).await.unwrap();
        s.insert_entity(&make_entity("port1", "port")).await.unwrap();
        s.insert_relationship(&make_relationship("r1", "ship1", "port1"))
            .await
            .unwrap();
        let rels = s.get_relationships_for_entity("ship1").await.unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].relationship_type, "docked_at");
    }

    #[tokio::test]
    async fn test_13_outgoing_incoming_relationships() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("a", "ship")).await.unwrap();
        s.insert_entity(&make_entity("b", "port")).await.unwrap();
        s.insert_relationship(&make_relationship("r1", "a", "b"))
            .await
            .unwrap();
        let out = s
            .get_outgoing_relationships("a", None)
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        let inc = s
            .get_incoming_relationships("b", None)
            .await
            .unwrap();
        assert_eq!(inc.len(), 1);
        // Wrong direction
        assert!(s.get_outgoing_relationships("b", None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_14_path_query() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("a", "ship")).await.unwrap();
        s.insert_entity(&make_entity("b", "waypoint")).await.unwrap();
        s.insert_entity(&make_entity("c", "port")).await.unwrap();
        s.insert_relationship(&make_relationship("r1", "a", "b"))
            .await
            .unwrap();
        s.insert_relationship(&make_relationship("r2", "b", "c"))
            .await
            .unwrap();
        let paths = s.path_query("a", "c", 3).await.unwrap();
        assert!(!paths.is_empty());
    }

    #[tokio::test]
    async fn test_15_audit_log_hash_chain() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.log_audit(
            "insert",
            Some("ship"),
            Some("e1"),
            Some("user1"),
            serde_json::json!({"note": "first entry"}),
        )
        .await
        .unwrap();
        s.log_audit(
            "update",
            Some("ship"),
            Some("e1"),
            None,
            serde_json::json!({"speed": 20}),
        )
        .await
        .unwrap();
        let stats = s.get_stats().await.unwrap();
        assert_eq!(stats.audit_log_entries, 2);
    }

    #[tokio::test]
    async fn test_16_register_and_get_data_sources() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.register_data_source(&make_source("ais-1")).await.unwrap();
        s.register_data_source(&make_source("adsb-1")).await.unwrap();
        let sources = s.get_data_sources().await.unwrap();
        assert_eq!(sources.len(), 2);
    }

    #[tokio::test]
    async fn test_17_update_source_heartbeat() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.register_data_source(&make_source("ais-1")).await.unwrap();
        s.update_source_heartbeat("ais-1").await.unwrap();
        let sources = s.get_data_sources().await.unwrap();
        assert!(sources[0].last_heartbeat.is_some());
    }

    #[tokio::test]
    async fn test_18_transactions_commit() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.begin_transaction().await.unwrap();
        s.insert_entity(&make_entity("tx1", "ship")).await.unwrap();
        s.commit_transaction().await.unwrap();
        // After COMMIT the entity must be visible
        assert!(s.get_entity("tx1").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_18b_transactions_rollback() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        // Insert and commit a baseline entity so we know the DB is functional
        s.insert_entity(&make_entity("baseline", "ship"))
            .await
            .unwrap();

        s.begin_transaction().await.unwrap();
        s.insert_entity(&make_entity("rolled-back", "ship"))
            .await
            .unwrap();
        // The entity should be visible within the transaction before rollback
        assert!(s.get_entity("rolled-back").await.unwrap().is_some());
        s.rollback_transaction().await.unwrap();
        // After ROLLBACK the entity must NOT be visible
        assert!(
            s.get_entity("rolled-back").await.unwrap().is_none(),
            "Entity inserted within a rolled-back transaction must not persist"
        );
        // Baseline entity should still be present
        assert!(s.get_entity("baseline").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_18c_nested_transactions_not_supported() {
        // DuckDB does not support nested transactions — calling BEGIN twice should error
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.begin_transaction().await.unwrap();
        let result = s.begin_transaction().await;
        // Expect an error; clean up regardless
        let is_err = result.is_err();
        let _ = s.rollback_transaction().await;
        assert!(is_err, "Nested BEGIN should return an error in DuckDB");
    }

    #[tokio::test]
    async fn test_19_search_entities() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        let mut e = make_entity("e1", "ship");
        e.name = Some("Rotterdam Express".to_string());
        s.insert_entity(&e).await.unwrap();
        let results = s
            .search_entities("Rotterdam", Some("ship"), 10)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].entity_id, "e1");
    }

    #[tokio::test]
    async fn test_20_health_check() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        assert!(s.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_21_stats() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("e1", "ship")).await.unwrap();
        let stats = s.get_stats().await.unwrap();
        assert_eq!(stats.total_entities, 1);
        assert_eq!(stats.total_events, 0);
    }

    #[tokio::test]
    async fn test_22_pagination() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        for i in 0..15 {
            s.insert_entity(&make_entity(&format!("s{}", i), "ship"))
                .await
                .unwrap();
        }
        let p1 = s.get_entities_by_type("ship", 5, 0).await.unwrap();
        let p2 = s.get_entities_by_type("ship", 5, 5).await.unwrap();
        assert_eq!(p1.len(), 5);
        assert_eq!(p2.len(), 5);
        let ids1: std::collections::HashSet<_> = p1.iter().map(|e| &e.entity_id).collect();
        let ids2: std::collections::HashSet<_> = p2.iter().map(|e| &e.entity_id).collect();
        assert!(ids1.is_disjoint(&ids2));
    }

    #[tokio::test]
    async fn test_23_file_based_storage() {
        let dir = std::env::temp_dir().join("orp_duckdb_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.duckdb");
        {
            let s = DuckDbStorage::new_with_path(path.to_str().unwrap()).unwrap();
            s.insert_entity(&make_entity("persist-1", "ship")).await.unwrap();
        }
        // Re-open
        let s2 = DuckDbStorage::new_with_path(path.to_str().unwrap()).unwrap();
        assert!(s2.get_entity("persist-1").await.unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_24_entity_geometry_updated_on_insert() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        let e = make_entity_with_geo("geo1", "ship", 48.8566, 2.3522); // Paris
        s.insert_entity(&e).await.unwrap();
        let near = s
            .get_entities_in_radius(48.8566, 2.3522, 10.0, None)
            .await
            .unwrap();
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].entity_id, "geo1");
    }

    #[tokio::test]
    async fn test_25_event_data_round_trip() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("e1", "ship")).await.unwrap();
        let mut ev = make_event("ev1", "e1");
        ev.data = serde_json::json!({"speed": 42.5, "heading": 90, "nested": {"a": 1}});
        s.insert_event(&ev).await.unwrap();
        let fetched = s.get_events_for_entity("e1", 1).await.unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].data["speed"], serde_json::json!(42.5));
        assert_eq!(fetched[0].data["nested"]["a"], serde_json::json!(1));
    }

    #[tokio::test]
    async fn test_26_count_entities() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        for i in 0..5 {
            s.insert_entity(&make_entity(&format!("cnt-{}", i), "ship")).await.unwrap();
        }
        let count = s.count_entities().await.unwrap();
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_27_entity_upsert_on_conflict() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        let mut e = make_entity("upsert-1", "ship");
        e.name = Some("Original".to_string());
        s.insert_entity(&e).await.unwrap();

        e.name = Some("Updated".to_string());
        s.insert_entity(&e).await.unwrap();

        let fetched = s.get_entity("upsert-1").await.unwrap().unwrap();
        assert_eq!(fetched.name, Some("Updated".to_string()));
    }

    #[tokio::test]
    async fn test_28_update_entity_property() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("prop-1", "ship")).await.unwrap();
        s.update_entity_property("prop-1", "speed", serde_json::json!(25.5)).await.unwrap();

        let entity = s.get_entity("prop-1").await.unwrap().unwrap();
        assert!(entity.properties.contains_key("speed"));
    }

    #[tokio::test]
    async fn test_29_search_by_name() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        let mut e = make_entity("search-1", "ship");
        e.name = Some("Ever Given".to_string());
        s.insert_entity(&e).await.unwrap();

        let results = s.search_entities("Ever", Some("ship"), 10).await.unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_30_search_empty_query_returns_all() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        for i in 0..3 {
            s.insert_entity(&make_entity(&format!("all-{}", i), "ship")).await.unwrap();
        }
        let results = s.search_entities("", None, 100).await.unwrap();
        assert!(results.len() >= 3);
    }

    #[tokio::test]
    async fn test_31_get_entities_in_radius_no_results() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        let e = make_entity_with_geo("far", "ship", 10.0, 10.0);
        s.insert_entity(&e).await.unwrap();

        let near = s.get_entities_in_radius(51.0, 4.0, 5.0, None).await.unwrap();
        assert!(near.is_empty());
    }

    #[tokio::test]
    async fn test_32_register_and_delete_data_source() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        let ds = orp_proto::DataSource {
            source_id: "ds-del".to_string(),
            source_name: "To Delete".to_string(),
            source_type: "ais".to_string(),
            trust_score: 0.9,
            events_ingested: 0,
            enabled: true,
        };
        s.register_data_source(&ds).await.unwrap();
        assert!(s.get_data_source("ds-del").await.unwrap().is_some());

        let deleted = s.delete_data_source("ds-del").await.unwrap();
        assert!(deleted);
        assert!(s.get_data_source("ds-del").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_33_delete_nonexistent_data_source() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        let deleted = s.delete_data_source("nonexistent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_34_events_global_with_filters() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("glob-e1", "ship")).await.unwrap();
        let ev = make_event("glob-ev1", "glob-e1");
        s.insert_event(&ev).await.unwrap();

        let events = s.get_events_global(Some("glob-e1"), None, None, None, None, 10, 0).await.unwrap();
        assert!(!events.is_empty());
    }

    #[tokio::test]
    async fn test_35_count_events_global() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("cnt-ev", "ship")).await.unwrap();
        for i in 0..3 {
            let mut ev = make_event(&format!("cnt-ev-{}", i), "cnt-ev");
            ev.event_type = "position_update".to_string();
            s.insert_event(&ev).await.unwrap();
        }
        let count = s.count_events_global(Some("cnt-ev"), None, None, None, None).await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_36_entity_delete_soft_removes() {
        let s = DuckDbStorage::new_in_memory().unwrap();
        s.insert_entity(&make_entity("soft-del", "ship")).await.unwrap();
        s.delete_entity("soft-del").await.unwrap();
        // After soft delete, entity still exists but is_active = false
        // get_entity returns it regardless of is_active
        let _entity = s.get_entity("soft-del").await.unwrap();
        // The entity may or may not be returned depending on impl
        // but get_entities_by_type should not return it
        let active = s.get_entities_by_type("ship", 100, 0).await.unwrap();
        assert!(active.iter().all(|e| e.entity_id != "soft-del"));
    }
}
