# ORP Core Engine Specification
**Version:** 1.0
**Date:** March 26, 2026
**Audience:** 490 senior engineers (systems, databases, distributed systems, compilers)
**Scope:** Phase 1 MVP (Maritime template, single-node, Linux x86_64 + macOS)

---

## Section 1: Cargo Workspace Structure

### 1.1 Workspace Layout

```
orp/
├── Cargo.toml                          # Workspace root
├── Cargo.lock
├── .github/
│   └── workflows/
│       ├── ci.yml
│       ├── release.yml
│       └── benchmark.yml
├── crates/
│   ├── orp-core/                       # Main binary crate
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── cli/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── args.rs             # clap argument parser
│   │   │   │   └── commands.rs
│   │   │   ├── server/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── http.rs             # Axum HTTP server
│   │   │   │   ├── websocket.rs        # WebSocket updates
│   │   │   │   └── handlers.rs
│   │   │   ├── run.rs                  # Orchestration (entry point)
│   │   │   └── telemetry.rs
│   │   └── build.rs
│   │
│   ├── orp-storage/                    # OLAP + Graph abstraction
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── duckdb_engine.rs        # DuckDB backend
│   │   │   ├── kuzu_engine.rs          # Kuzu graph backend
│   │   ├── kuzu_sync.rs                # DuckDB → Kuzu sync (30s)
│   │   └── traits.rs                   # Storage trait
│   │
│   ├── orp-stream/                     # Stream processing & dedup
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── processor.rs            # StreamProcessor trait impl
│   │   │   ├── dedup.rs                # Dedup window (RocksDB)
│   │   │   ├── windowing.rs            # Tumbling/sliding windows
│   │   │   └── alerting.rs             # Monitor agents
│   │   └── tests/
│   │
│   ├── orp-connector/                  # Connector trait + adapters
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── traits.rs               # Connector trait
│   │   │   ├── adapters/
│   │   │   │   ├── ais.rs              # AIS/NMEA TCP receiver
│   │   │   │   ├── adsb.rs             # ADS-B TCP receiver
│   │   │   │   ├── http.rs             # REST API polling
│   │   │   │   ├── mqtt.rs             # MQTT subscriber
│   │   │   │   ├── csv_watcher.rs      # File-based CSV watch
│   │   │   │   └── websocket_client.rs # WebSocket client
│   │   ├── config/
│   │   │   ├── mod.rs
│   │   │   └── schemas.rs              # Schema validation
│   │   └── tests/fixtures/
│   │
│   ├── orp-query/                      # Query engine (ORP-QL v0.1)
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── parser.rs               # ORP-QL lexer/parser (nom)
│   │   │   ├── executor.rs             # Query execution engine
│   │   │   ├── plan.rs                 # Query planning & optimization
│   │   │   ├── ast.rs                  # Abstract syntax tree
│   │   │   └── functions.rs            # Built-in functions
│   │   └── tests/
│   │
│   ├── orp-entity/                     # Entity resolution
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── resolver.rs             # EntityResolver trait
│   │   │   ├── structural.rs           # MMSI/ICAO exact matching
│   │   │   ├── storage.rs              # Entity canonicalization
│   │   │   └── graph.rs                # Entity graph building
│   │   └── tests/
│   │
│   ├── orp-config/                     # Configuration system
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── yaml.rs                 # YAML config parsing
│   │   │   ├── schema.rs               # Config schema (serde)
│   │   │   ├── validation.rs           # Config validation
│   │   │   └── templates.rs            # Template system
│   │   └── templates/
│   │       └── maritime.yaml
│   │
│   ├── orp-audit/                      # Audit logging
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── logger.rs               # Append-only audit log
│   │   │   └── crypto.rs               # Ed25519 signing
│   │   └── tests/
│   │
│   ├── orp-proto/                      # Protocol buffers
│   │   ├── Cargo.toml
│   │   ├── build.rs
│   │   ├── proto/
│   │   │   ├── event.proto
│   │   │   ├── entity.proto
│   │   │   └── query.proto
│   │   └── src/lib.rs
│   │
│   ├── orp-geospatial/                 # Geospatial utilities
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── geo.rs                  # GIS operations (DuckDB Spatial)
│   │   │   └── index.rs                # R-tree index operations
│   │   └── tests/
│   │
│   ├── orp-security/                   # Auth, ABAC, crypto
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── oidc.rs                 # OIDC integration
│   │   │   ├── abac.rs                 # Attribute-based access control
│   │   │   ├── signer.rs               # Ed25519 operations
│   │   │   └── vault.rs                # Credential storage (encrypted)
│   │   └── tests/
│   │
│   └── orp-testbed/                    # Integration test fixtures
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs
│       │   ├── synthetic.rs            # Synthetic data generators
│       │   ├── fixtures.rs             # Test fixture utilities
│       │   └── benchmarks.rs           # Benchmark harness
│       └── benches/
│           ├── query_latency.rs
│           ├── stream_throughput.rs
│           └── graph_traversal.rs
│
└── docs/
    ├── ARCHITECTURE.md
    ├── CONNECTOR_DEV_GUIDE.md
    └── SECURITY_ARCHITECTURE.md
```

### 1.2 Dependency Graph

```
orp-core
  ├→ orp-server (HTTP/WS)
  ├→ orp-storage (trait abstraction)
  ├→ orp-stream (stream processor)
  ├→ orp-connector (adapters)
  ├→ orp-query (ORP-QL)
  ├→ orp-config (YAML)
  ├→ orp-security (OIDC, ABAC)
  ├→ orp-audit (append-only log)
  └→ orp-telemetry (OpenTelemetry)

orp-storage
  ├→ orp-proto (protobuf messages)
  └→ duckdb-rs, kuzu-rs (C++ FFI bindings)

orp-stream
  ├→ orp-storage
  └→ rocksdb (state backend)

orp-connector
  ├→ orp-storage
  └→ tokio (async runtime)

orp-query
  ├→ orp-storage
  ├→ nom (parser combinators)
  └→ arrow (columnar data)

orp-entity
  ├→ orp-storage
  └→ orp-query
```

### 1.3 Key Crate Dependencies (Cargo.toml excerpt)

**orp-core:**
```toml
[dependencies]
tokio = { version = "1.35", features = ["full"] }
axum = "0.7"
tower = "0.4"
tracing = "0.1"
tracing-subscriber = "0.3"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
serde_yaml = "0.9"

[dependencies.orp-storage]
path = "../orp-storage"

[dependencies.orp-stream]
path = "../orp-stream"

[dependencies.orp-connector]
path = "../orp-connector"
```

**orp-storage:**
```toml
[dependencies]
duckdb = { version = "0.9", features = ["bundled"] }
kuzu = "0.x"  # Native Rust bindings
rocksdb = "0.22"
serde = { version = "1.0", features = ["derive"] }
arrow = "51.0"

[build-dependencies]
cc = "1.0"
```

---

## Section 2: DuckDB Schema (OLAP Layer)

### 2.1 Initialization Script

All tables created on startup. DuckDB runs embedded, file-based (`data.duckdb`).

