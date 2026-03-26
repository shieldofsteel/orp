//! Universal Ingest — Accept ANY JSON payload and auto-detect its entity type.
//!
//! # Endpoints
//! - `POST /api/v1/ingest`        — ingest a single JSON object
//! - `POST /api/v1/ingest/batch`  — ingest an array of JSON objects
//!
//! # Auto-detection rules (evaluated in priority order)
//! | Condition                              | Assigned type |
//! |----------------------------------------|---------------|
//! | `mmsi` or `imo` present                | `ship`        |
//! | `icao` or (`callsign` + `altitude`)    | `aircraft`    |
//! | `ip` or `hostname`                     | `host`        |
//! | `cve` or `vulnerability`               | `threat`      |
//! | `temperature` or `humidity`            | `sensor`      |
//! | `plate` or `vin`                       | `vehicle`     |
//! | `lat` + `lon` only                     | `point`       |
//! | fallback                               | `generic`     |
//!
//! # Entity ID generation
//! A deterministic ID is derived from the payload's identifying fields so that
//! re-ingesting the same physical entity updates rather than duplicates it.
//!
//! # Nested JSON flattening
//! Nested objects are flattened to dot-notation keys, e.g.
//! `{"engine": {"rpm": 3000}}` → `{"engine.rpm": 3000}`.

use crate::server::http::AppState;
use crate::server::websocket::BroadcastEvent;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use orp_proto::{Entity, GeoPoint};
use orp_security::middleware::AuthContext;
use orp_security::{AbacEngine, EvaluationContext, EvaluationResult, Resource, Subject};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

// ── Error helper ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: String,
    status: u16,
    message: String,
    request_id: String,
    timestamp: String,
}

fn error_response(
    code: &str,
    status: StatusCode,
    message: &str,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: ErrorBody {
                code: code.to_string(),
                status: status.as_u16(),
                message: message.to_string(),
                request_id: uuid::Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
        }),
    )
}

// ── ABAC helper ───────────────────────────────────────────────────────────────

fn check_abac(
    abac: &AbacEngine,
    auth: &AuthContext,
    action: &str,
    resource_type: &str,
    resource_id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let ctx = EvaluationContext {
        subject: Subject {
            sub: auth.subject.clone(),
            permissions: auth.permissions.clone(),
            role: if auth.has_permission("admin") {
                Some("admin".to_string())
            } else {
                None
            },
            org_id: auth.org_id.clone(),
            attributes: HashMap::new(),
        },
        action: action.to_string(),
        resource: Resource {
            r#type: resource_type.to_string(),
            id: resource_id.to_string(),
            attributes: HashMap::new(),
        },
    };
    let decision = abac.evaluate(&ctx);
    if decision.result == EvaluationResult::Deny {
        return Err(error_response(
            "FORBIDDEN",
            StatusCode::FORBIDDEN,
            &format!("Access denied: {}", decision.reason),
        ));
    }
    Ok(())
}

// ── Auto-detection ────────────────────────────────────────────────────────────

/// Determine the ORP entity type from a flattened property map.
pub fn detect_entity_type(props: &HashMap<String, serde_json::Value>) -> &'static str {
    let has = |key: &str| props.contains_key(key);

    if has("mmsi") || has("imo") {
        return "ship";
    }
    if has("icao") || (has("callsign") && has("altitude")) {
        return "aircraft";
    }
    if has("ip") || has("hostname") {
        return "host";
    }
    if has("cve") || has("vulnerability") {
        return "threat";
    }
    if has("temperature") || has("humidity") {
        return "sensor";
    }
    if has("plate") || has("vin") {
        return "vehicle";
    }
    if has("lat") && has("lon") {
        return "point";
    }
    "generic"
}

/// Generate a deterministic entity_id from identifying fields in the payload.
/// Falls back to a random UUID if no identifying field is present.
pub fn generate_entity_id(entity_type: &str, props: &HashMap<String, serde_json::Value>) -> String {
    let key_fields: &[&str] = match entity_type {
        "ship" => &["mmsi", "imo"],
        "aircraft" => &["icao", "callsign"],
        "host" => &["ip", "hostname"],
        "threat" => &["cve"],
        "sensor" => &["sensor_id", "device_id", "id"],
        "vehicle" => &["vin", "plate"],
        "point" => &["lat", "lon"],
        _ => &["id", "entity_id"],
    };

    for field in key_fields {
        if let Some(val) = props.get(*field) {
            let raw = match val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if !raw.is_empty() && raw != "null" {
                return format!("{}-{}", entity_type, raw.replace(' ', "_"));
            }
        }
    }

    // Fallback: deterministic UUID from all key-value pairs sorted
    let mut parts: Vec<String> = props
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    parts.sort();
    let combined = parts.join(";");

    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    combined.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{}-{:016x}", entity_type, hash)
}

