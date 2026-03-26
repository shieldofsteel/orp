use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// GeoJSON parser (RFC 7946)
// ---------------------------------------------------------------------------
// GeoJSON is a JSON format for encoding geographic data structures.
//
// Top-level types:
//   - Feature: a spatially bounded entity with properties
//   - FeatureCollection: an array of Features
//   - Geometry: Point, MultiPoint, LineString, MultiLineString,
//               Polygon, MultiPolygon, GeometryCollection
//
// A Feature has:
//   - "type": "Feature"
//   - "geometry": { "type": "Point", "coordinates": [lon, lat, alt?] }
//   - "properties": { ... arbitrary key-values ... }
//   - "id": optional string or number

/// GeoJSON geometry type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeometryType {
    Point,
    MultiPoint,
    LineString,
    MultiLineString,
    Polygon,
    MultiPolygon,
    GeometryCollection,
}

impl GeometryType {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "Point" => Some(GeometryType::Point),
            "MultiPoint" => Some(GeometryType::MultiPoint),
            "LineString" => Some(GeometryType::LineString),
            "MultiLineString" => Some(GeometryType::MultiLineString),
            "Polygon" => Some(GeometryType::Polygon),
            "MultiPolygon" => Some(GeometryType::MultiPolygon),
            "GeometryCollection" => Some(GeometryType::GeometryCollection),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GeometryType::Point => "Point",
            GeometryType::MultiPoint => "MultiPoint",
            GeometryType::LineString => "LineString",
            GeometryType::MultiLineString => "MultiLineString",
            GeometryType::Polygon => "Polygon",
            GeometryType::MultiPolygon => "MultiPolygon",
            GeometryType::GeometryCollection => "GeometryCollection",
        }
    }
}

/// Parsed GeoJSON geometry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Geometry {
    pub geometry_type: GeometryType,
    /// Raw coordinate data (preserved from original JSON).
    pub coordinates: JsonValue,
}

/// Parsed GeoJSON Feature.
#[derive(Clone, Debug)]
pub struct GeoJsonFeature {
    pub id: Option<String>,
    pub geometry: Option<Geometry>,
    pub properties: HashMap<String, JsonValue>,
}