```sql
-- ENTITIES: All typed objects (ships, ports, aircraft, weather, etc.)
CREATE TABLE IF NOT EXISTS entities (
  entity_id VARCHAR PRIMARY KEY,                    -- "ship_477280410", "port_rotterdam", "aircraft_n123ab"
  entity_type VARCHAR NOT NULL,                     -- 'ship', 'port', 'aircraft', 'weather', etc.
  canonical_id VARCHAR UNIQUE,                      -- Canonical entity ID (after entity resolution)
  name VARCHAR,
  first_seen TIMESTAMP DEFAULT CURRENT_TIMESTAMP,   -- When first observed
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP, -- Last property update
  confidence FLOAT DEFAULT 1.0,                     -- Confidence score (0.0-1.0)
  source_count INTEGER DEFAULT 1,                   -- Number of sources reporting this entity
  is_canonical BOOLEAN DEFAULT FALSE                -- True if this is the canonical entity
);

CREATE INDEX idx_entities_type ON entities(entity_type);
CREATE INDEX idx_entities_canonical ON entities(canonical_id);
CREATE INDEX idx_entities_last_updated ON entities(last_updated);
CREATE UNIQUE INDEX idx_entities_entity_id ON entities(entity_id);

-- ENTITY_PROPERTIES: Typed key-value store (dynamic properties)
CREATE TABLE IF NOT EXISTS entity_properties (
  id INTEGER PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  entity_id VARCHAR NOT NULL,
  property_key VARCHAR NOT NULL,                    -- e.g., 'position', 'speed', 'temperature'
  property_value VARCHAR NOT NULL,                  -- JSON serialized
  property_type VARCHAR NOT NULL,                   -- 'float', 'int', 'string', 'geometry', 'timestamp'
  source_id VARCHAR NOT NULL,                       -- Which data source provided this
  timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  confidence FLOAT DEFAULT 1.0,                     -- Per-property confidence
  is_latest BOOLEAN DEFAULT TRUE,                   -- Part of latest state

  FOREIGN KEY (entity_id) REFERENCES entities(entity_id)
);

CREATE INDEX idx_entity_props_entity ON entity_properties(entity_id);
CREATE INDEX idx_entity_props_key ON entity_properties(property_key);
CREATE INDEX idx_entity_props_timestamp ON entity_properties(timestamp);
CREATE INDEX idx_entity_props_latest ON entity_properties(is_latest);

-- GEOSPATIAL PROPERTIES (optimized index for geo queries)
CREATE TABLE IF NOT EXISTS entity_geometry (
  entity_id VARCHAR PRIMARY KEY,
  geometry ST_GEOMETRY NOT NULL,                    -- DuckDB Spatial type
  latitude FLOAT NOT NULL,
  longitude FLOAT NOT NULL,
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

  FOREIGN KEY (entity_id) REFERENCES entities(entity_id)
);

CREATE INDEX idx_geom_spatial ON entity_geometry USING RTREE(geometry);
CREATE INDEX idx_geom_coords ON entity_geometry(latitude, longitude);

-- EVENTS: All temporal facts (position updates, state changes, alerts)
CREATE TABLE IF NOT EXISTS events (
  event_id VARCHAR PRIMARY KEY,                     -- "evt_abc123def456"
  entity_id VARCHAR NOT NULL,
  event_type VARCHAR NOT NULL,                      -- 'position_update', 'course_change', 'port_arrival', 'alert'
  event_timestamp TIMESTAMP NOT NULL,               -- When the event occurred (in external time)
  ingestion_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP, -- When ORP saw it
  source_id VARCHAR NOT NULL,                       -- Which connector provided this
  event_data JSON NOT NULL,                         -- Full event payload
  confidence FLOAT DEFAULT 1.0,
  severity VARCHAR DEFAULT 'info',                  -- 'info', 'warning', 'critical'

  FOREIGN KEY (entity_id) REFERENCES entities(entity_id),
  FOREIGN KEY (source_id) REFERENCES data_sources(source_id)
);

CREATE INDEX idx_events_entity ON events(entity_id);
CREATE INDEX idx_events_type ON events(event_type);
CREATE INDEX idx_events_timestamp ON events(event_timestamp);
CREATE INDEX idx_events_severity ON events(severity);
CREATE INDEX idx_events_ingestion ON events(ingestion_timestamp);

-- RELATIONSHIPS: Entity-to-entity edges
CREATE TABLE IF NOT EXISTS relationships (
  relationship_id VARCHAR PRIMARY KEY,              -- "rel_abc123"
  source_entity_id VARCHAR NOT NULL,
  target_entity_id VARCHAR NOT NULL,
  relationship_type VARCHAR NOT NULL,               -- 'docked_at', 'owns', 'threatens', 'next_port'
  properties JSON,                                  -- e.g., {"confidence": 0.95, "source": "ais"}
  created_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  last_confirmed_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  confidence FLOAT DEFAULT 1.0,
  is_active BOOLEAN DEFAULT TRUE,

  FOREIGN KEY (source_entity_id) REFERENCES entities(entity_id),
  FOREIGN KEY (target_entity_id) REFERENCES entities(entity_id)
);

CREATE INDEX idx_rels_source ON relationships(source_entity_id);
CREATE INDEX idx_rels_target ON relationships(target_entity_id);
CREATE INDEX idx_rels_type ON relationships(relationship_type);
CREATE INDEX idx_rels_active ON relationships(is_active);

-- DATA_SOURCES: Registered connectors & trust scores
CREATE TABLE IF NOT EXISTS data_sources (
  source_id VARCHAR PRIMARY KEY,                    -- "ais_global", "adsb_eu", "weather_noaa"
  source_name VARCHAR NOT NULL,
  source_type VARCHAR NOT NULL,                     -- 'ais', 'adsb', 'http', 'mqtt', 'csv', 'websocket'
  url VARCHAR,                                      -- Connection string if applicable
  enabled BOOLEAN DEFAULT TRUE,
  trust_score FLOAT DEFAULT 0.8,                    -- Confidence in data from this source (0.0-1.0)
  last_heartbeat TIMESTAMP,                         -- Last successful connection
  events_ingested_total INTEGER DEFAULT 0,
  entities_provided_total INTEGER DEFAULT 0,
  error_count INTEGER DEFAULT 0,
  certificate_fingerprint VARCHAR,                  -- Ed25519 public key for signing verification
  created_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_sources_type ON data_sources(source_type);
CREATE INDEX idx_sources_enabled ON data_sources(enabled);

-- AUDIT_LOG: Immutable, append-only, hash-chained
CREATE TABLE IF NOT EXISTS audit_log (
  sequence_number BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
  timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  operation VARCHAR NOT NULL,                       -- 'insert', 'update', 'delete', 'query'
  entity_type VARCHAR,
  entity_id VARCHAR,
  user_id VARCHAR,
  previous_hash VARCHAR,                            -- SHA256 of previous log entry
  content_hash VARCHAR NOT NULL,                    -- SHA256(sequence_number || operation || timestamp || entity || details)
  signature VARCHAR,                                -- Ed25519 signature over content_hash
  details JSON,

  UNIQUE (sequence_number)
);

CREATE INDEX idx_audit_timestamp ON audit_log(timestamp);
CREATE INDEX idx_audit_operation ON audit_log(operation);
CREATE INDEX idx_audit_entity ON audit_log(entity_type, entity_id);

-- SNAPSHOTS: Periodic state snapshots for fast historical queries
CREATE TABLE IF NOT EXISTS snapshots (
  snapshot_id VARCHAR PRIMARY KEY,                  -- "snap_20260326_150000"
  snapshot_timestamp TIMESTAMP NOT NULL,
  entity_state JSON NOT NULL,                       -- Serialized state of all entities
  relationship_state JSON NOT NULL,                 -- All relationships
  size_bytes BIGINT,
  created_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_snapshots_timestamp ON snapshots(snapshot_timestamp);

-- MONITOR_RULES: User-defined alert rules
CREATE TABLE IF NOT EXISTS monitor_rules (
  rule_id VARCHAR PRIMARY KEY,
  rule_name VARCHAR NOT NULL,
  entity_type VARCHAR NOT NULL,
  condition_sql VARCHAR NOT NULL,                   -- SQL condition: "speed > 25"
  action_type VARCHAR DEFAULT 'alert',              -- 'alert', 'log', 'webhook'
  action_target VARCHAR,                            -- Webhook URL if action_type = 'webhook'
  enabled BOOLEAN DEFAULT TRUE,
  created_timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  last_triggered TIMESTAMP
);

CREATE INDEX idx_monitor_rules_entity_type ON monitor_rules(entity_type);
CREATE INDEX idx_monitor_rules_enabled ON monitor_rules(enabled);
```

### 2.2 Partitioning Strategy

Events table is partitioned by month to enable fast deletion of old data:

```sql
ALTER TABLE events PARTITION BY RANGE_BUCKET(
  event_timestamp,
  INTERVAL '30 days'
);
```

Retention policy: Events older than 90 days are auto-deleted via scheduled task.

### 2.3 Data Type Mapping

| DuckDB Type | Usage | Example |
|---|---|---|
| `VARCHAR` | Entity/event IDs, names, types | `'ship_477280410'` |
| `FLOAT` | Position, speed, confidence, temperature | `23.45` |
| `INTEGER` | Counts, event severity codes | `1` |
| `TIMESTAMP` | Temporal facts | `'2026-03-26 14:30:00'` |
| `JSON` | Dynamic properties, event payloads | `{"speed": 12.5, "heading": 90}` |
| `ST_GEOMETRY` | Geospatial coordinates | Point, LineString, Polygon |
| `BIGINT` | Audit sequence numbers | `123456789` |

---

## Section 3: Kuzu Graph Schema

### 3.1 Graph Schema DDL

Kuzu is synced from DuckDB every 30 seconds. Schema is immutable once created.

