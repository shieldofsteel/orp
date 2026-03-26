//! Intelligence overlay layer system — Palantir-grade geospatial overlays.
//!
//! Provides a layer registry backed by DuckDB, REST endpoints for CRUD,
//! and built-in data for major shipping lanes and ICAO airspace boundaries.
//!
//! REST API:
//!   GET    /api/v1/layers          — list all layers
//!   POST   /api/v1/layers          — create a new layer
//!   GET    /api/v1/layers/:id      — get a specific layer
//!   PUT    /api/v1/layers/:id      — update a layer
//!   DELETE /api/v1/layers/:id      — delete a layer
//!   POST   /api/v1/layers/seed     — seed built-in layers

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use chrono::{DateTime, Utc};
use duckdb::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// ── Layer schema ──────────────────────────────────────────────────────────────

/// The data format / rendering type of a layer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerType {
    /// GeoJSON — polygon/line/point data embedded inline or via URL.
    GeoJson,
    /// Well-Known Text — simple geometry strings.
    Wkt,
    /// XYZ or TMS tile URL template (`{z}/{x}/{y}`).
    Tiles,
}

impl std::fmt::Display for LayerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayerType::GeoJson => write!(f, "geojson"),
            LayerType::Wkt => write!(f, "wkt"),
            LayerType::Tiles => write!(f, "tiles"),
        }
    }
}

impl std::str::FromStr for LayerType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "geojson" => Ok(LayerType::GeoJson),
            "wkt" => Ok(LayerType::Wkt),
            "tiles" => Ok(LayerType::Tiles),
            other => Err(format!("Unknown layer type: {}", other)),
        }
    }
}

/// A named intelligence overlay layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub layer_id: String,
    /// Machine-friendly key (e.g. `shipping_lanes`, `exclusion_zones`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    pub layer_type: LayerType,
    /// URL to fetch data from, or `None` if `data` is inline.
    pub source_url: Option<String>,
    /// Inline GeoJSON / WKT data (mutually exclusive with `source_url`).
    pub data: Option<String>,
    /// Whether the layer is visible to clients by default.
    pub visible: bool,
    /// Rendering opacity 0.0–1.0.
    pub opacity: f64,
    /// Z-index for render order (higher = on top).
    pub z_index: i32,
    /// Arbitrary style hints (colour, line width, fill, etc.) as JSON.
    pub style: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── Create / update DTOs ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateLayerRequest {
    pub name: String,
    pub description: Option<String>,
    pub layer_type: LayerType,
    pub source_url: Option<String>,
    pub data: Option<String>,
    pub visible: Option<bool>,
    pub opacity: Option<f64>,
    pub z_index: Option<i32>,
    pub style: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLayerRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub source_url: Option<String>,
    pub data: Option<String>,
    pub visible: Option<bool>,
    pub opacity: Option<f64>,
    pub z_index: Option<i32>,
    pub style: Option<serde_json::Value>,
}

// ── DuckDB DDL ────────────────────────────────────────────────────────────────