/// Parsed GeoJSON FeatureCollection.
#[derive(Clone, Debug)]
pub struct GeoJsonFeatureCollection {
    pub features: Vec<GeoJsonFeature>,
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse a GeoJSON Geometry object.
pub fn parse_geometry(value: &JsonValue) -> Option<Geometry> {
    let obj = value.as_object()?;
    let type_str = obj.get("type")?.as_str()?;
    let geometry_type = GeometryType::parse(type_str)?;

    if geometry_type == GeometryType::GeometryCollection {
        // GeometryCollection has "geometries" instead of "coordinates"
        let geometries = obj.get("geometries")?;
        return Some(Geometry {
            geometry_type,
            coordinates: geometries.clone(),
        });
    }

    let coordinates = obj.get("coordinates")?.clone();
    Some(Geometry {
        geometry_type,
        coordinates,
    })
}

/// Extract centroid (lat, lon) from a Geometry.
/// For Point: direct coordinates.
/// For others: average of all coordinate points (simple centroid).
pub fn geometry_centroid(geom: &Geometry) -> Option<(f64, f64)> {
    match geom.geometry_type {
        GeometryType::Point => {
            let coords = geom.coordinates.as_array()?;
            let lon = coords.first()?.as_f64()?;
            let lat = coords.get(1)?.as_f64()?;
            Some((lat, lon))
        }
        GeometryType::MultiPoint | GeometryType::LineString => {
            average_coordinates(&geom.coordinates)
        }
        GeometryType::MultiLineString | GeometryType::Polygon => {
            // coordinates is array of arrays of [lon, lat]
            let rings = geom.coordinates.as_array()?;
            let mut all_coords = Vec::new();
            for ring in rings {
                if let Some(pts) = ring.as_array() {
                    all_coords.extend(pts.iter().cloned());
                }
            }
            average_coordinates(&json!(all_coords))
        }
        GeometryType::MultiPolygon => {
            let polygons = geom.coordinates.as_array()?;
            let mut all_coords = Vec::new();
            for polygon in polygons {
                if let Some(rings) = polygon.as_array() {
                    for ring in rings {
                        if let Some(pts) = ring.as_array() {
                            all_coords.extend(pts.iter().cloned());
                        }
                    }
                }
            }
            average_coordinates(&json!(all_coords))
        }
        GeometryType::GeometryCollection => None,
    }
}

/// Average [lon, lat] coordinate pairs.
fn average_coordinates(coords: &JsonValue) -> Option<(f64, f64)> {
    let arr = coords.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut sum_lat = 0.0;
    let mut sum_lon = 0.0;
    let mut count = 0;

    for coord in arr {
        let c = coord.as_array()?;
        let lon = c.first()?.as_f64()?;
        let lat = c.get(1)?.as_f64()?;
        sum_lon += lon;
        sum_lat += lat;
        count += 1;
    }

    if count == 0 {
        return None;
    }
    Some((sum_lat / count as f64, sum_lon / count as f64))
}

/// Parse a GeoJSON Feature.
pub fn parse_feature(value: &JsonValue) -> Option<GeoJsonFeature> {
    let obj = value.as_object()?;
    let type_str = obj.get("type")?.as_str()?;
    if type_str != "Feature" {
        return None;
    }

    let geometry = obj.get("geometry").and_then(|g| {
        if g.is_null() {
            None
        } else {
            parse_geometry(g)
        }
    });

    let properties = obj
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|p| p.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    let id = obj.get("id").map(|v| match v {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        _ => v.to_string(),
    });

    Some(GeoJsonFeature {
        id,
        geometry,
        properties,
    })
}

/// Parse a GeoJSON FeatureCollection.
pub fn parse_feature_collection(value: &JsonValue) -> Result<GeoJsonFeatureCollection, ConnectorError> {
    let obj = value.as_object().ok_or_else(|| {
        ConnectorError::ParseError("GeoJSON: expected JSON object".into())
    })?;

    let type_str = obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match type_str {
        "FeatureCollection" => {
            let features_val = obj.get("features").ok_or_else(|| {
                ConnectorError::ParseError("GeoJSON: FeatureCollection missing 'features' array".into())
            })?;
            let features_arr = features_val.as_array().ok_or_else(|| {
                ConnectorError::ParseError("GeoJSON: 'features' is not an array".into())
            })?;

            let features: Vec<GeoJsonFeature> = features_arr
                .iter()
                .filter_map(parse_feature)
                .collect();

            Ok(GeoJsonFeatureCollection { features })
        }
        "Feature" => {
            // Single Feature — wrap in a FeatureCollection
            let feature = parse_feature(value).ok_or_else(|| {
                ConnectorError::ParseError("GeoJSON: failed to parse Feature".into())
            })?;
            Ok(GeoJsonFeatureCollection {
                features: vec![feature],
            })
        }
        _ => {
            // Bare geometry — wrap as feature with no properties
            if let Some(geom) = parse_geometry(value) {
                let feature = GeoJsonFeature {
                    id: None,
                    geometry: Some(geom),
                    properties: HashMap::new(),
                };
                Ok(GeoJsonFeatureCollection {
                    features: vec![feature],
                })
            } else {
                Err(ConnectorError::ParseError(format!(
                    "GeoJSON: unsupported type '{}'",
                    type_str
                )))
            }
        }
    }
}

/// Parse a GeoJSON string.
pub fn parse_geojson(data: &str) -> Result<GeoJsonFeatureCollection, ConnectorError> {
    let value: JsonValue = serde_json::from_str(data).map_err(|e| {
        ConnectorError::ParseError(format!("GeoJSON: invalid JSON: {}", e))
    })?;
    parse_feature_collection(&value)
}

// ---------------------------------------------------------------------------
// Feature → SourceEvent conversion
// ---------------------------------------------------------------------------

/// Convert a GeoJsonFeature into a SourceEvent.
pub fn feature_to_source_event(
    feature: &GeoJsonFeature,
    index: usize,
    connector_id: &str,
) -> SourceEvent {
    let entity_id = feature
        .id
        .clone()
        .unwrap_or_else(|| format!("geojson:feature-{}", index));

    let (latitude, longitude) = feature
        .geometry
        .as_ref()
        .and_then(geometry_centroid)
        .unwrap_or((0.0, 0.0));

    let mut properties = feature.properties.clone();

    if let Some(ref geom) = feature.geometry {
        properties.insert("geometry_type".into(), json!(geom.geometry_type.as_str()));
        properties.insert("geometry".into(), json!({
            "type": geom.geometry_type.as_str(),
            "coordinates": geom.coordinates,
        }));
    }

    // Determine entity type from properties or default
    let entity_type = feature
        .properties
        .get("entity_type")
        .and_then(|v| v.as_str())
        .unwrap_or("feature")
        .to_string();

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type,
        properties,
        timestamp: Utc::now(),
        latitude: Some(latitude),
        longitude: Some(longitude),
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct GeoJsonConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl GeoJsonConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_processed: Arc::new(AtomicU64::new(0)),
            errors: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl Connector for GeoJsonConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        let path = self
            .config
            .url
            .as_deref()
            .ok_or_else(|| {
                ConnectorError::ConfigError("GeoJSON: url (file path or URL) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        let collection = parse_geojson(&content).inspect_err(|_e| {
            errors.fetch_add(1, Ordering::Relaxed);
        })?;

        for (i, feature) in collection.features.iter().enumerate() {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let event = feature_to_source_event(feature, i, &connector_id);
            if tx.send(event).await.is_err() {
                break;
            }
            events_processed.fetch_add(1, Ordering::Relaxed);
        }

        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ConnectorError::ConnectionError(
                "GeoJSON connector is not running".into(),
            ));
        }
        Ok(())
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        ConnectorStats {
            events_processed: self.events_processed.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            last_event_timestamp: None,
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

    #[test]
    fn test_parse_point_geometry() {
        let val = json!({"type": "Point", "coordinates": [-73.9857, 40.7484]});
        let geom = parse_geometry(&val).unwrap();
        assert_eq!(geom.geometry_type, GeometryType::Point);
        let (lat, lon) = geometry_centroid(&geom).unwrap();
        assert!((lat - 40.7484).abs() < 0.0001);
        assert!((lon - (-73.9857)).abs() < 0.0001);
    }

    #[test]
    fn test_parse_linestring_geometry() {
        let val = json!({
            "type": "LineString",
            "coordinates": [[0.0, 0.0], [10.0, 10.0], [20.0, 20.0]]
        });
        let geom = parse_geometry(&val).unwrap();
        assert_eq!(geom.geometry_type, GeometryType::LineString);
        let (lat, lon) = geometry_centroid(&geom).unwrap();
        assert!((lat - 10.0).abs() < 0.0001);
        assert!((lon - 10.0).abs() < 0.0001);
    }

    #[test]
    fn test_parse_polygon_geometry() {
        let val = json!({
            "type": "Polygon",
            "coordinates": [[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]]]
        });
        let geom = parse_geometry(&val).unwrap();
        assert_eq!(geom.geometry_type, GeometryType::Polygon);
        let (lat, lon) = geometry_centroid(&geom).unwrap();
        assert!((lat - 4.0).abs() < 0.0001); // avg of 0,0,10,10,0 = 4
        assert!((lon - 4.0).abs() < 0.0001);
    }