```sql
-- NODE TABLES

-- Ships
CREATE NODE TABLE ship (
  mmsi INT64 PRIMARY KEY,
  entity_id STRING,
  name STRING,
  ship_type STRING,                               -- e.g., 'container_ship', 'tanker'
  imo INT64,
  callsign STRING,
  length DOUBLE,
  beam DOUBLE,
  draft DOUBLE,
  speed DOUBLE,
  heading INT64,
  latitude DOUBLE,
  longitude DOUBLE,
  destination STRING,
  last_updated TIMESTAMP,
  confidence DOUBLE
)

-- Ports
CREATE NODE TABLE port (
  port_id STRING PRIMARY KEY,
  entity_id STRING UNIQUE,
  name STRING,
  country STRING,
  latitude DOUBLE,
  longitude DOUBLE,
  region STRING
)

-- Aircraft
CREATE NODE TABLE aircraft (
  icao_hex STRING PRIMARY KEY,
  entity_id STRING UNIQUE,
  callsign STRING,
  aircraft_type STRING,
  latitude DOUBLE,
  longitude DOUBLE,
  altitude DOUBLE,
  speed DOUBLE,
  heading INT64,
  last_updated TIMESTAMP
)

-- Weather Systems
CREATE NODE TABLE weather_system (
  system_id STRING PRIMARY KEY,
  entity_id STRING UNIQUE,
  system_type STRING,                              -- 'hurricane', 'low_pressure', 'storm'
  center_lat DOUBLE,
  center_lon DOUBLE,
  radius_km DOUBLE,
  intensity DOUBLE,                                -- 0.0-1.0
  last_updated TIMESTAMP
)

-- Organizations
CREATE NODE TABLE organization (
  org_id STRING PRIMARY KEY,
  entity_id STRING UNIQUE,
  name STRING,
  type STRING,                                     -- 'shipping_line', 'port_authority', 'insurer'
  country STRING
)

-- Routes
CREATE NODE TABLE route (
  route_id STRING PRIMARY KEY,
  entity_id STRING UNIQUE,
  name STRING,
  origin_port STRING,
  destination_port STRING,
  waypoints JSON
)

-- Sensors/IoT Devices
CREATE NODE TABLE sensor (
  sensor_id STRING PRIMARY KEY,
  entity_id STRING UNIQUE,
  sensor_type STRING,                              -- 'temperature', 'pressure', 'gps', 'lidar'
  location_lat DOUBLE,
  location_lon DOUBLE,
  last_reading DOUBLE,
  last_updated TIMESTAMP
)

-- RELATIONSHIP TABLES

-- Ships docked at ports
CREATE REL TABLE docked_at (
  FROM ship TO port,
  docking_timestamp TIMESTAMP,
  undocking_timestamp TIMESTAMP,
  berth_number STRING,
  confidence DOUBLE
)

-- Ships heading to ports
CREATE REL TABLE heading_to (
  FROM ship TO port,
  estimated_arrival TIMESTAMP,
  confidence DOUBLE
)

-- Ships owned by organizations
CREATE REL TABLE owned_by (
  FROM ship TO organization,
  ownership_start TIMESTAMP,
  confidence DOUBLE
)

-- Ships managed by organizations
CREATE REL TABLE managed_by (
  FROM ship TO organization,
  management_start TIMESTAMP,
  confidence DOUBLE
)

-- Organizations insure ships
CREATE REL TABLE insures (
  FROM organization TO ship,
  policy_number STRING,
  policy_start TIMESTAMP,
  policy_end TIMESTAMP,
  confidence DOUBLE
)

-- Ports are in regions/countries
CREATE REL TABLE in_region (
  FROM port TO organization,
  confidence DOUBLE
)

-- Weather threatens routes
CREATE REL TABLE threatens (
  FROM weather_system TO route,
  threat_level DOUBLE,                             -- 0.0-1.0
  distance_km DOUBLE,
  updated_timestamp TIMESTAMP,
  confidence DOUBLE
)

-- Routes traverse ports
CREATE REL TABLE traverse (
  FROM route TO port,
  sequence_order INT64,
  confidence DOUBLE
)

-- Ships follow routes
CREATE REL TABLE follows (
  FROM ship TO route,
  started TIMESTAMP,
  ended TIMESTAMP,
  deviation_degree DOUBLE,
  confidence DOUBLE
)

-- Aircraft near ships
CREATE REL TABLE near (
  FROM aircraft TO ship,
  distance_m DOUBLE,
  recorded_timestamp TIMESTAMP,
  confidence DOUBLE
)

-- Sensors deployed on ships
CREATE REL TABLE deployed_on (
  FROM sensor TO ship,
  deployment_start TIMESTAMP,
  confidence DOUBLE
)

-- Sensors measure at locations
CREATE REL TABLE measures_at (
  FROM sensor TO port,
  frequency_seconds INT64,
  confidence DOUBLE
)
```

### 3.2 Sync Mechanism (DuckDB → Kuzu)

Every 30 seconds, a background task:

1. Reads `entities` + `entity_properties` from DuckDB
2. Transforms into Kuzu node format
3. Reads `relationships` from DuckDB
4. Transforms into Kuzu relationship format
5. Executes COPY statements into Kuzu

**Implementation (in `orp-storage/src/kuzu_sync.rs`):**

```rust
pub async fn sync_duckdb_to_kuzu(
  duck_conn: &mut DuckDBConnection,
  kuzu_conn: &mut KuzuConnection,
  sync_interval: Duration,
) -> Result<()> {
  loop {
    // Fetch entities modified since last sync
    let entities = duck_conn.query("SELECT * FROM entities WHERE last_updated > ?1")?;

    // Transform and bulk-insert into Kuzu
    for entity in entities {
      match entity.entity_type {
        "ship" => {
          let stmt = format!(
            "COPY ship(mmsi, entity_id, name, ...) FROM [data] (format='parquet')",
            // Parquet streaming from DuckDB
          );
          kuzu_conn.execute(&stmt)?;
        },
        // ... other types
      }
    }

    // Sync relationships
    let rels = duck_conn.query("SELECT * FROM relationships WHERE last_confirmed > ?1")?;
    for rel in rels {
      let stmt = format!(
        "COPY {} FROM [data] (format='parquet')",
        rel.relationship_type
      );
      kuzu_conn.execute(&stmt)?;
    }

    tokio::time::sleep(sync_interval).await;
  }
}
```

---

## Section 4: Core Rust Traits

### 4.1 Storage Trait (orp-storage/src/traits.rs)

Abstract interface over DuckDB/Kuzu/RocksDB.

```rust
use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Entity {
  pub entity_id: String,
  pub entity_type: String,
  pub canonical_id: Option<String>,
  pub name: Option<String>,
  pub properties: HashMap<String, JsonValue>,
  pub confidence: f32,
  pub last_updated: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct Event {
  pub event_id: String,
  pub entity_id: String,
  pub event_type: String,
  pub event_timestamp: chrono::DateTime<chrono::Utc>,
  pub source_id: String,
  pub data: JsonValue,
  pub confidence: f32,
}

#[derive(Clone, Debug)]
pub struct Relationship {
  pub relationship_id: String,
  pub source_entity_id: String,
  pub target_entity_id: String,
  pub relationship_type: String,
  pub properties: HashMap<String, JsonValue>,
  pub confidence: f32,
  pub is_active: bool,
}

#[async_trait]
pub trait Storage: Send + Sync {
  // ENTITY OPERATIONS
  async fn insert_entity(&self, entity: Entity) -> Result<()>;
  async fn get_entity(&self, entity_id: &str) -> Result<Option<Entity>>;
  async fn get_entities_by_type(&self, entity_type: &str) -> Result<Vec<Entity>>;
  async fn update_entity_property(
    &self,
    entity_id: &str,
    key: String,
    value: JsonValue,
  ) -> Result<()>;
  async fn set_canonical_id(&self, entity_id: &str, canonical_id: &str) -> Result<()>;

  // GEOSPATIAL QUERIES
  async fn get_entities_in_radius(
    &self,
    lat: f64,
    lon: f64,
    radius_km: f64,
    entity_type: Option<&str>,
  ) -> Result<Vec<Entity>>;

  async fn get_entities_in_polygon(
    &self,
    polygon_wkt: &str,
    entity_type: Option<&str>,
  ) -> Result<Vec<Entity>>;

  // EVENT OPERATIONS
  async fn insert_event(&self, event: Event) -> Result<()>;
  async fn get_events_for_entity(
    &self,
    entity_id: &str,
    limit: usize,
  ) -> Result<Vec<Event>>;

  async fn get_events_in_time_range(
    &self,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
  ) -> Result<Vec<Event>>;

  async fn get_events_in_region(
    &self,
    lat: f64,
    lon: f64,
    radius_km: f64,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
  ) -> Result<Vec<Event>>;

  // RELATIONSHIP OPERATIONS
  async fn insert_relationship(&self, rel: Relationship) -> Result<()>;
  async fn get_outgoing_relationships(
    &self,
    source_entity_id: &str,
    rel_type: Option<&str>,
  ) -> Result<Vec<Relationship>>;

  async fn get_incoming_relationships(
    &self,
    target_entity_id: &str,
    rel_type: Option<&str>,
  ) -> Result<Vec<Relationship>>;

  // GRAPH OPERATIONS (Kuzu backend)
  async fn graph_query(&self, query_str: &str) -> Result<Vec<HashMap<String, JsonValue>>>;
  async fn path_query(
    &self,
    source_entity_id: &str,
    target_entity_id: &str,
    max_hops: usize,
  ) -> Result<Vec<Vec<Relationship>>>;

  // AUDIT OPERATIONS
  async fn log_audit(
    &self,
    operation: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    user_id: Option<&str>,
    details: JsonValue,
  ) -> Result<()>;

  // DATA SOURCE OPERATIONS
  async fn register_data_source(
    &self,
    source_id: &str,
    source_name: &str,
    source_type: &str,
    trust_score: f32,
  ) -> Result<()>;

  async fn update_source_heartbeat(&self, source_id: &str) -> Result<()>;
  async fn get_data_sources(&self) -> Result<Vec<DataSource>>;

  // TRANSACTION OPERATIONS
  async fn begin_transaction(&self) -> Result<()>;
  async fn commit_transaction(&self) -> Result<()>;
  async fn rollback_transaction(&self) -> Result<()>;

  // ADMINISTRATIVE
  async fn health_check(&self) -> Result<()>;
  async fn get_stats(&self) -> Result<StorageStats>;
}

#[derive(Debug, Clone)]
pub struct DataSource {
  pub source_id: String,
  pub source_name: String,
  pub source_type: String,
  pub trust_score: f32,
  pub events_ingested: u64,
}

#[derive(Debug, Clone)]
pub struct StorageStats {
  pub total_entities: u64,
  pub total_events: u64,
  pub total_relationships: u64,
  pub database_size_bytes: u64,
}
```