/// Flatten a nested JSON object into dot-notation key → value pairs.
///
/// ```
/// # use std::collections::HashMap;
/// # use orp_core::server::ingest::flatten_json;
/// let raw = serde_json::json!({"engine": {"rpm": 3000}, "name": "foo"});
/// let flat = flatten_json(&raw, "");
/// assert_eq!(flat["engine.rpm"], serde_json::json!(3000));
/// assert_eq!(flat["name"], serde_json::json!("foo"));
/// ```
pub fn flatten_json(
    value: &serde_json::Value,
    prefix: &str,
) -> HashMap<String, serde_json::Value> {
    let mut result = HashMap::new();
    flatten_recursive(value, prefix, &mut result);
    result
}

fn flatten_recursive(
    value: &serde_json::Value,
    prefix: &str,
    out: &mut HashMap<String, serde_json::Value>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                let new_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                flatten_recursive(val, &new_key, out);
            }
        }
        // Arrays: store as-is (don't expand array elements)
        other => {
            out.insert(prefix.to_string(), other.clone());
        }
    }
}

/// Parse lat/lon from a flattened property map.
/// Handles both numeric and string representations.
fn extract_geometry(props: &HashMap<String, serde_json::Value>) -> Option<GeoPoint> {
    let lat = props
        .get("lat")
        .or_else(|| props.get("latitude"))
        .and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        })?;

    let lon = props
        .get("lon")
        .or_else(|| props.get("longitude"))
        .and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        })?;

    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }

    let alt = props
        .get("alt")
        .or_else(|| props.get("altitude"))
        .and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        });

    Some(GeoPoint { lat, lon, alt })
}