    #[test]
    fn test_parse_feature() {
        let val = json!({
            "type": "Feature",
            "id": "building-42",
            "geometry": {
                "type": "Point",
                "coordinates": [-73.9857, 40.7484]
            },
            "properties": {
                "name": "Empire State Building",
                "height_m": 443
            }
        });
        let feature = parse_feature(&val).unwrap();
        assert_eq!(feature.id, Some("building-42".into()));
        assert!(feature.geometry.is_some());
        assert_eq!(feature.properties["name"], json!("Empire State Building"));
        assert_eq!(feature.properties["height_m"], json!(443));
    }

    #[test]
    fn test_parse_feature_collection() {
        let val = json!({
            "type": "FeatureCollection",
            "features": [
                {
                    "type": "Feature",
                    "id": "1",
                    "geometry": {"type": "Point", "coordinates": [0.0, 0.0]},
                    "properties": {"name": "Origin"}
                },
                {
                    "type": "Feature",
                    "id": "2",
                    "geometry": {"type": "Point", "coordinates": [1.0, 1.0]},
                    "properties": {"name": "Nearby"}
                }
            ]
        });
        let collection = parse_feature_collection(&val).unwrap();
        assert_eq!(collection.features.len(), 2);
        assert_eq!(collection.features[0].properties["name"], json!("Origin"));
    }