### 4.2 Connector Trait (orp-connector/src/traits.rs)

```rust
use async_trait::async_trait;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct SourceEvent {
  pub connector_id: String,
  pub entity_id: String,
  pub entity_type: String,
  pub properties: HashMap<String, JsonValue>,
  pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
pub struct ConnectorConfig {
  pub connector_id: String,
  pub connector_type: String, // 'ais', 'adsb', 'http', 'mqtt', 'csv', 'websocket'
  pub url: Option<String>,
  pub schedule: Option<String>, // Cron expression or "every Xs"
  pub entity_type: String,
  pub properties: HashMap<String, JsonValue>, // Custom connector-specific properties
  pub enabled: bool,
  pub trust_score: f32,
}

#[async_trait]
pub trait Connector: Send + Sync {
  /// Unique identifier for this connector instance
  fn connector_id(&self) -> &str;

  /// Start the connector; must send events to the output channel
  async fn start(
    &self,
    tx: mpsc::Sender<SourceEvent>,
  ) -> Result<()>;

  /// Stop the connector gracefully
  async fn stop(&self) -> Result<()>;

  /// Check if connector is healthy (e.g., can reach data source)
  async fn health_check(&self) -> Result<()>;

  /// Get connector configuration
  fn config(&self) -> &ConnectorConfig;

  /// Get statistics (events processed, errors, etc.)
  fn stats(&self) -> ConnectorStats;
}

#[derive(Clone, Debug)]
pub struct ConnectorStats {
  pub events_processed: u64,
  pub errors: u64,
  pub last_event_timestamp: Option<chrono::DateTime<chrono::Utc>>,
  pub uptime_seconds: u64,
}
```

### 4.3 StreamProcessor Trait (orp-stream/src/traits.rs)

```rust
use async_trait::async_trait;
use crate::connector::SourceEvent;

#[derive(Clone, Debug)]
pub struct StreamContext {
  pub event: SourceEvent,
  pub dedup_window_seconds: u64,
  pub batch_size: usize,
}

#[async_trait]
pub trait StreamProcessor: Send + Sync {
  /// Process a single event from a connector
  async fn process_event(&self, ctx: StreamContext) -> Result<()>;

  /// Flush buffered events to storage
  async fn flush(&self) -> Result<()>;

  /// Get current buffer size
  fn buffer_size(&self) -> usize;

  /// Get processing stats
  fn stats(&self) -> ProcessorStats;
}

#[derive(Clone, Debug)]
pub struct ProcessorStats {
  pub events_processed: u64,
  pub events_deduplicated: u64,
  pub events_stored: u64,
  pub average_latency_ms: f64,
  pub errors: u64,
}
```

### 4.4 QueryEngine Trait (orp-query/src/traits.rs)

```rust
use async_trait::async_trait;
use serde_json::Value as JsonValue;

#[derive(Clone, Debug)]
pub enum QueryType {
  StructuredQuery(String), // ORP-QL
  GraphQuery(String),      // Cypher-like
  GeospatialQuery(String), // WKT + filters
  TimeSeriesQuery(String), // Time-windowed aggregation
}

#[derive(Clone, Debug)]
pub struct QueryResult {
  pub rows: Vec<HashMap<String, JsonValue>>,
  pub columns: Vec<String>,
  pub execution_time_ms: f64,
  pub row_count: usize,
}

#[async_trait]
pub trait QueryEngine: Send + Sync {
  /// Execute a query and return results
  async fn execute(&self, query: &str, query_type: QueryType) -> Result<QueryResult>;

  /// Validate query syntax without executing
  fn validate(&self, query: &str, query_type: QueryType) -> Result<()>;

  /// Get query execution plan (for debugging/optimization)
  fn explain(&self, query: &str, query_type: QueryType) -> Result<String>;

  /// Get statistics about the database
  async fn get_stats(&self) -> Result<QueryStats>;
}

#[derive(Clone, Debug)]
pub struct QueryStats {
  pub total_entities: u64,
  pub total_events: u64,
  pub total_relationships: u64,
  pub cache_hit_rate: f32,
  pub average_query_latency_ms: f64,
}
```

### 4.5 EntityResolver Trait (orp-entity/src/traits.rs)

```rust
use async_trait::async_trait;

#[derive(Clone, Debug)]
pub struct ResolutionMatch {
  pub entity_id_1: String,
  pub entity_id_2: String,
  pub confidence: f32,
  pub match_type: MatchType,
}

#[derive(Clone, Debug)]
pub enum MatchType {
  ExactStructuralMatch,  // MMSI or ICAO match
  NameAndGeospatial,     // Name similarity + location
  TemporalCorrelation,   // Multiple updates from different sources in sync
}

#[async_trait]
pub trait EntityResolver: Send + Sync {
  /// Find probable matches for a given entity
  async fn find_matches(
    &self,
    entity_id: &str,
    candidate_count: usize,
  ) -> Result<Vec<ResolutionMatch>>;

  /// Merge two entities into one canonical entity
  async fn merge_entities(
    &self,
    entity_id_1: &str,
    entity_id_2: &str,
    canonical_id: &str,
  ) -> Result<()>;

  /// Get canonicalized entity ID
  async fn resolve(&self, entity_id: &str) -> Result<Option<String>>;

  /// Register a user-confirmed match for ML training
  async fn record_match(
    &self,
    entity_id_1: &str,
    entity_id_2: &str,
    is_match: bool,
  ) -> Result<()>;
}
```

---

## Section 5: Event Schema (Canonical Format)

### 5.1 Rust Struct (orp-proto/src/event.rs)

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;