/// Parse the `confidence` field if present.
fn extract_confidence(props: &HashMap<String, serde_json::Value>) -> f64 {
    props
        .get("confidence")
        .and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        })
        .map(|c| c.clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

/// Parse the `name` field.
fn extract_name(props: &HashMap<String, serde_json::Value>) -> Option<String> {
    props
        .get("name")
        .or_else(|| props.get("vessel_name"))
        .or_else(|| props.get("aircraft_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// ── Core processing ───────────────────────────────────────────────────────────

/// Response shape for a single ingested entity.
#[derive(Serialize)]
pub struct IngestedEntity {
    pub id: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub name: Option<String>,
    pub properties: HashMap<String, serde_json::Value>,
    pub geometry: Option<serde_json::Value>,
    pub confidence: f64,
    pub created: bool, // true = newly created, false = updated
}

/// Process a single raw JSON payload: detect type, flatten, upsert, return.
async fn process_payload(
    payload: serde_json::Value,
    state: &AppState,
    subject: &str,
) -> Result<IngestedEntity, String> {
    if !payload.is_object() {
        return Err("Payload must be a JSON object".to_string());
    }

    // Flatten nested structure
    let props = flatten_json(&payload, "");

    // Detect entity type
    let entity_type = detect_entity_type(&props);

    // Generate or extract entity_id
    let entity_id = generate_entity_id(entity_type, &props);

    // Extract optional fields
    let geometry = extract_geometry(&props);
    let confidence = extract_confidence(&props);
    let name = extract_name(&props);

    // Check if entity already exists to determine create vs update
    let existed = state
        .storage
        .get_entity(&entity_id)
        .await
        .map(|opt| opt.is_some())
        .unwrap_or(false);

    let entity = Entity {
        entity_id: entity_id.clone(),
        entity_type: entity_type.to_string(),
        name: name.clone(),
        properties: props.clone(),
        geometry: geometry.clone(),
        confidence,
        canonical_id: None,
        is_active: true,
        created_at: if existed {
            state
                .storage
                .get_entity(&entity_id)
                .await
                .ok()
                .flatten()
                .map(|e| e.created_at)
                .unwrap_or_else(chrono::Utc::now)
        } else {
            chrono::Utc::now()
        },
        last_updated: chrono::Utc::now(),
    };

    state
        .storage
        .insert_entity(&entity)
        .await
        .map_err(|e| format!("Storage error: {}", e))?;

    // Audit log
    if let Err(e) = state
        .storage
        .log_audit(
            if existed { "entity_updated_via_ingest" } else { "entity_created_via_ingest" },
            Some(entity_type),
            Some(&entity_id),
            Some(subject),
            serde_json::json!({
                "entity_id": entity_id,
                "entity_type": entity_type,
                "method": "universal_ingest",
            }),
        )
        .await
    {
        tracing::warn!("Ingest audit log failed: {}", e);
    }

    // Broadcast WebSocket event
    if existed {
        let _ = state.broadcast_tx.send(BroadcastEvent::EntityUpdate {
            entity_id: entity_id.clone(),
            entity_type: entity_type.to_string(),
            changes: serde_json::to_value(&props).unwrap_or_default(),
            geometry: geometry.as_ref().map(|g| {
                serde_json::json!({"type": "Point", "coordinates": [g.lon, g.lat]})
            }),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });
    } else {
        let _ = state.broadcast_tx.send(BroadcastEvent::EntityCreated {
            entity_id: entity_id.clone(),
            entity_type: entity_type.to_string(),
            entity_name: name.clone(),
            properties: serde_json::to_value(&props).unwrap_or_default(),
            geometry: geometry.as_ref().map(|g| {
                serde_json::json!({"type": "Point", "coordinates": [g.lon, g.lat]})
            }),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });
    }

    Ok(IngestedEntity {
        id: entity_id,
        entity_type: entity_type.to_string(),
        name,
        properties: props,
        geometry: geometry.map(|g| {
            serde_json::json!({"type": "Point", "coordinates": [g.lon, g.lat]})
        }),
        confidence,
        created: !existed,
    })
}

// ── HTTP handlers ─────────────────────────────────────────────────────────────

/// `POST /api/v1/ingest` — accept any JSON payload, auto-detect type, upsert.
pub async fn ingest_single(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:write", "entity", "*") {
        return resp.into_response();
    }

    match process_payload(payload, &state, &auth.subject).await {
        Ok(entity) => {
            let status = if entity.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(entity)).into_response()
        }
        Err(e) => error_response("INGEST_ERROR", StatusCode::BAD_REQUEST, &e).into_response(),
    }
}

/// `POST /api/v1/ingest/batch` — accept an array of JSON objects.
///
/// Processes each item independently; partial failures are reported in the
/// `errors` array without aborting the rest of the batch.
pub async fn ingest_batch(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:write", "entity", "*") {
        return resp.into_response();
    }

    let items = match payload.as_array() {
        Some(arr) => arr.clone(),
        None => {
            return error_response(
                "VALIDATION_ERROR",
                StatusCode::BAD_REQUEST,
                "Batch payload must be a JSON array",
            )
            .into_response()
        }
    };

    if items.is_empty() {
        return error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            "Batch array cannot be empty",
        )
        .into_response();
    }

    const MAX_BATCH: usize = 1000;
    if items.len() > MAX_BATCH {
        return error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            &format!("Batch size exceeds maximum of {}", MAX_BATCH),
        )
        .into_response();
    }

    let mut created = Vec::new();
    let mut updated = Vec::new();
    let mut errors: Vec<serde_json::Value> = Vec::new();

    for (idx, item) in items.into_iter().enumerate() {
        match process_payload(item, &state, &auth.subject).await {
            Ok(entity) => {
                if entity.created {
                    created.push(entity);
                } else {
                    updated.push(entity);
                }
            }
            Err(e) => {
                errors.push(serde_json::json!({
                    "index": idx,
                    "error": e,
                }));
            }
        }
    }

    let total = created.len() + updated.len();
    let status = if errors.is_empty() {
        StatusCode::OK
    } else if total == 0 {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::MULTI_STATUS
    };

    (
        status,
        Json(serde_json::json!({
            "created": created,
            "updated": updated,
            "errors": errors,
            "summary": {
                "total_processed": total,
                "created_count": created.len(),
                "updated_count": updated.len(),
                "error_count": errors.len(),
            }
        })),
    )
        .into_response()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn props(json: serde_json::Value) -> HashMap<String, serde_json::Value> {
        json.as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    // ── detect_entity_type ───────────────────────────────────────────────────

    #[test]
    fn test_detect_ship_by_mmsi() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"mmsi": "123456789"}))),
            "ship"
        );
    }

    #[test]
    fn test_detect_ship_by_imo() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"imo": "IMO1234567"}))),
            "ship"
        );
    }

    #[test]
    fn test_detect_aircraft_by_icao() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"icao": "A12345"}))),
            "aircraft"
        );
    }

    #[test]
    fn test_detect_aircraft_by_callsign_and_altitude() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({
                "callsign": "SWA1234",
                "altitude": 35000
            }))),
            "aircraft"
        );
    }

    #[test]
    fn test_detect_host_by_ip() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"ip": "192.168.1.1"}))),
            "host"
        );
    }

    #[test]
    fn test_detect_host_by_hostname() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"hostname": "server-01"}))),
            "host"
        );
    }

    #[test]
    fn test_detect_threat_by_cve() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"cve": "CVE-2023-1234"}))),
            "threat"
        );
    }

    #[test]
    fn test_detect_threat_by_vulnerability() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"vulnerability": "RCE in OpenSSL"}))),
            "threat"
        );
    }

    #[test]
    fn test_detect_sensor_by_temperature() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"temperature": 22.5}))),
            "sensor"
        );
    }

    #[test]
    fn test_detect_sensor_by_humidity() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"humidity": 65.0}))),
            "sensor"
        );
    }

    #[test]
    fn test_detect_vehicle_by_plate() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"plate": "ABC1234"}))),
            "vehicle"
        );
    }

    #[test]
    fn test_detect_vehicle_by_vin() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"vin": "1HGCM82633A004352"}))),
            "vehicle"
        );
    }

    #[test]
    fn test_detect_point_by_lat_lon() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({
                "lat": 51.92,
                "lon": 4.47,
            }))),
            "point"
        );
    }

    #[test]
    fn test_detect_generic_fallback() {
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({"some_field": "some_value"}))),
            "generic"
        );
    }

    #[test]
    fn test_detect_ship_takes_priority_over_lat_lon() {
        // mmsi + lat/lon → ship (ship detection is checked first)
        assert_eq!(
            detect_entity_type(&props(serde_json::json!({
                "mmsi": "123456789",
                "lat": 51.92,
                "lon": 4.47,
            }))),
            "ship"
        );
    }

    // ── flatten_json ─────────────────────────────────────────────────────────

    #[test]
    fn test_flatten_simple() {
        let raw = serde_json::json!({"a": 1, "b": "hello"});
        let flat = flatten_json(&raw, "");
        assert_eq!(flat["a"], serde_json::json!(1));
        assert_eq!(flat["b"], serde_json::json!("hello"));
    }

    #[test]
    fn test_flatten_nested() {
        let raw = serde_json::json!({"engine": {"rpm": 3000, "temp": 80}});
        let flat = flatten_json(&raw, "");
        assert_eq!(flat["engine.rpm"], serde_json::json!(3000));
        assert_eq!(flat["engine.temp"], serde_json::json!(80));
    }

    #[test]
    fn test_flatten_deeply_nested() {
        let raw = serde_json::json!({"a": {"b": {"c": 42}}});
        let flat = flatten_json(&raw, "");
        assert_eq!(flat["a.b.c"], serde_json::json!(42));
    }

    #[test]
    fn test_flatten_array_kept_as_is() {
        let raw = serde_json::json!({"tags": [1, 2, 3]});
        let flat = flatten_json(&raw, "");
        assert_eq!(flat["tags"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_flatten_empty_object() {
        let raw = serde_json::json!({});
        let flat = flatten_json(&raw, "");
        assert!(flat.is_empty());
    }

    // ── generate_entity_id ───────────────────────────────────────────────────

    #[test]
    fn test_generate_id_ship_mmsi() {
        let p = props(serde_json::json!({"mmsi": "123456789"}));
        assert_eq!(generate_entity_id("ship", &p), "ship-123456789");
    }

    #[test]
    fn test_generate_id_aircraft_icao() {
        let p = props(serde_json::json!({"icao": "ABC123"}));
        assert_eq!(generate_entity_id("aircraft", &p), "aircraft-ABC123");
    }

    #[test]
    fn test_generate_id_vehicle_vin() {
        let p = props(serde_json::json!({"vin": "1HGCM82633A004352"}));
        assert_eq!(generate_entity_id("vehicle", &p), "vehicle-1HGCM82633A004352");
    }

    #[test]
    fn test_generate_id_fallback_deterministic() {
        let p = props(serde_json::json!({"unknown_key": "value"}));
        let id1 = generate_entity_id("generic", &p);
        let id2 = generate_entity_id("generic", &p);
        assert_eq!(id1, id2, "Fallback IDs must be deterministic");
        assert!(id1.starts_with("generic-"));
    }

    // ── extract_geometry ─────────────────────────────────────────────────────

    #[test]
    fn test_extract_geometry_valid() {
        let p = props(serde_json::json!({"lat": 51.92, "lon": 4.47}));
        let geo = extract_geometry(&p).unwrap();
        assert!((geo.lat - 51.92).abs() < 1e-9);
        assert!((geo.lon - 4.47).abs() < 1e-9);
        assert!(geo.alt.is_none());
    }

    #[test]
    fn test_extract_geometry_with_altitude() {
        let p = props(serde_json::json!({"lat": 51.92, "lon": 4.47, "altitude": 35000.0}));
        let geo = extract_geometry(&p).unwrap();
        assert_eq!(geo.alt, Some(35000.0));
    }

    #[test]
    fn test_extract_geometry_invalid_lat() {
        let p = props(serde_json::json!({"lat": 999.0, "lon": 4.47}));
        assert!(extract_geometry(&p).is_none());
    }

    #[test]
    fn test_extract_geometry_missing_lon() {
        let p = props(serde_json::json!({"lat": 51.92}));
        assert!(extract_geometry(&p).is_none());
    }

    #[test]
    fn test_extract_geometry_string_values() {
        let p = props(serde_json::json!({"lat": "51.92", "lon": "4.47"}));
        let geo = extract_geometry(&p).unwrap();
        assert!((geo.lat - 51.92).abs() < 1e-6);
    }

    // ── extract_confidence ───────────────────────────────────────────────────

    #[test]
    fn test_confidence_present() {
        let p = props(serde_json::json!({"confidence": 0.85}));
        assert!((extract_confidence(&p) - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_confidence_clamped_above_1() {
        let p = props(serde_json::json!({"confidence": 1.5}));
        assert!((extract_confidence(&p) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_confidence_defaults_to_1() {
        let p = props(serde_json::json!({}));
        assert!((extract_confidence(&p) - 1.0).abs() < f64::EPSILON);
    }

    // ── Integration: process full payload shapes ──────────────────────────────

    #[test]
    fn test_ship_payload_full() {
        let raw = serde_json::json!({
            "mmsi": "123456789",
            "name": "MV Test",
            "lat": 51.92,
            "lon": 4.47,
            "speed": 14.5,
            "heading": 270.0,
        });
        let props = flatten_json(&raw, "");
        assert_eq!(detect_entity_type(&props), "ship");
        let id = generate_entity_id("ship", &props);
        assert_eq!(id, "ship-123456789");
        let geo = extract_geometry(&props).unwrap();
        assert!((geo.lat - 51.92).abs() < 1e-9);
    }

    #[test]
    fn test_iot_sensor_payload() {
        let raw = serde_json::json!({
            "device_id": "sensor-001",
            "temperature": 22.5,
            "humidity": 61.2,
            "lat": -33.87,
            "lon": 151.21,
        });
        let props = flatten_json(&raw, "");
        assert_eq!(detect_entity_type(&props), "sensor");
    }

    #[test]
    fn test_aircraft_payload() {
        let raw = serde_json::json!({
            "icao": "A380CF",
            "callsign": "SQ321",
            "altitude": 38000,
            "lat": 1.35,
            "lon": 103.82,
        });
        let props = flatten_json(&raw, "");
        assert_eq!(detect_entity_type(&props), "aircraft");
        let id = generate_entity_id("aircraft", &props);
        assert_eq!(id, "aircraft-A380CF");
    }

    #[test]
    fn test_threat_payload() {
        let raw = serde_json::json!({
            "cve": "CVE-2024-12345",
            "severity": "critical",
            "affected_system": "nginx",
        });
        let props = flatten_json(&raw, "");
        assert_eq!(detect_entity_type(&props), "threat");
        let id = generate_entity_id("threat", &props);
        assert_eq!(id, "threat-CVE-2024-12345");
    }

    #[test]
    fn test_nested_payload_flattened_correctly() {
        let raw = serde_json::json!({
            "mmsi": "987654321",
            "position": {
                "lat": 48.86,
                "lon": 2.35,
            },
            "engine": {
                "status": "running",
                "rpm": 2400,
            }
        });
        let flat = flatten_json(&raw, "");
        assert!(flat.contains_key("mmsi"));
        assert!(flat.contains_key("position.lat"));
        assert!(flat.contains_key("engine.rpm"));
        assert_eq!(flat["engine.rpm"], serde_json::json!(2400));
    }
}