    #[test]
    fn test_parse_single_feature_as_collection() {
        let val = json!({
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [10.0, 20.0]},
            "properties": {"key": "value"}
        });
        let collection = parse_feature_collection(&val).unwrap();
        assert_eq!(collection.features.len(), 1);
    }

    #[test]
    fn test_parse_bare_geometry_as_collection() {
        let val = json!({"type": "Point", "coordinates": [5.0, 15.0]});
        let collection = parse_feature_collection(&val).unwrap();
        assert_eq!(collection.features.len(), 1);
        assert!(collection.features[0].geometry.is_some());
    }

    #[test]
    fn test_feature_to_source_event() {
        let feature = GeoJsonFeature {
            id: Some("poi-1".into()),
            geometry: Some(Geometry {
                geometry_type: GeometryType::Point,
                coordinates: json!([-122.4194, 37.7749]),
            }),
            properties: {
                let mut m = HashMap::new();
                m.insert("name".into(), json!("San Francisco"));
                m
            },
        };
        let event = feature_to_source_event(&feature, 0, "geojson-test");
        assert_eq!(event.entity_id, "poi-1");
        assert!((event.latitude.unwrap() - 37.7749).abs() < 0.0001);
        assert!((event.longitude.unwrap() - (-122.4194)).abs() < 0.0001);
        assert_eq!(event.properties["name"], json!("San Francisco"));
        assert_eq!(event.properties["geometry_type"], json!("Point"));
    }

    #[test]
    fn test_feature_null_geometry() {
        let val = json!({
            "type": "Feature",
            "geometry": null,
            "properties": {"name": "No location"}
        });
        let feature = parse_feature(&val).unwrap();
        assert!(feature.geometry.is_none());
    }

    #[test]
    fn test_multipolygon_centroid() {
        let val = json!({
            "type": "MultiPolygon",
            "coordinates": [
                [[[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [0.0, 0.0]]],
                [[[10.0, 10.0], [12.0, 10.0], [12.0, 12.0], [10.0, 12.0], [10.0, 10.0]]]
            ]
        });
        let geom = parse_geometry(&val).unwrap();
        assert_eq!(geom.geometry_type, GeometryType::MultiPolygon);
        let (lat, lon) = geometry_centroid(&geom).unwrap();
        // Average of all points across both polygons
        assert!(lat > 0.0 && lat < 12.0);
        assert!(lon > 0.0 && lon < 12.0);
    }

    #[test]
    fn test_geojson_connector_id() {
        let config = ConnectorConfig {
            connector_id: "geojson-1".to_string(),
            connector_type: "geojson".to_string(),
            url: None,
            entity_type: "feature".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = GeoJsonConnector::new(config);
        assert_eq!(connector.connector_id(), "geojson-1");
    }

    #[tokio::test]
    async fn test_geojson_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "geojson-h".to_string(),
            connector_type: "geojson".to_string(),
            url: None,
            entity_type: "feature".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = GeoJsonConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_parse_geojson_string() {
        let data = r#"{
            "type": "FeatureCollection",
            "features": [
                {
                    "type": "Feature",
                    "geometry": {"type": "Point", "coordinates": [1.0, 2.0]},
                    "properties": {"key": "val"}
                }
            ]
        }"#;
        let collection = parse_geojson(data).unwrap();
        assert_eq!(collection.features.len(), 1);
    }

    #[test]
    fn test_parse_invalid_geojson() {
        assert!(parse_geojson("{invalid json}").is_err());
    }

    #[test]
    fn test_geometry_collection() {
        let val = json!({
            "type": "GeometryCollection",
            "geometries": [
                {"type": "Point", "coordinates": [0.0, 0.0]},
                {"type": "LineString", "coordinates": [[1.0, 1.0], [2.0, 2.0]]}
            ]
        });
        let geom = parse_geometry(&val).unwrap();
        assert_eq!(geom.geometry_type, GeometryType::GeometryCollection);
    }

    #[test]
    fn test_numeric_feature_id() {
        let val = json!({
            "type": "Feature",
            "id": 42,
            "geometry": {"type": "Point", "coordinates": [0.0, 0.0]},
            "properties": {}
        });
        let feature = parse_feature(&val).unwrap();
        assert_eq!(feature.id, Some("42".into()));
    }
}