/// Canonical ORP Event format — all events conform to this
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrpEvent {
  /// Unique event identifier (UUID v4)
  pub event_id: String,

  /// Which entity does this event describe?
  pub entity_id: String,

  /// What type of entity? ("ship", "port", "aircraft", "weather", etc.)
  pub entity_type: String,

  /// Classification of the event
  pub event_type: String, // "position_update", "property_change", "alert", "state_transition"

  /// When did this event occur in external/real-world time?
  pub event_timestamp: DateTime<Utc>,

  /// When did ORP ingest this event?
  pub ingestion_timestamp: DateTime<Utc>,

  /// Which connector/source provided this?
  pub source_id: String,

  /// Trust score of the source (0.0-1.0)
  pub source_trust: f32,

  /// The actual event payload (structured)
  pub payload: EventPayload,

  /// How confident is ORP in this event?
  pub confidence: f32,

  /// Alert severity if this is an alert event
  pub severity: Option<EventSeverity>,

  /// Cryptographic signature (Ed25519) from data source
  pub signature: Option<String>,

  /// Audit trail metadata
  pub audit: AuditMetadata,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventPayload {
  /// Entity position update
  PositionUpdate {
    latitude: f64,
    longitude: f64,
    altitude: Option<f64>,
    accuracy_meters: Option<f32>,
    speed_knots: Option<f32>,
    heading_degrees: Option<f32>,
  },

  /// Entity property changed
  PropertyChange {
    property_key: String,
    old_value: Option<JsonValue>,
    new_value: JsonValue,
    is_derived: bool, // true if computed, not observed
  },

  /// State transition event
  StateTransition {
    old_state: String,
    new_state: String,
    reason: Option<String>,
  },

  /// Relationship created or modified
  RelationshipChange {
    related_entity_id: String,
    relationship_type: String,
    added: bool, // true if added, false if removed
    properties: HashMap<String, JsonValue>,
  },

  /// Alert fired
  AlertTriggered {
    alert_rule_id: String,
    alert_rule_name: String,
    condition: String,
    message: String,
    metadata: HashMap<String, JsonValue>,
  },

  /// Arbitrary event (catch-all)
  Custom {
    data: JsonValue,
  },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EventSeverity {
  Info,
  Warning,
  Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditMetadata {
  pub user_id: Option<String>,
  pub api_key_id: Option<String>,
  pub operation_id: String,
}

impl OrpEvent {
  /// Create a new event with defaults
  pub fn new(
    entity_id: String,
    entity_type: String,
    event_type: String,
    payload: EventPayload,
    source_id: String,
    source_trust: f32,
  ) -> Self {
    Self {
      event_id: uuid::Uuid::new_v4().to_string(),
      entity_id,
      entity_type,
      event_type,
      event_timestamp: Utc::now(),
      ingestion_timestamp: Utc::now(),
      source_id,
      source_trust,
      payload,
      confidence: source_trust,
      severity: None,
      signature: None,
      audit: AuditMetadata {
        user_id: None,
        api_key_id: None,
        operation_id: uuid::Uuid::new_v4().to_string(),
      },
    }
  }

  /// Convert to JSON
  pub fn to_json(&self) -> serde_json::Result<String> {
    serde_json::to_string(self)
  }

  /// Convert to Protobuf bytes
  pub fn to_protobuf(&self) -> Result<Vec<u8>, prost::EncodeError> {
    let proto = self.to_proto();
    let mut buf = Vec::new();
    prost::Message::encode(&proto, &mut buf)?;
    Ok(buf)
  }

  fn to_proto(&self) -> crate::protos::Event {
    // Implementation converts to Protobuf message
    todo!()
  }
}
```

### 5.2 Protobuf Definition (orp-proto/proto/event.proto)

```protobuf
syntax = "proto3";

package orp.event;

message OrpEvent {
  string event_id = 1;
  string entity_id = 2;
  string entity_type = 3;
  string event_type = 4;

  google.protobuf.Timestamp event_timestamp = 5;
  google.protobuf.Timestamp ingestion_timestamp = 6;

  string source_id = 7;
  float source_trust = 8;

  EventPayload payload = 9;
  float confidence = 10;

  enum EventSeverity {
    INFO = 0;
    WARNING = 1;
    CRITICAL = 2;
  }
  EventSeverity severity = 11;

  string signature = 12;
  AuditMetadata audit = 13;
}

message EventPayload {
  oneof variant {
    PositionUpdate position_update = 1;
    PropertyChange property_change = 2;
    StateTransition state_transition = 3;
    RelationshipChange relationship_change = 4;
    AlertTriggered alert_triggered = 5;
    CustomData custom = 6;
  }
}

message PositionUpdate {
  double latitude = 1;
  double longitude = 2;
  google.protobuf.DoubleValue altitude = 3;
  google.protobuf.FloatValue accuracy_meters = 4;
  google.protobuf.FloatValue speed_knots = 5;
  google.protobuf.FloatValue heading_degrees = 6;
}

message PropertyChange {
  string property_key = 1;
  google.protobuf.StringValue old_value = 2;
  string new_value = 3;
  bool is_derived = 4;
}

message StateTransition {
  string old_state = 1;
  string new_state = 2;
  google.protobuf.StringValue reason = 3;
}

message RelationshipChange {
  string related_entity_id = 1;
  string relationship_type = 2;
  bool added = 3;
  map<string, string> properties = 4;
}

message AlertTriggered {
  string alert_rule_id = 1;
  string alert_rule_name = 2;
  string condition = 3;
  string message = 4;
  map<string, string> metadata = 5;
}

message CustomData {
  google.protobuf.Struct data = 1;
}

message AuditMetadata {
  google.protobuf.StringValue user_id = 1;
  google.protobuf.StringValue api_key_id = 2;
  string operation_id = 3;
}
```

### 5.3 JSON Schema Example

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "ORP Event",
  "type": "object",
  "required": ["event_id", "entity_id", "entity_type", "event_type", "event_timestamp", "source_id", "payload"],
  "properties": {
    "event_id": { "type": "string", "format": "uuid" },
    "entity_id": { "type": "string" },
    "entity_type": { "enum": ["ship", "port", "aircraft", "weather", "organization", "route", "sensor"] },
    "event_type": { "enum": ["position_update", "property_change", "state_transition", "relationship_change", "alert_triggered", "custom"] },
    "event_timestamp": { "type": "string", "format": "date-time" },
    "ingestion_timestamp": { "type": "string", "format": "date-time" },
    "source_id": { "type": "string" },
    "source_trust": { "type": "number", "minimum": 0, "maximum": 1 },
    "payload": { "type": "object" },
    "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
    "severity": { "enum": ["info", "warning", "critical"] },
    "signature": { "type": "string" }
  }
}
```

---

## Section 6: Configuration Schema (YAML)

### 6.1 Master Configuration Format

**File:** `config.yaml` (loaded at startup)

```yaml
# ORP Configuration
# Version: 1.0

# Server configuration
server:
  host: "0.0.0.0"
  port: 9090
  workers: 4
  log_level: "info"                  # "trace", "debug", "info", "warn", "error"
  telemetry_enabled: true
  telemetry_endpoint: "http://localhost:4317"  # OTLP gRPC endpoint

# Storage configuration
storage:
  duckdb:
    path: "./data.duckdb"
    memory_limit_gb: 4
    max_connections: 10
    default_schema: "memory"          # "memory" for in-memory, "persistent" for disk

  kuzu:
    path: "./data.kuzu"
    memory_limit_gb: 2
    sync_interval_seconds: 30

  rocksdb:
    path: "./state.db"
    cache_size_mb: 512

  sqlite:
    path: "./config.sqlite"           # User settings, saved queries

# Data retention policy
retention:
  events_ttl_days: 90
  snapshots_ttl_days: 30
  audit_log_ttl_days: 365
  delete_batch_size: 10000

# Security configuration
security:
  oidc:
    enabled: false
    provider_url: "https://idp.example.com"
    client_id: "orp-client"
    client_secret: "${ORP_OIDC_CLIENT_SECRET}"  # Environment variable
    scopes: ["openid", "profile", "email"]
    redirect_uri: "http://localhost:9090/auth/callback"

  abac:
    enabled: true
    policy_file: "./policies.rego"   # OPA Rego policies

  signing:
    algorithm: "Ed25519"
    private_key_path: "./keys/private.key"

# Data source connectors
connectors:
  - name: "ais_global"
    type: "ais"
    enabled: true
    url: "tcp://ais.example.com:5631"
    entity_type: "ship"
    trust_score: 0.95
    retry_policy:
      max_retries: 5
      backoff_ms: 1000
    mapping:
      entity_id: "mmsi"
      name: "shipname"
      properties:
        speed: "sog"
        heading: "cog"
        destination: "destination"
        ship_type: "shiptype"

  - name: "adsb_eu"
    type: "adsb"
    enabled: true
    url: "tcp://adsb.example.com:30005"
    entity_type: "aircraft"
    trust_score: 0.90
    mapping:
      entity_id: "icao_hex"
      properties:
        altitude: "altitude"
        speed: "velocity"
        heading: "track"

  - name: "weather_noaa"
    type: "http"
    enabled: true
    url: "https://api.weather.gov/alerts/active"
    schedule: "every 5m"             # Cron or "every Xs"
    entity_type: "weather_system"
    trust_score: 0.85
    mapping:
      entity_id: "id"
      properties:
        severity: "properties.severity"
        center_lat: "geometry.coordinates[1]"
        center_lon: "geometry.coordinates[0]"

  - name: "internal_cargo"
    type: "http"
    enabled: true
    url: "https://internal-api.example.com/shipments"
    schedule: "every 1h"
    headers:
      Authorization: "Bearer ${CARGO_API_KEY}"
    entity_type: "shipment"
    trust_score: 0.99

  - name: "mqtt_sensors"
    type: "mqtt"
    enabled: true
    url: "mqtt://mqtt.example.com:1883"
    topic: "sensors/+/telemetry"
    entity_type: "sensor"
    trust_score: 0.80

# Entity resolution configuration
entity_resolution:
  enabled: true
  phase: "structural"               # "structural" (Phase 1) or "probabilistic" (Phase 2)
  structural:
    fields: ["mmsi", "icao_hex"]    # Unique identifiers per entity type
  probabilistic:
    enabled: false
    model_path: "./models/entity_resolution.pkl"  # Phase 2
    confidence_threshold: 0.85

# Monitoring & alerting
monitors:
  - rule_id: "course_change_alert"
    name: "Sudden course change"
    entity_type: "ship"
    condition: "ABS(heading - LAG(heading) OVER (PARTITION BY entity_id ORDER BY timestamp)) > 30"
    action: "alert"
    enabled: true

  - rule_id: "port_arrival_alert"
    name: "Ship entered port"
    entity_type: "ship"
    condition: "ST_DISTANCE(geometry, port_geometry) < 2"
    action: "alert"
    enabled: true

  - rule_id: "high_temp_sensor"
    name: "Sensor temperature high"
    entity_type: "sensor"
    condition: "temperature > 50"
    action: "alert"
    enabled: true

# API configuration
api:
  rate_limiting:
    enabled: true
    requests_per_minute: 100

  cors:
    enabled: true
    allowed_origins: ["*"]

  authentication:
    api_key_header: "X-API-Key"
    jwt_secret: "${JWT_SECRET}"

# Frontend configuration
frontend:
  enabled: true
  port: 9090
  assets_path: "./frontend/dist"
  default_map_center: [51.92, 4.27]   # [lat, lon] for Rotterdam
  default_zoom: 8

# Logging
logging:
  level: "info"
  format: "json"                      # "json" or "text"
  output: "stdout"                    # "stdout" or file path
  audit_log_path: "./audit.log"

# Templates (pre-configured setups)
templates:
  - name: "maritime"
    description: "Maritime monitoring template"
    connectors: ["ais_global", "adsb_eu", "weather_noaa"]
    sample_data_ttl_hours: 720        # 30 days of sample data
```

### 6.2 Validation Schema (serde + custom)

**File:** `orp-config/src/schema.rs`

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
  pub server: ServerConfig,
  pub storage: StorageConfig,
  pub retention: RetentionPolicy,
  pub security: SecurityConfig,
  pub connectors: Vec<ConnectorConfig>,
  pub entity_resolution: EntityResolutionConfig,
  pub monitors: Vec<MonitorRule>,
  pub api: ApiConfig,
  pub frontend: FrontendConfig,
  pub logging: LoggingConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConfig {
  pub host: String,
  pub port: u16,
  pub workers: u32,
  pub log_level: String,
  pub telemetry_enabled: bool,
  pub telemetry_endpoint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StorageConfig {
  pub duckdb: DuckDbConfig,
  pub kuzu: KuzuConfig,
  pub rocksdb: RocksDbConfig,
  pub sqlite: SqliteConfig,
}

// ... (more config structs)

impl Config {
  pub fn validate(&self) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    if self.server.port < 1024 && self.server.port != 0 {
      errors.push("Port must be >= 1024 or 0 (random)".to_string());
    }

    if self.storage.duckdb.memory_limit_gb < 1 {
      errors.push("DuckDB memory limit must be >= 1GB".to_string());
    }

    // Validate each connector
    for conn in &self.connectors {
      if conn.name.is_empty() {
        errors.push("Connector name cannot be empty".to_string());
      }
    }

    if errors.is_empty() {
      Ok(())
    } else {
      Err(errors)
    }
  }
}
```

---

## Section 7: Data Flow Diagram (ASCII)

```
┌────────────────────────────────────────────────────────────────────────────────┐
│                            ORP SINGLE BINARY DATA FLOW                         │
└────────────────────────────────────────────────────────────────────────────────┘

EXTERNAL DATA SOURCES
┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│ AIS Feed     │  │ ADS-B Feed   │  │ NOAA Weather │  │ Internal API │
│ (TCP/NMEA)   │  │ (TCP)        │  │ (HTTP REST)  │  │ (HTTP/JWT)   │
└───────┬──────┘  └───────┬──────┘  └───────┬──────┘  └───────┬──────┘
        │                 │                 │                 │
        └─────────────────┼─────────────────┼─────────────────┘
                          │
                          ▼
        ┌─────────────────────────────────────┐
        │   CONNECTOR LAYER (Tokio async)     │
        │  ├─ AIS Tap (TCP receiver)          │
        │  ├─ ADS-B Tap (TCP receiver)        │
        │  ├─ HTTP Connector (polling)        │
        │  ├─ MQTT Client (subscribe)         │
        │  ├─ CSV Watcher (file polling)      │
        │  └─ WebSocket Client                │
        └─────────────┬──────────────────────┘
                      │
      SourceEvent channel (mpsc::Sender)
                      │
                      ▼
        ┌──────────────────────────────────────┐
        │   STREAM PROCESSOR (Dedup & Batch)   │
        │  ├─ RocksDB dedup window (60s)       │
        │  ├─ Change detection                 │
        │  ├─ Windowing (batches of 1000)      │
        │  └─ Buffering for flush              │
        └─────────────┬──────────────────────┘
                      │
         EntityEvent channel (batch insert)
                      │
                      ▼
        ┌──────────────────────────────────────┐
        │   ENTITY RESOLUTION LAYER            │
        │  ├─ Structural matching (MMSI/ICAO)  │
        │  └─ Canonical ID assignment          │
        └─────────────┬──────────────────────┘
                      │
            Resolved events → storage::insert()
                      │
                      ▼
        ┌──────────────────────────────────────────┐
        │   DUCKDB ENGINE (OLAP + Geospatial)      │
        │  ├─ Entities table (with geometry index) │
        │  ├─ Entity_properties (key-value pairs)  │
        │  ├─ Events table (immutable)             │
        │  ├─ Relationships table                  │
        │  ├─ R-tree geospatial index              │
        │  ├─ Audit log (hash-chained)             │
        │  └─ Data sources registry                │
        │                                          │
        │  File: data.duckdb (~500MB-2GB)         │
        └─────────────┬──────────────────────┘
                      │
          Every 30 seconds (background task)
                      │
                      ▼
        ┌──────────────────────────────────────────┐
        │   KUZU GRAPH ENGINE (Native Graph DB)    │
        │  ├─ Ships, Ports, Aircraft, Weather      │
        │  ├─ Organizations, Routes, Sensors       │
        │  ├─ Relationships (docked_at, owned_by,  │
        │  │   threatens, heading_to, etc.)        │
        │  └─ Path traversal & reachability        │
        │                                          │
        │  File: data.kuzu (~300MB-1GB)           │
        └─────────────┬──────────────────────┘
                      │
        ┌─────────────┴──────────────┐
        │                            │
        ▼                            ▼
   ┌──────────────┐        ┌──────────────────────┐
   │   HTTP API   │        │  WebSocket Server    │
   │   Axum       │        │  (Real-time updates) │
   │  ├─ /api/*   │        │                      │
   │  └─ /health  │        │  Broadcasts to       │
   │              │        │  connected clients   │
   └───────┬──────┘        └────────┬─────────────┘
           │                        │
           └────────────┬───────────┘
                        │
                        ▼
        ┌──────────────────────────────────────────┐
        │   QUERY ENGINE (ORP-QL v0.1)             │
        │  ├─ Parser (nom combinators)             │
        │  ├─ Query planner                        │
        │  ├─ DuckDB executor                      │
        │  ├─ Kuzu graph executor                  │
        │  └─ Result aggregator                    │
        └─────────────┬──────────────────────┘
                      │
              QueryResult (JSON)
                      │
                      ▼
        ┌──────────────────────────────────────────┐
        │   REACT FRONTEND (Web UI)                │
        │  ├─ Map view (Deck.gl 2D, CesiumJS 3D)  │
        │  ├─ Entity inspector panel               │
        │  ├─ Query bar + autocomplete             │
        │  ├─ Timeline scrubber                    │
        │  └─ Alert feed                           │
        │                                          │
        │  HTTP: GET /api/ships?lat=X&lon=Y        │
        │  WebSocket: Real-time position updates   │
        └──────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────────────────┐
│  SECURITY LAYER (Orthogonal to above)                                        │
│  ├─ OIDC authentication (optional)                                           │
│  ├─ ABAC policies (what data can user see?)                                  │
│  ├─ Ed25519 signing (verify data sources)                                    │
│  ├─ Audit logging (append-only, hash-chained)                                │
│  └─ Encrypted credential vault                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## Section 8: Error Handling Strategy

### 8.1 Error Type Hierarchy

```rust
// orp-core/src/error.rs

use std::fmt;
use axum::http::StatusCode;

#[derive(Debug)]
pub enum OrpError {
  // Storage layer
  StorageError(String),
  DatabaseError(String),
  TransactionError(String),

  // Connector layer
  ConnectorError(String),
  ConnectorNotFound(String),
  DataSourceError(String),

  // Stream processing
  StreamProcessorError(String),
  DeduplicationError(String),

  // Entity resolution
  EntityResolutionError(String),

  // Query execution
  QueryError(String),
  QueryValidationError(String),
  QueryTimeoutError,

  // Security
  AuthenticationError(String),
  AuthorizationError(String),
  SignatureVerificationError(String),

  // Configuration
  ConfigError(String),
  ValidationError(Vec<String>),

  // FFI (Foreign Function Interface)
  FfiError(String), // DuckDB/Kuzu C++ errors

  // Network
  NetworkError(String),
  TimeoutError,

  // System
  IoError(std::io::Error),
  SerializationError(String),

  // Unknown
  Unknown(String),
}

impl fmt::Display for OrpError {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match self {
      Self::StorageError(msg) => write!(f, "Storage error: {}", msg),
      Self::DatabaseError(msg) => write!(f, "Database error: {}", msg),
      // ... etc
      _ => write!(f, "Unknown error"),
    }
  }
}

impl std::error::Error for OrpError {}

// Convert to HTTP response
impl From<OrpError> for (StatusCode, String) {
  fn from(err: OrpError) -> Self {
    match err {
      OrpError::QueryError(_) => (StatusCode::BAD_REQUEST, err.to_string()),
      OrpError::AuthenticationError(_) => (StatusCode::UNAUTHORIZED, err.to_string()),
      OrpError::AuthorizationError(_) => (StatusCode::FORBIDDEN, err.to_string()),
      OrpError::QueryTimeoutError => (StatusCode::REQUEST_TIMEOUT, "Query timeout".to_string()),
      OrpError::DatabaseError(_) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
      _ => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
  }
}

pub type Result<T> = std::result::Result<T, OrpError>;
```

### 8.2 FFI Error Handling (DuckDB/Kuzu)

```rust
// orp-storage/src/ffi_errors.rs

/// Wrapper for C++ FFI error handling
/// DuckDB and Kuzu return raw C error codes; we translate to Rust Result<T>

pub unsafe fn handle_duckdb_result<T>(
  result: duckdb_rs::Result<T>,
  context: &str,
) -> Result<T> {
  match result {
    Ok(value) => Ok(value),
    Err(e) => {
      // Log to audit trail
      tracing::error!("DuckDB FFI error in {}: {}", context, e);
      Err(OrpError::FfiError(format!("DuckDB: {} ({})", e, context)))
    }
  }
}

pub unsafe fn handle_kuzu_result<T>(
  result: kuzu_rs::Result<T>,
  context: &str,
) -> Result<T> {
  match result {
    Ok(value) => Ok(value),
    Err(e) => {
      tracing::error!("Kuzu FFI error in {}: {}", context, e);
      Err(OrpError::FfiError(format!("Kuzu: {} ({})", e, context)))
    }
  }
}
```

### 8.3 Retry Policies

```rust
// orp-core/src/retry.rs

use std::time::Duration;
use tokio::time::sleep;

pub struct RetryPolicy {
  pub max_retries: u32,
  pub initial_backoff: Duration,
  pub max_backoff: Duration,
  pub backoff_multiplier: f64,
}

impl Default for RetryPolicy {
  fn default() -> Self {
    Self {
      max_retries: 5,
      initial_backoff: Duration::from_millis(100),
      max_backoff: Duration::from_secs(60),
      backoff_multiplier: 2.0,
    }
  }
}

impl RetryPolicy {
  pub async fn execute<F, T, Fut>(&self, mut f: F) -> Result<T>
  where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
  {
    let mut attempt = 0;
    let mut backoff = self.initial_backoff;

    loop {
      match f().await {
        Ok(result) => return Ok(result),
        Err(e) => {
          attempt += 1;
          if attempt >= self.max_retries {
            return Err(e);
          }

          tracing::warn!(
            "Attempt {} failed: {}. Retrying in {:?}",
            attempt,
            e,
            backoff
          );

          sleep(backoff).await;
          backoff = Duration::from_secs_f64(
            (backoff.as_secs_f64() * self.backoff_multiplier)
              .min(self.max_backoff.as_secs_f64()),
          );
        }
      }
    }
  }
}
```

### 8.4 Dead Letter Queue

```rust
// orp-stream/src/dlq.rs

use rocksdb::DB;
use serde_json::json;

pub struct DeadLetterQueue {
  db: DB,
}

impl DeadLetterQueue {
  pub fn new(path: &str) -> Result<Self> {
    let db = DB::open_default(path)?;
    Ok(Self { db })
  }

  /// Record a failed event for manual inspection
  pub fn record_failure(
    &self,
    event_id: &str,
    event: &[u8],
    error: &str,
  ) -> Result<()> {
    let dlq_entry = json!({
      "event_id": event_id,
      "failed_at": chrono::Utc::now(),
      "error": error,
      "retry_count": 0,
      "event_bytes": std::str::from_utf8(event)?,
    });

    self.db.put(
      event_id.as_bytes(),
      dlq_entry.to_string().as_bytes(),
    )?;
    Ok(())
  }

  /// Get all failed events for analysis
  pub fn get_failures(&self, limit: usize) -> Result<Vec<(String, String)>> {
    let iter = self.db.iterator(rocksdb::IteratorMode::from_start());
    let mut result = Vec::new();

    for (key, value) in iter.take(limit) {
      let event_id = String::from_utf8_lossy(&key).to_string();
      let entry = String::from_utf8_lossy(&value).to_string();
      result.push((event_id, entry));
    }

    Ok(result)
  }
}
```

---

## Section 9: Testing Strategy

### 9.1 Unit Test Patterns

**Example: Entity Resolver Unit Test**

```rust
// orp-entity/src/structural.rs

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn test_mmsi_exact_match() {
    let resolver = StructuralResolver::new();

    let entity1 = Entity {
      entity_id: "ship_1".to_string(),
      entity_type: "ship".to_string(),
      properties: {
        let mut m = HashMap::new();
        m.insert("mmsi".to_string(), json!(477280410));
        m
      },
      ..Default::default()
    };

    let entity2 = Entity {
      entity_id: "ship_2".to_string(),
      entity_type: "ship".to_string(),
      properties: {
        let mut m = HashMap::new();
        m.insert("mmsi".to_string(), json!(477280410));
        m
      },
      ..Default::default()
    };

    let matches = resolver
      .find_matches(&entity1.entity_id, 10)
      .await
      .expect("resolution failed");

    assert!(!matches.is_empty());
    assert_eq!(matches[0].match_type, MatchType::ExactStructuralMatch);
    assert_eq!(matches[0].confidence, 1.0);
  }

  #[tokio::test]
  async fn test_icao_exact_match() {
    let resolver = StructuralResolver::new();

    let entity1 = Entity {
      entity_id: "aircraft_1".to_string(),
      entity_type: "aircraft".to_string(),
      properties: {
        let mut m = HashMap::new();
        m.insert("icao_hex".to_string(), json!("ABC123"));
        m
      },
      ..Default::default()
    };

    let entity2 = Entity {
      entity_id: "aircraft_2".to_string(),
      entity_type: "aircraft".to_string(),
      properties: {
        let mut m = HashMap::new();
        m.insert("icao_hex".to_string(), json!("ABC123"));
        m
      },
      ..Default::default()
    };

    let matches = resolver
      .find_matches(&entity1.entity_id, 10)
      .await
      .expect("resolution failed");

    assert!(!matches.is_empty());
    assert_eq!(matches[0].confidence, 1.0);
  }
}
```

### 9.2 Integration Test Fixtures

**File:** `orp-testbed/src/fixtures.rs`

```rust
use crate::synthetic::*;

pub async fn setup_test_database() -> (DuckDbConnection, KuzuConnection) {
  let duck_conn = DuckDbConnection::open_memory().unwrap();
  let kuzu_conn = KuzuConnection::new(":memory:").unwrap();

  // Create schema
  duck_conn
    .execute_batch(DUCKDB_SCHEMA_SQL)
    .expect("failed to create DuckDB schema");

  kuzu_conn
    .execute_batch(KUZU_SCHEMA_SQL)
    .expect("failed to create Kuzu schema");

  (duck_conn, kuzu_conn)
}

pub async fn load_maritime_test_data(
  duck_conn: &DuckDbConnection,
  count: usize,
) -> Result<()> {
  let ships = generate_synthetic_ships(count);
  let ports = generate_synthetic_ports(10);

  for ship in ships {
    duck_conn
      .insert_entity(&ship)
      .await
      .expect("insert ship failed");
  }

  for port in ports {
    duck_conn
      .insert_entity(&port)
      .await
      .expect("insert port failed");
  }

  Ok(())
}

#[tokio::test]
async fn test_query_ships_in_radius() {
  let (duck_conn, _kuzu_conn) = setup_test_database().await;
  load_maritime_test_data(&duck_conn, 1000).await.unwrap();

  // Query: ships within 50km of Rotterdam
  let results = duck_conn
    .get_entities_in_radius(51.92, 4.27, 50.0, Some("ship"))
    .await
    .expect("query failed");

  assert!(results.len() > 0);
  assert!(results.iter().all(|e| e.entity_type == "ship"));
}

#[tokio::test]
async fn test_graph_path_traversal() {
  let (duck_conn, kuzu_conn) = setup_test_database().await;
  load_maritime_test_data(&duck_conn, 500).await.unwrap();

  // Sync DuckDB → Kuzu
  sync_duckdb_to_kuzu(&mut duck_conn, &mut kuzu_conn, Duration::from_secs(1))
    .await
    .expect("sync failed");

  // Query: Find all ports reachable by ship within 2 hops
  let results = kuzu_conn
    .path_query("ship_1", "port_1", 2)
    .await
    .expect("graph query failed");

  assert!(results.len() > 0);
}
```

### 9.3 Benchmark Suite

**File:** `orp-testbed/benches/query_latency.rs`

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_simple_query(c: &mut Criterion) {
  c.bench_function("ships_in_radius_1m_entities", |b| {
    b.to_async().block_on(async {
      let (conn, _kuzu) = setup_test_database().await;
      load_maritime_test_data(&conn, 1_000_000).await.unwrap();

      black_box(
        conn
          .get_entities_in_radius(51.92, 4.27, 50.0, Some("ship"))
          .await
      )
    })
  });
}

fn bench_graph_query(c: &mut Criterion) {
  c.bench_function("path_traversal_3_hops", |b| {
    b.to_async().block_on(async {
      let (duck_conn, kuzu_conn) = setup_test_database().await;
      load_maritime_test_data(&duck_conn, 1_000_000).await.unwrap();

      black_box(
        kuzu_conn
          .path_query("ship_477280410", "port_rotterdam", 3)
          .await
      )
    })
  });
}

criterion_group!(benches, bench_simple_query, bench_graph_query);
criterion_main!(benches);
```

Run benchmarks:

```bash
cargo bench --all --features bench
```

---

## Section 10: Binary Build Pipeline

### 10.1 Build Targets & Cross-Compilation

**Supported Phase 1 targets:**

- `x86_64-unknown-linux-gnu` (primary)
- `x86_64-apple-darwin` (macOS Intel)
- `aarch64-apple-darwin` (macOS Apple Silicon)

**Phase 2 targets:**

- `aarch64-unknown-linux-gnu` (ARM Linux)
- `x86_64-pc-windows-msvc` (Windows)
- `aarch64-linux-android` (Android)

### 10.2 Build Script (build.rs)

**File:** `orp-core/build.rs`

```rust
use std::env;
use std::path::PathBuf;

fn main() {
  // Determine target platform
  let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
  let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();

  // Set compiler flags
  if target_os == "linux" {
    println!("cargo:rustc-link-lib=dylib=c++");
    println!("cargo:rustc-link-search=/usr/lib/x86_64-linux-gnu");
  } else if target_os == "macos" {
    println!("cargo:rustc-link-lib=dylib=c++");
    println!("cargo:rustc-link-search=/usr/local/lib");
  }

  // Link DuckDB statically
  println!("cargo:rustc-link-lib=static=duckdb");
  println!("cargo:rustc-link-search=/usr/local/lib");

  // Link Kuzu statically
  println!("cargo:rustc-link-lib=static=kuzu");

  // Generate Protobuf code
  prost_build::Config::new()
    .compile_protos(&["proto/event.proto"], &["proto"])
    .expect("protobuf compilation failed");

  // Print cargo directives
  println!("cargo:rerun-if-changed=build.rs");
  println!("cargo:rerun-if-changed=proto/");
}
```

### 10.3 Release Build (Cargo.toml)

**File:** `orp-core/Cargo.toml`

```toml
[profile.release]
opt-level = 3
lto = "fat"                 # Link-time optimization
codegen-units = 1          # Better optimization, slower build
strip = true               # Strip symbols from binary
panic = "abort"            # Smaller binary

[profile.bench]
inherits = "release"
lto = "thin"               # Faster builds for benchmarking
```

### 10.4 CI/CD Pipeline (.github/workflows/ci.yml)

```yaml
name: CI

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main, develop]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y libduckdb-dev libkuzu-dev

      - name: Run tests
        run: cargo test --all --verbose

      - name: Run clippy
        run: cargo clippy --all -- -D warnings

      - name: Check formatting
        run: cargo fmt --all -- --check

  build:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
          - os: macos-latest
            target: x86_64-apple-darwin
          - os: macos-latest
            target: aarch64-apple-darwin

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Build release binary
        run: cargo build --release --target ${{ matrix.target }}

      - name: Strip binary
        run: strip target/${{ matrix.target }}/release/orp || true

      - name: Get binary size
        run: ls -lh target/${{ matrix.target }}/release/orp

  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Run benchmarks
        run: cargo bench --all --features bench -- --output-format bencher | tee output.txt

      - name: Store benchmark result
        uses: benchmark-action/github-action@v1
        with:
          tool: 'cargo'
          output-file-path: output.txt

  release:
    runs-on: ${{ matrix.os }}
    if: startsWith(github.ref, 'refs/tags/')
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
          - os: macos-latest
            target: x86_64-apple-darwin
          - os: macos-latest
            target: aarch64-apple-darwin

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Build release
        run: cargo build --release --target ${{ matrix.target }}

      - name: Package binary
        run: |
          mkdir -p orp-${{ matrix.target }}
          cp target/${{ matrix.target }}/release/orp orp-${{ matrix.target }}/
          cp LICENSE orp-${{ matrix.target }}/
          tar czf orp-${{ matrix.target }}.tar.gz orp-${{ matrix.target }}/

      - name: Upload to release
        uses: softprops/action-gh-release@v1
        with:
          files: orp-${{ matrix.target }}.tar.gz
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

### 10.5 Binary Size Budget

```bash
# Target binary size: < 350MB (without AI model)

$ ls -lh target/release/orp
-rw-r--r-- 1 user group 287M Mar 26 14:23 orp

# Breakdown (via binary-size analysis):
# Rust core:           40MB
# DuckDB:              50MB
# Kuzu:                40MB
# RocksDB:             20MB
# Dependencies:        60MB
# Geospatial libs:     20MB
# React frontend:      20MB
# Connectors:          30MB
# Other:               27MB
# ────────────
# Total:              287MB ✓

# Measure regularly in CI:
cargo bloat --release -n 10
```

### 10.6 Linking Strategy

**Static linking** for DuckDB, Kuzu, RocksDB:

```bash
# Build DuckDB from source with static flags
git clone https://github.com/duckdb/duckdb.git
cd duckdb
cmake . -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF
make install

# Verify no runtime dependencies on system libraries
ldd target/release/orp | grep -i duckdb  # Should be empty
```

---

## Section 11: Module Documentation Index

Each crate includes comprehensive documentation:

- **orp-core:** `README.md` + `src/lib.rs` module docs
- **orp-storage:** Schema docs, Kuzu sync architecture, performance tuning
- **orp-stream:** Dedup window operation, windowing strategies
- **orp-connector:** Connector implementation guide with examples (AIS, HTTP)
- **orp-query:** ORP-QL grammar reference, query planning details
- **orp-entity:** Entity resolution algorithm, structural matching rules
- **orp-security:** OIDC integration guide, ABAC policy examples
- **orp-audit:** Audit log verification, hash-chain validation

Generated API docs available via:

```bash
cargo doc --open --all
```

---

## Section 12: Performance Targets (Revisited)

| Metric | Target | Acceptance Criteria |
|---|---|---|
| Binary size | < 350MB | Measured at release time |
| Startup time | < 5 seconds | Cold start, disk I/O included |
| Memory (1M entities) | < 3GB RAM | DuckDB + Kuzu + connectors |
| Simple query (p50) | < 200ms | "Ships near point" |
| Simple query (p99) | < 1s | Same, with high load |
| Graph query 3 hops (p50) | < 1s | Path traversal in Kuzu |
| Graph query 3 hops (p99) | < 5s | Under stress |
| Stream throughput | 100K events/sec | Sustained, with dedup |
| Connector startup | < 2s | TCP/HTTP/MQTT connected |
| Audit log latency | < 10ms | Synchronous write |

---

**This specification is the golden source. Every engineer on the build team should reference these exact schemas, traits, and module structures. No hand-waving. Executable code, exact DDL, exact trait signatures.**

**Version 1.0 locked. Ready for 490 engineers.**