pub const LAYERS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS layers (
    layer_id    VARCHAR PRIMARY KEY,
    name        VARCHAR NOT NULL UNIQUE,
    description VARCHAR NOT NULL DEFAULT '',
    layer_type  VARCHAR NOT NULL,
    source_url  VARCHAR,
    data        TEXT,
    visible     BOOLEAN NOT NULL DEFAULT TRUE,
    opacity     DOUBLE NOT NULL DEFAULT 1.0,
    z_index     INTEGER NOT NULL DEFAULT 0,
    style       JSON,
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_layers_name    ON layers(name);
CREATE INDEX IF NOT EXISTS idx_layers_type    ON layers(layer_type);
CREATE INDEX IF NOT EXISTS idx_layers_visible ON layers(visible);
"#;

// ── Registry ──────────────────────────────────────────────────────────────────

/// Thread-safe layer registry backed by DuckDB.
#[derive(Clone)]
pub struct LayerRegistry {
    conn: Arc<Mutex<Connection>>,
}

impl LayerRegistry {
    /// Create a registry against an existing DuckDB connection.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, LayerError> {
        {
            let c = conn.lock().map_err(|_| LayerError::Lock)?;
            c.execute_batch(LAYERS_SCHEMA).map_err(LayerError::Db)?;
        }
        Ok(Self { conn })
    }

    // ── CRUD ─────────────────────────────────────────────────────────────────

    pub fn create(&self, req: CreateLayerRequest) -> Result<Layer, LayerError> {
        let layer_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let layer_type_str = req.layer_type.to_string();
        let style_str = req.style.as_ref().map(|v| v.to_string());
        let desc = req.description.unwrap_or_default();
        let visible = req.visible.unwrap_or(true);
        let opacity = req.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
        let z_index = req.z_index.unwrap_or(0);

        let c = self.conn.lock().map_err(|_| LayerError::Lock)?;
        c.execute(
            r#"INSERT INTO layers
               (layer_id, name, description, layer_type, source_url, data,
                visible, opacity, z_index, style, created_at, updated_at)
               VALUES (?,?,?,?,?,?,?,?,?,?,?,?)"#,
            params![
                layer_id,
                req.name,
                desc,
                layer_type_str,
                req.source_url,
                req.data,
                visible,
                opacity,
                z_index,
                style_str,
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )
        .map_err(LayerError::Db)?;

        Ok(Layer {
            layer_id,
            name: req.name,
            description: desc,
            layer_type: req.layer_type,
            source_url: req.source_url,
            data: req.data,
            visible,
            opacity,
            z_index,
            style: req.style,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn get(&self, layer_id: &str) -> Result<Option<Layer>, LayerError> {
        let c = self.conn.lock().map_err(|_| LayerError::Lock)?;
        let mut stmt = c
            .prepare(
                "SELECT layer_id, name, description, layer_type, source_url, data,
                         visible, opacity, z_index, style,
                         CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR)
                  FROM layers WHERE layer_id = ?",
            )
            .map_err(LayerError::Db)?;

        let mut rows = stmt.query(params![layer_id]).map_err(LayerError::Db)?;
        if let Some(row) = rows.next().map_err(LayerError::Db)? {
            Ok(Some(Self::row_to_layer(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_by_name(&self, name: &str) -> Result<Option<Layer>, LayerError> {
        let c = self.conn.lock().map_err(|_| LayerError::Lock)?;
        let mut stmt = c
            .prepare(
                "SELECT layer_id, name, description, layer_type, source_url, data,
                         visible, opacity, z_index, style,
                         CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR)
                  FROM layers WHERE name = ?",
            )
            .map_err(LayerError::Db)?;
        let mut rows = stmt.query(params![name]).map_err(LayerError::Db)?;
        if let Some(row) = rows.next().map_err(LayerError::Db)? {
            Ok(Some(Self::row_to_layer(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn list(&self) -> Result<Vec<Layer>, LayerError> {
        let c = self.conn.lock().map_err(|_| LayerError::Lock)?;
        let mut stmt = c
            .prepare(
                "SELECT layer_id, name, description, layer_type, source_url, data,
                         visible, opacity, z_index, style,
                         CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR)
                  FROM layers ORDER BY z_index ASC, name ASC",
            )
            .map_err(LayerError::Db)?;
        let rows = stmt
            .query_map([], Self::row_to_layer)
            .map_err(LayerError::Db)?;
        let mut layers = Vec::new();
        for r in rows {
            layers.push(r.map_err(LayerError::Db)?);
        }
        Ok(layers)
    }

    pub fn update(&self, layer_id: &str, req: UpdateLayerRequest) -> Result<Option<Layer>, LayerError> {
        let now = Utc::now();
        // Fetch existing first
        let existing = match self.get(layer_id)? {
            Some(l) => l,
            None => return Ok(None),
        };

        let name = req.name.unwrap_or(existing.name);
        let description = req.description.unwrap_or(existing.description);
        let source_url = req.source_url.or(existing.source_url);
        let data = req.data.or(existing.data);
        let visible = req.visible.unwrap_or(existing.visible);
        let opacity = req.opacity.unwrap_or(existing.opacity).clamp(0.0, 1.0);
        let z_index = req.z_index.unwrap_or(existing.z_index);
        let style = req.style.or(existing.style);
        let style_str = style.as_ref().map(|v| v.to_string());

        let c = self.conn.lock().map_err(|_| LayerError::Lock)?;
        let affected = c
            .execute(
                r#"UPDATE layers
                   SET name=?, description=?, source_url=?, data=?,
                       visible=?, opacity=?, z_index=?, style=?, updated_at=?
                   WHERE layer_id=?"#,
                params![
                    name,
                    description,
                    source_url,
                    data,
                    visible,
                    opacity,
                    z_index,
                    style_str,
                    now.to_rfc3339(),
                    layer_id,
                ],
            )
            .map_err(LayerError::Db)?;

        if affected == 0 {
            return Ok(None);
        }

        Ok(Some(Layer {
            layer_id: layer_id.to_string(),
            name,
            description,
            layer_type: existing.layer_type,
            source_url,
            data,
            visible,
            opacity,
            z_index,
            style,
            created_at: existing.created_at,
            updated_at: now,
        }))
    }

    pub fn delete(&self, layer_id: &str) -> Result<bool, LayerError> {
        let c = self.conn.lock().map_err(|_| LayerError::Lock)?;
        let affected = c
            .execute("DELETE FROM layers WHERE layer_id = ?", params![layer_id])
            .map_err(LayerError::Db)?;
        Ok(affected > 0)
    }

    // ── Row deserializer ──────────────────────────────────────────────────────

    fn row_to_layer(row: &duckdb::Row) -> duckdb::Result<Layer> {
        let layer_type_str: String = row.get(3)?;
        let layer_type = layer_type_str.parse::<LayerType>().unwrap_or(LayerType::GeoJson);
        let style_str: Option<String> = row.get(9)?;
        let style = style_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let created_str: String = row.get(10)?;
        let updated_str: String = row.get(11)?;
        let parse_ts = |s: String| -> DateTime<Utc> {
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .or_else(|_| {
                    chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S")
                        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f"))
                        .map(|n| n.and_utc())
                })
                .unwrap_or_else(|_| Utc::now())
        };
        Ok(Layer {
            layer_id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            layer_type,
            source_url: row.get(4)?,
            data: row.get(5)?,
            visible: row.get(6)?,
            opacity: row.get(7)?,
            z_index: row.get(8)?,
            style,
            created_at: parse_ts(created_str),
            updated_at: parse_ts(updated_str),
        })
    }

    // ── Built-in layers ───────────────────────────────────────────────────────

    /// Seed the database with built-in intelligence overlays.
    /// Safe to call multiple times — skips existing layers by name.
    pub fn seed_builtin_layers(&self) -> Result<Vec<String>, LayerError> {
        let mut seeded = Vec::new();
        for (name, builder) in BUILTIN_LAYERS {
            if self.get_by_name(name)?.is_some() {
                continue; // already exists
            }
            let req = builder();
            let layer = self.create(req)?;
            seeded.push(layer.name);
        }
        Ok(seeded)
    }
}

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum LayerError {
    #[error("DuckDB error: {0}")]
    Db(#[from] duckdb::Error),
    #[error("Lock poisoned")]
    Lock,
    #[error("Layer not found")]
    NotFound,
    #[error("Validation error: {0}")]
    Validation(String),
}

// ── Built-in layer data ───────────────────────────────────────────────────────

/// Major global shipping lanes as GeoJSON LineString features.
/// Covers: SLOC (Strait of Malacca), Suez Canal, Panama Canal, Dover Strait,
/// Bab el-Mandeb, Strait of Hormuz, and Trans-Pacific / Trans-Atlantic.
const SHIPPING_LANES_GEOJSON: &str = r#"{
  "type": "FeatureCollection",
  "features": [
    {
      "type": "Feature",
      "properties": { "name": "Strait of Malacca", "traffic": "very_high", "category": "chokepoint" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [98.67, 3.78], [100.35, 2.50], [103.82, 1.25], [104.20, 0.90]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Suez Canal", "traffic": "very_high", "category": "canal" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [32.58, 29.93], [32.57, 30.75], [32.55, 31.25], [32.32, 31.56],
          [32.18, 31.80], [32.10, 32.10]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Panama Canal", "traffic": "high", "category": "canal" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [-79.92, 8.87], [-79.60, 9.15], [-79.50, 9.38], [-79.53, 9.57]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Dover Strait / English Channel", "traffic": "very_high", "category": "chokepoint" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [-1.80, 49.80], [0.0, 50.50], [1.37, 51.00], [2.50, 51.20]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Strait of Hormuz", "traffic": "very_high", "category": "chokepoint" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [56.50, 24.47], [56.90, 24.55], [57.50, 25.00], [58.00, 25.50]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Bab el-Mandeb", "traffic": "high", "category": "chokepoint" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [42.80, 12.60], [43.30, 12.20], [43.60, 11.90]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Trans-Atlantic (North)", "traffic": "high", "category": "ocean_route" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [-9.14, 38.72], [-15.0, 40.0], [-30.0, 42.0], [-45.0, 43.0],
          [-60.0, 42.0], [-70.0, 40.0], [-74.0, 40.71]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Trans-Pacific (North)", "traffic": "high", "category": "ocean_route" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [121.47, 31.23], [135.0, 34.0], [150.0, 38.0], [165.0, 45.0],
          [180.0, 48.0], [-165.0, 52.0], [-150.0, 55.0], [-122.41, 47.61]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Cape of Good Hope Route", "traffic": "medium", "category": "ocean_route" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [18.43, -33.93], [20.0, -35.0], [25.0, -37.0], [30.0, -37.5],
          [35.0, -36.5], [40.0, -34.0], [43.0, -30.0]
        ]
      }
    },
    {
      "type": "Feature",
      "properties": { "name": "Strait of Gibraltar", "traffic": "very_high", "category": "chokepoint" },
      "geometry": {
        "type": "LineString",
        "coordinates": [
          [-5.60, 35.90], [-5.35, 36.00], [-5.00, 36.10], [-4.50, 36.20]
        ]
      }
    }
  ]
}"#;

/// ICAO airspace boundaries — key FIRs and SRAs represented as GeoJSON Polygons.
/// Includes: EUROCONTROL core ARAs, North Atlantic Tracks corridor,
/// Gulf/MIDANPIRG, Pacific Composite, and major busy FIRs.
const ICAO_AIRSPACE_GEOJSON: &str = r#"{
  "type": "FeatureCollection",
  "features": [
    {
      "type": "Feature",
      "properties": {
        "name": "North Atlantic Tracks (NAT) Corridor",
        "icao": "NATC",
        "class": "oceanic",
        "altitude_min_ft": 28500,
        "altitude_max_ft": 41000
      },
      "geometry": {
        "type": "Polygon",
        "coordinates": [[
          [-15.0, 45.0], [0.0, 50.0], [10.0, 55.0], [-10.0, 61.0],
          [-30.0, 62.0], [-50.0, 60.0], [-60.0, 55.0], [-60.0, 45.0],
          [-45.0, 42.0], [-30.0, 42.0], [-15.0, 45.0]
        ]]
      }
    },
    {
      "type": "Feature",
      "properties": {
        "name": "London FIR (EGTT)",
        "icao": "EGTT",
        "class": "fir",
        "authority": "NATS"
      },
      "geometry": {
        "type": "Polygon",
        "coordinates": [[
          [-8.0, 49.0], [2.5, 49.0], [5.0, 51.5], [3.0, 55.0],
          [-1.0, 57.0], [-8.0, 55.0], [-8.0, 49.0]
        ]]
      }
    },
    {
      "type": "Feature",
      "properties": {
        "name": "Paris FIR (LFFF)",
        "icao": "LFFF",
        "class": "fir",
        "authority": "DSNA"
      },
      "geometry": {
        "type": "Polygon",
        "coordinates": [[
          [-2.0, 43.0], [8.0, 43.5], [8.5, 47.5], [6.0, 50.0],
          [2.5, 50.5], [-2.0, 49.0], [-2.0, 43.0]
        ]]
      }
    },
    {
      "type": "Feature",
      "properties": {
        "name": "Dubai FIR (OMAE) — Gulf Region",
        "icao": "OMAE",
        "class": "fir",
        "authority": "UAE GCAA"
      },
      "geometry": {
        "type": "Polygon",
        "coordinates": [[
          [50.0, 22.0], [60.0, 22.0], [63.0, 25.5], [60.0, 27.0],
          [56.0, 27.5], [52.0, 26.5], [50.0, 24.0], [50.0, 22.0]
        ]]
      }
    },
    {
      "type": "Feature",
      "properties": {
        "name": "Singapore FIR (WSSS)",
        "icao": "WSSS",
        "class": "fir",
        "authority": "CAAS"
      },
      "geometry": {
        "type": "Polygon",
        "coordinates": [[
          [100.0, -1.0], [108.0, -1.0], [110.0, 3.0], [108.0, 7.0],
          [104.0, 8.0], [100.0, 5.5], [100.0, -1.0]
        ]]
      }
    },
    {
      "type": "Feature",
      "properties": {
        "name": "New York Oceanic FIR (KZWY)",
        "icao": "KZWY",
        "class": "oceanic"
      },
      "geometry": {
        "type": "Polygon",
        "coordinates": [[
          [-80.0, 30.0], [-60.0, 30.0], [-50.0, 40.0], [-40.0, 50.0],
          [-60.0, 55.0], [-80.0, 45.0], [-80.0, 30.0]
        ]]
      }
    }
  ]
}"#;

/// Lookup table: layer name → constructor function.
type BuiltinLayerBuilder = fn() -> CreateLayerRequest;

static BUILTIN_LAYERS: &[(&str, BuiltinLayerBuilder)] = &[
    ("shipping_lanes", || CreateLayerRequest {
        name: "shipping_lanes".to_string(),
        description: Some(
            "Major global shipping lanes and strategic chokepoints (Malacca, Suez, Panama, Hormuz, Dover, Gibraltar)".to_string(),
        ),
        layer_type: LayerType::GeoJson,
        source_url: None,
        data: Some(SHIPPING_LANES_GEOJSON.to_string()),
        visible: Some(true),
        opacity: Some(0.75),
        z_index: Some(10),
        style: Some(serde_json::json!({
            "line_color": "#0077ff",
            "line_width": 2,
            "line_dash": [6, 3]
        })),
    }),
    ("icao_airspace", || CreateLayerRequest {
        name: "icao_airspace".to_string(),
        description: Some(
            "ICAO FIR boundaries and airspace classifications — NAT, EUROCONTROL, Gulf, Singapore, New York Oceanic".to_string(),
        ),
        layer_type: LayerType::GeoJson,
        source_url: None,
        data: Some(ICAO_AIRSPACE_GEOJSON.to_string()),
        visible: Some(false),
        opacity: Some(0.40),
        z_index: Some(5),
        style: Some(serde_json::json!({
            "fill_color": "#8800ff",
            "fill_opacity": 0.1,
            "line_color": "#8800ff",
            "line_width": 1
        })),
    }),
    ("exclusion_zones", || CreateLayerRequest {
        name: "exclusion_zones".to_string(),
        description: Some("Maritime exclusion and danger zones — user-defined".to_string()),
        layer_type: LayerType::GeoJson,
        source_url: None,
        data: Some(r#"{"type":"FeatureCollection","features":[]}"#.to_string()),
        visible: Some(true),
        opacity: Some(0.60),
        z_index: Some(20),
        style: Some(serde_json::json!({
            "fill_color": "#ff0000",
            "fill_opacity": 0.2,
            "line_color": "#ff0000",
            "line_width": 2
        })),
    }),
    ("threat_zones", || CreateLayerRequest {
        name: "threat_zones".to_string(),
        description: Some("Dynamic threat zones from assessed entities — auto-populated by threat engine".to_string()),
        layer_type: LayerType::GeoJson,
        source_url: None,
        data: Some(r#"{"type":"FeatureCollection","features":[]}"#.to_string()),
        visible: Some(true),
        opacity: Some(0.70),
        z_index: Some(30),
        style: Some(serde_json::json!({
            "fill_color": "#ff6600",
            "fill_opacity": 0.3,
            "line_color": "#ff6600",
            "line_width": 2
        })),
    }),
    ("geofences", || CreateLayerRequest {
        name: "geofences".to_string(),
        description: Some("Operational geofences — alert when entities cross these boundaries".to_string()),
        layer_type: LayerType::GeoJson,
        source_url: None,
        data: Some(r#"{"type":"FeatureCollection","features":[]}"#.to_string()),
        visible: Some(true),
        opacity: Some(0.50),
        z_index: Some(15),
        style: Some(serde_json::json!({
            "fill_color": "#00cc44",
            "fill_opacity": 0.15,
            "line_color": "#00cc44",
            "line_width": 2,
            "line_dash": [4, 4]
        })),
    }),
];

// ── Axum AppState shim ────────────────────────────────────────────────────────
// The layer registry is plumbed into AppState via this wrapper. The HTTP router
// must have `layer_registry: Arc<LayerRegistry>` on its state.

/// Shared state wrapper for the layers subsystem.
#[derive(Clone)]
pub struct LayerState {
    pub registry: Arc<LayerRegistry>,
}

// ── Axum handlers ─────────────────────────────────────────────────────────────

/// GET /api/v1/layers
pub async fn list_layers(
    State(state): State<LayerState>,
) -> Result<Json<Vec<Layer>>, (StatusCode, Json<serde_json::Value>)> {
    state
        .registry
        .list()
        .map(Json)
        .map_err(|e| layer_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

/// GET /api/v1/layers/:id
pub async fn get_layer(
    State(state): State<LayerState>,
    Path(id): Path<String>,
) -> Result<Json<Layer>, (StatusCode, Json<serde_json::Value>)> {
    match state.registry.get(&id).map_err(|e| layer_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))? {
        Some(layer) => Ok(Json(layer)),
        None => Err(layer_error(StatusCode::NOT_FOUND, "Layer not found")),
    }
}

/// POST /api/v1/layers
pub async fn create_layer(
    State(state): State<LayerState>,
    Json(req): Json<CreateLayerRequest>,
) -> Result<(StatusCode, Json<Layer>), (StatusCode, Json<serde_json::Value>)> {
    // Validate: must have either source_url or data
    if req.source_url.is_none() && req.data.is_none() {
        return Err(layer_error(
            StatusCode::BAD_REQUEST,
            "Either source_url or data must be provided",
        ));
    }
    if req.name.trim().is_empty() {
        return Err(layer_error(StatusCode::BAD_REQUEST, "name cannot be empty"));
    }
    let layer = state
        .registry
        .create(req)
        .map_err(|e| layer_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok((StatusCode::CREATED, Json(layer)))
}

/// PUT /api/v1/layers/:id
pub async fn update_layer(
    State(state): State<LayerState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateLayerRequest>,
) -> Result<Json<Layer>, (StatusCode, Json<serde_json::Value>)> {
    match state
        .registry
        .update(&id, req)
        .map_err(|e| layer_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    {
        Some(layer) => Ok(Json(layer)),
        None => Err(layer_error(StatusCode::NOT_FOUND, "Layer not found")),
    }
}

/// DELETE /api/v1/layers/:id
pub async fn delete_layer(
    State(state): State<LayerState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let deleted = state
        .registry
        .delete(&id)
        .map_err(|e| layer_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(layer_error(StatusCode::NOT_FOUND, "Layer not found"))
    }
}

/// POST /api/v1/layers/seed
pub async fn seed_layers(
    State(state): State<LayerState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let seeded = state
        .registry
        .seed_builtin_layers()
        .map_err(|e| layer_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(serde_json::json!({
        "seeded": seeded,
        "count": seeded.len()
    })))
}

fn layer_error(status: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "error": { "message": msg, "status": status.as_u16() } })),
    )
}

/// Build the Axum router for the layers subsystem.
/// Mount this under `/api/v1` in the main router.
pub fn layers_router(registry: Arc<LayerRegistry>) -> axum::Router {
    use axum::routing::{get, post};
    let state = LayerState { registry };
    axum::Router::new()
        .route("/layers", get(list_layers).post(create_layer))
        .route("/layers/seed", post(seed_layers))
        .route("/layers/{id}", get(get_layer).put(update_layer).delete(delete_layer))
        .with_state(state)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::Connection;

    fn make_registry() -> LayerRegistry {
        let conn = Connection::open_in_memory().unwrap();
        let conn = Arc::new(Mutex::new(conn));
        LayerRegistry::new(conn).unwrap()
    }

    #[test]
    fn test_create_and_get_layer() {
        let reg = make_registry();
        let req = CreateLayerRequest {
            name: "test_layer".to_string(),
            description: Some("A test layer".to_string()),
            layer_type: LayerType::GeoJson,
            source_url: None,
            data: Some(r#"{"type":"FeatureCollection","features":[]}"#.to_string()),
            visible: Some(true),
            opacity: Some(0.8),
            z_index: Some(5),
            style: None,
        };
        let created = reg.create(req).unwrap();
        assert_eq!(created.name, "test_layer");
        assert_eq!(created.opacity, 0.8);

        let fetched = reg.get(&created.layer_id).unwrap().unwrap();
        assert_eq!(fetched.name, "test_layer");
        assert_eq!(fetched.layer_type, LayerType::GeoJson);
    }

    #[test]
    fn test_update_layer() {
        let reg = make_registry();
        let req = CreateLayerRequest {
            name: "to_update".to_string(),
            description: None,
            layer_type: LayerType::Wkt,
            source_url: None,
            data: Some("POINT(0 0)".to_string()),
            visible: Some(true),
            opacity: Some(1.0),
            z_index: Some(0),
            style: None,
        };
        let created = reg.create(req).unwrap();
        let upd = UpdateLayerRequest {
            name: None,
            description: Some("Updated desc".to_string()),
            source_url: None,
            data: None,
            visible: Some(false),
            opacity: Some(0.5),
            z_index: None,
            style: None,
        };
        let updated = reg.update(&created.layer_id, upd).unwrap().unwrap();
        assert_eq!(updated.description, "Updated desc");
        assert!(!updated.visible);
        assert_eq!(updated.opacity, 0.5);
    }

    #[test]
    fn test_delete_layer() {
        let reg = make_registry();
        let req = CreateLayerRequest {
            name: "to_delete".to_string(),
            description: None,
            layer_type: LayerType::GeoJson,
            source_url: None,
            data: Some("{}".to_string()),
            visible: Some(true),
            opacity: Some(1.0),
            z_index: Some(0),
            style: None,
        };
        let created = reg.create(req).unwrap();
        assert!(reg.delete(&created.layer_id).unwrap());
        assert!(reg.get(&created.layer_id).unwrap().is_none());
        assert!(!reg.delete(&created.layer_id).unwrap());
    }

    #[test]
    fn test_list_layers() {
        let reg = make_registry();
        for i in 0..3 {
            reg.create(CreateLayerRequest {
                name: format!("layer_{}", i),
                description: None,
                layer_type: LayerType::GeoJson,
                source_url: None,
                data: Some("{}".to_string()),
                visible: Some(true),
                opacity: Some(1.0),
                z_index: Some(i),
                style: None,
            })
            .unwrap();
        }
        let list = reg.list().unwrap();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_seed_builtin_layers() {
        let reg = make_registry();
        let seeded = reg.seed_builtin_layers().unwrap();
        assert_eq!(seeded.len(), BUILTIN_LAYERS.len());

        // Second call should seed nothing (idempotent)
        let seeded_again = reg.seed_builtin_layers().unwrap();
        assert_eq!(seeded_again.len(), 0);
    }

    #[test]
    fn test_shipping_lanes_geojson_is_valid() {
        let v: serde_json::Value = serde_json::from_str(SHIPPING_LANES_GEOJSON).unwrap();
        assert_eq!(v["type"], "FeatureCollection");
        let features = v["features"].as_array().unwrap();
        assert!(!features.is_empty());
        // Every feature has geometry.type == LineString
        for f in features {
            assert_eq!(f["geometry"]["type"], "LineString");
        }
    }

    #[test]
    fn test_icao_airspace_geojson_is_valid() {
        let v: serde_json::Value = serde_json::from_str(ICAO_AIRSPACE_GEOJSON).unwrap();
        assert_eq!(v["type"], "FeatureCollection");
        let features = v["features"].as_array().unwrap();
        assert!(!features.is_empty());
    }

    #[test]
    fn test_opacity_clamped() {
        let reg = make_registry();
        let req = CreateLayerRequest {
            name: "clamped".to_string(),
            description: None,
            layer_type: LayerType::GeoJson,
            source_url: None,
            data: Some("{}".to_string()),
            visible: Some(true),
            opacity: Some(1.5), // over maximum
            z_index: Some(0),
            style: None,
        };
        let created = reg.create(req).unwrap();
        assert_eq!(created.opacity, 1.0); // clamped
    }

    #[test]
    fn test_layer_type_roundtrip() {
        for (s, expected) in [("geojson", LayerType::GeoJson), ("wkt", LayerType::Wkt), ("tiles", LayerType::Tiles)] {
            let parsed: LayerType = s.parse().unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), s);
        }
    }
}
