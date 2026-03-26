use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// GTFS-Realtime parser
// ---------------------------------------------------------------------------
// GTFS-Realtime is a protobuf-based format for real-time transit information
// (Google/MobilityData standard).
//
// Message hierarchy:
//   FeedMessage
//     └── FeedHeader (gtfs_realtime_version, timestamp)
//     └── FeedEntity[] (id, is_deleted)
//           ├── TripUpdate (trip descriptor, stop time updates)
//           ├── VehiclePosition (position, trip, vehicle, timestamp)
//           └── Alert (informed entities, cause, effect, header/description)
//
// Since this connector avoids pulling in `prost` generated code (the ORP proto
// build is separate), we manually parse the protobuf wire format for the
// subset of GTFS-RT fields we need.
//
// Alternatively, this module accepts JSON representations of GTFS-RT data
// (as served by many transit agencies via HTTP/JSON endpoints).

/// GTFS-RT vehicle position.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GtfsVehiclePosition {
    pub trip_id: Option<String>,
    pub route_id: Option<String>,
    pub vehicle_id: Option<String>,
    pub vehicle_label: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub bearing: Option<f32>,
    pub speed: Option<f32>,
    pub timestamp: Option<u64>,
    pub occupancy_status: Option<String>,
    pub current_stop_sequence: Option<u32>,
    pub stop_id: Option<String>,
    pub current_status: Option<String>,
}

/// GTFS-RT trip update.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GtfsTripUpdate {
    pub trip_id: String,
    pub route_id: Option<String>,
    pub vehicle_id: Option<String>,
    pub timestamp: Option<u64>,
    pub stop_time_updates: Vec<GtfsStopTimeUpdate>,
}

/// A single stop time update within a trip update.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GtfsStopTimeUpdate {
    pub stop_sequence: Option<u32>,
    pub stop_id: Option<String>,
    pub arrival_delay: Option<i32>,
    pub departure_delay: Option<i32>,
    pub schedule_relationship: Option<String>,
}

/// GTFS-RT alert.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GtfsAlert {
    pub alert_id: Option<String>,
    pub cause: Option<String>,
    pub effect: Option<String>,
    pub header_text: Option<String>,
    pub description_text: Option<String>,
    pub route_ids: Vec<String>,
    pub stop_ids: Vec<String>,
}

/// Parsed GTFS-RT feed (from JSON representation).
#[derive(Clone, Debug)]
pub struct GtfsFeed {
    pub timestamp: Option<u64>,
    pub vehicle_positions: Vec<GtfsVehiclePosition>,
    pub trip_updates: Vec<GtfsTripUpdate>,
    pub alerts: Vec<GtfsAlert>,
}

// ---------------------------------------------------------------------------
// Occupancy status codes
// ---------------------------------------------------------------------------

fn occupancy_status_str(code: u64) -> &'static str {
    match code {
        0 => "EMPTY",
        1 => "MANY_SEATS_AVAILABLE",
        2 => "FEW_SEATS_AVAILABLE",
        3 => "STANDING_ROOM_ONLY",
        4 => "CRUSHED_STANDING_ROOM_ONLY",
        5 => "FULL",
        6 => "NOT_ACCEPTING_PASSENGERS",
        _ => "UNKNOWN",
    }
}

fn vehicle_stop_status_str(code: u64) -> &'static str {
    match code {
        0 => "INCOMING_AT",
        1 => "STOPPED_AT",
        2 => "IN_TRANSIT_TO",
        _ => "UNKNOWN",
    }
}

fn schedule_relationship_str(code: u64) -> &'static str {
    match code {
        0 => "SCHEDULED",
        1 => "ADDED",
        2 => "UNSCHEDULED",
        3 => "CANCELED",
        5 => "REPLACEMENT",
        _ => "UNKNOWN",
    }
}

// ---------------------------------------------------------------------------
// JSON parsers
// ---------------------------------------------------------------------------

/// Parse a GTFS-RT JSON feed (as commonly served by transit agencies).
pub fn parse_gtfs_json(data: &str) -> Result<GtfsFeed, ConnectorError> {
    let value: JsonValue = serde_json::from_str(data).map_err(|e| {
        ConnectorError::ParseError(format!("GTFS-RT: invalid JSON: {}", e))
    })?;

    parse_gtfs_json_value(&value)
}

/// Parse from a JSON Value.
pub fn parse_gtfs_json_value(value: &JsonValue) -> Result<GtfsFeed, ConnectorError> {
    let header = value.get("header").or_else(|| value.get("Header"));
    let feed_ts = header
        .and_then(|h| h.get("timestamp"))
        .and_then(|t| t.as_u64());

    let entities = value
        .get("entity")
        .or_else(|| value.get("Entity"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut vehicle_positions = Vec::new();
    let mut trip_updates = Vec::new();
    let mut alerts = Vec::new();

    for entity in &entities {
        // Vehicle position
        if let Some(vp) = entity
            .get("vehicle")
            .or_else(|| entity.get("vehiclePosition"))
        {
            if let Some(pos) = parse_vehicle_position_json(vp) {
                vehicle_positions.push(pos);
            }
        }

        // Trip update
        if let Some(tu) = entity
            .get("tripUpdate")
            .or_else(|| entity.get("trip_update"))
        {
            if let Some(update) = parse_trip_update_json(tu) {
                trip_updates.push(update);
            }
        }

        // Alert
        if let Some(al) = entity.get("alert") {
            if let Some(alert) = parse_alert_json(al, entity.get("id")) {
                alerts.push(alert);
            }
        }
    }

    Ok(GtfsFeed {
        timestamp: feed_ts,
        vehicle_positions,
        trip_updates,
        alerts,
    })
}

fn parse_vehicle_position_json(vp: &JsonValue) -> Option<GtfsVehiclePosition> {
    let pos = vp.get("position")?;
    let lat = pos.get("latitude").and_then(|v| v.as_f64())?;
    let lon = pos.get("longitude").and_then(|v| v.as_f64())?;
    let bearing = pos.get("bearing").and_then(|v| v.as_f64()).map(|v| v as f32);
    let speed = pos.get("speed").and_then(|v| v.as_f64()).map(|v| v as f32);

    let trip = vp.get("trip");
    let vehicle = vp.get("vehicle");

    let occupancy = vp
        .get("occupancyStatus")
        .or_else(|| vp.get("occupancy_status"))
        .and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_u64().map(|c| occupancy_status_str(c).to_string()))
        });

    let current_status = vp
        .get("currentStatus")
        .or_else(|| vp.get("current_status"))
        .and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_u64().map(|c| vehicle_stop_status_str(c).to_string()))
        });

    Some(GtfsVehiclePosition {
        trip_id: trip
            .and_then(|t| t.get("tripId").or_else(|| t.get("trip_id")))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        route_id: trip
            .and_then(|t| t.get("routeId").or_else(|| t.get("route_id")))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        vehicle_id: vehicle
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        vehicle_label: vehicle
            .and_then(|v| v.get("label"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        latitude: lat,
        longitude: lon,
        bearing,
        speed,
        timestamp: vp.get("timestamp").and_then(|v| v.as_u64()),
        occupancy_status: occupancy,
        current_stop_sequence: vp
            .get("currentStopSequence")
            .or_else(|| vp.get("current_stop_sequence"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        stop_id: vp
            .get("stopId")
            .or_else(|| vp.get("stop_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        current_status,
    })
}

fn parse_trip_update_json(tu: &JsonValue) -> Option<GtfsTripUpdate> {
    let trip = tu.get("trip")?;
    let trip_id = trip
        .get("tripId")
        .or_else(|| trip.get("trip_id"))
        .and_then(|v| v.as_str())?
        .to_string();

    let route_id = trip
        .get("routeId")
        .or_else(|| trip.get("route_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let vehicle_id = tu
        .get("vehicle")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let stop_updates = tu
        .get("stopTimeUpdate")
        .or_else(|| tu.get("stop_time_update"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|stu| {
                    GtfsStopTimeUpdate {
                        stop_sequence: stu
                            .get("stopSequence")
                            .or_else(|| stu.get("stop_sequence"))
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32),
                        stop_id: stu
                            .get("stopId")
                            .or_else(|| stu.get("stop_id"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        arrival_delay: stu
                            .get("arrival")
                            .and_then(|a| a.get("delay"))
                            .and_then(|v| v.as_i64())
                            .map(|v| v as i32),
                        departure_delay: stu
                            .get("departure")
                            .and_then(|d| d.get("delay"))
                            .and_then(|v| v.as_i64())
                            .map(|v| v as i32),
                        schedule_relationship: stu
                            .get("scheduleRelationship")
                            .or_else(|| stu.get("schedule_relationship"))
                            .and_then(|v| {
                                v.as_str()
                                    .map(|s| s.to_string())
                                    .or_else(|| {
                                        v.as_u64().map(|c| schedule_relationship_str(c).to_string())
                                    })
                            }),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Some(GtfsTripUpdate {
        trip_id,
        route_id,
        vehicle_id,
        timestamp: tu.get("timestamp").and_then(|v| v.as_u64()),
        stop_time_updates: stop_updates,
    })
}

fn parse_alert_json(al: &JsonValue, entity_id: Option<&JsonValue>) -> Option<GtfsAlert> {
    let header_text = al
        .get("headerText")
        .or_else(|| al.get("header_text"))
        .and_then(|v| {
            v.get("translation")
                .and_then(|t| t.as_array())
                .and_then(|arr| arr.first())
                .and_then(|t| t.get("text"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
        });

    let description_text = al
        .get("descriptionText")
        .or_else(|| al.get("description_text"))
        .and_then(|v| {
            v.get("translation")
                .and_then(|t| t.as_array())
                .and_then(|arr| arr.first())
                .and_then(|t| t.get("text"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
        });

    let mut route_ids = Vec::new();
    let mut stop_ids = Vec::new();

    if let Some(entities) = al
        .get("informedEntity")
        .or_else(|| al.get("informed_entity"))
        .and_then(|v| v.as_array())
    {
        for ie in entities {
            if let Some(rid) = ie
                .get("routeId")
                .or_else(|| ie.get("route_id"))
                .and_then(|v| v.as_str())
            {
                route_ids.push(rid.to_string());
            }
            if let Some(sid) = ie
                .get("stopId")
                .or_else(|| ie.get("stop_id"))
                .and_then(|v| v.as_str())
            {
                stop_ids.push(sid.to_string());
            }
        }
    }

    Some(GtfsAlert {
        alert_id: entity_id.and_then(|v| v.as_str()).map(|s| s.to_string()),
        cause: al.get("cause").and_then(|v| v.as_str()).map(|s| s.to_string()),
        effect: al.get("effect").and_then(|v| v.as_str()).map(|s| s.to_string()),
        header_text,
        description_text,
        route_ids,
        stop_ids,
    })
}

// ---------------------------------------------------------------------------
// SourceEvent conversion
// ---------------------------------------------------------------------------

/// Convert a vehicle position to a SourceEvent.
pub fn vehicle_position_to_source_event(
    vp: &GtfsVehiclePosition,
    connector_id: &str,
) -> SourceEvent {
    let entity_id = vp
        .vehicle_id
        .as_deref()
        .map(|id| format!("gtfs:vehicle:{}", id))
        .unwrap_or_else(|| {
            format!(
                "gtfs:vehicle:{}",
                vp.trip_id.as_deref().unwrap_or("unknown")
            )
        });

    let ts = vp
        .timestamp
        .and_then(|t| DateTime::from_timestamp(t as i64, 0))
        .unwrap_or_else(Utc::now);

    let mut properties = HashMap::new();
    properties.insert("latitude".into(), json!(vp.latitude));
    properties.insert("longitude".into(), json!(vp.longitude));

    if let Some(ref tid) = vp.trip_id {
        properties.insert("trip_id".into(), json!(tid));
    }
    if let Some(ref rid) = vp.route_id {
        properties.insert("route_id".into(), json!(rid));
    }
    if let Some(ref vid) = vp.vehicle_id {
        properties.insert("vehicle_id".into(), json!(vid));
    }
    if let Some(ref label) = vp.vehicle_label {
        properties.insert("vehicle_label".into(), json!(label));
    }
    if let Some(b) = vp.bearing {
        properties.insert("bearing".into(), json!(b));
    }
    if let Some(s) = vp.speed {
        properties.insert("speed".into(), json!(s));
    }
    if let Some(ref occ) = vp.occupancy_status {
        properties.insert("occupancy_status".into(), json!(occ));
    }
    if let Some(seq) = vp.current_stop_sequence {
        properties.insert("current_stop_sequence".into(), json!(seq));
    }
    if let Some(ref sid) = vp.stop_id {
        properties.insert("stop_id".into(), json!(sid));
    }
    if let Some(ref cs) = vp.current_status {
        properties.insert("current_status".into(), json!(cs));
    }

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "vehicle".into(),
        properties,
        timestamp: ts,
        latitude: Some(vp.latitude),
        longitude: Some(vp.longitude),
    }
}

/// Convert a trip update to a SourceEvent.
pub fn trip_update_to_source_event(
    tu: &GtfsTripUpdate,
    connector_id: &str,
) -> SourceEvent {
    let entity_id = format!("gtfs:trip:{}", tu.trip_id);

    let ts = tu
        .timestamp
        .and_then(|t| DateTime::from_timestamp(t as i64, 0))
        .unwrap_or_else(Utc::now);

    let mut properties = HashMap::new();
    properties.insert("trip_id".into(), json!(tu.trip_id));
    if let Some(ref rid) = tu.route_id {
        properties.insert("route_id".into(), json!(rid));
    }
    if let Some(ref vid) = tu.vehicle_id {
        properties.insert("vehicle_id".into(), json!(vid));
    }

    let updates: Vec<JsonValue> = tu
        .stop_time_updates
        .iter()
        .map(|stu| {
            json!({
                "stop_sequence": stu.stop_sequence,
                "stop_id": stu.stop_id,
                "arrival_delay": stu.arrival_delay,
                "departure_delay": stu.departure_delay,
                "schedule_relationship": stu.schedule_relationship,
            })
        })
        .collect();
    properties.insert("stop_time_updates".into(), json!(updates));

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "vehicle".into(),
        properties,
        timestamp: ts,
        latitude: None,
        longitude: None,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct GtfsConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl GtfsConnector {
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
impl Connector for GtfsConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        let url = self
            .config
            .url
            .as_deref()
            .ok_or_else(|| {
                ConnectorError::ConfigError("GTFS-RT: url (endpoint or file path) required".into())
            })?;

        let content = if url.starts_with("http://") || url.starts_with("https://") {
            reqwest::get(url)
                .await
                .map_err(|e| ConnectorError::ConnectionError(format!("GTFS-RT: HTTP error: {}", e)))?
                .text()
                .await
                .map_err(|e| ConnectorError::ConnectionError(format!("GTFS-RT: read error: {}", e)))?
        } else {
            tokio::fs::read_to_string(url)
                .await
                .map_err(ConnectorError::IoError)?
        };

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        let feed = parse_gtfs_json(&content).inspect_err(|_e| {
            errors.fetch_add(1, Ordering::Relaxed);
        })?;

        for vp in &feed.vehicle_positions {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let event = vehicle_position_to_source_event(vp, &connector_id);
            if tx.send(event).await.is_err() {
                break;
            }
            events_processed.fetch_add(1, Ordering::Relaxed);
        }

        for tu in &feed.trip_updates {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let event = trip_update_to_source_event(tu, &connector_id);
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
                "GTFS-RT connector is not running".into(),
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

    fn sample_feed_json() -> &'static str {
        r#"{
            "header": {"gtfsRealtimeVersion": "2.0", "timestamp": 1700000000},
            "entity": [
                {
                    "id": "v1",
                    "vehicle": {
                        "trip": {"tripId": "trip-100", "routeId": "route-A"},
                        "vehicle": {"id": "bus-42", "label": "Route A Express"},
                        "position": {"latitude": 40.7128, "longitude": -74.0060, "bearing": 180.0, "speed": 12.5},
                        "timestamp": 1700000000,
                        "currentStopSequence": 5,
                        "stopId": "stop-15",
                        "currentStatus": "IN_TRANSIT_TO"
                    }
                },
                {
                    "id": "t1",
                    "tripUpdate": {
                        "trip": {"tripId": "trip-200", "routeId": "route-B"},
                        "vehicle": {"id": "bus-99"},
                        "timestamp": 1700000100,
                        "stopTimeUpdate": [
                            {"stopSequence": 1, "stopId": "stop-1", "arrival": {"delay": 30}, "departure": {"delay": 45}},
                            {"stopSequence": 2, "stopId": "stop-2", "arrival": {"delay": -10}}
                        ]
                    }
                },
                {
                    "id": "a1",
                    "alert": {
                        "cause": "CONSTRUCTION",
                        "effect": "DETOUR",
                        "headerText": {"translation": [{"text": "Route detour"}]},
                        "descriptionText": {"translation": [{"text": "Detour due to construction on Main St"}]},
                        "informedEntity": [
                            {"routeId": "route-C"},
                            {"stopId": "stop-20"}
                        ]
                    }
                }
            ]
        }"#
    }

    #[test]
    fn test_parse_gtfs_feed() {
        let feed = parse_gtfs_json(sample_feed_json()).unwrap();
        assert_eq!(feed.timestamp, Some(1700000000));
        assert_eq!(feed.vehicle_positions.len(), 1);
        assert_eq!(feed.trip_updates.len(), 1);
        assert_eq!(feed.alerts.len(), 1);
    }

    #[test]
    fn test_vehicle_position_fields() {
        let feed = parse_gtfs_json(sample_feed_json()).unwrap();
        let vp = &feed.vehicle_positions[0];
        assert_eq!(vp.vehicle_id, Some("bus-42".into()));
        assert_eq!(vp.trip_id, Some("trip-100".into()));
        assert_eq!(vp.route_id, Some("route-A".into()));
        assert!((vp.latitude - 40.7128).abs() < 0.001);
        assert!((vp.longitude - (-74.0060)).abs() < 0.001);
        assert!((vp.bearing.unwrap() - 180.0).abs() < 0.1);
        assert!((vp.speed.unwrap() - 12.5).abs() < 0.1);
        assert_eq!(vp.current_stop_sequence, Some(5));
        assert_eq!(vp.stop_id, Some("stop-15".into()));
    }

    #[test]
    fn test_trip_update_fields() {
        let feed = parse_gtfs_json(sample_feed_json()).unwrap();
        let tu = &feed.trip_updates[0];
        assert_eq!(tu.trip_id, "trip-200");
        assert_eq!(tu.route_id, Some("route-B".into()));
        assert_eq!(tu.stop_time_updates.len(), 2);
        assert_eq!(tu.stop_time_updates[0].arrival_delay, Some(30));
        assert_eq!(tu.stop_time_updates[0].departure_delay, Some(45));
        assert_eq!(tu.stop_time_updates[1].arrival_delay, Some(-10));
    }

    #[test]
    fn test_alert_fields() {
        let feed = parse_gtfs_json(sample_feed_json()).unwrap();
        let alert = &feed.alerts[0];
        assert_eq!(alert.cause, Some("CONSTRUCTION".into()));
        assert_eq!(alert.effect, Some("DETOUR".into()));
        assert_eq!(alert.header_text, Some("Route detour".into()));
        assert_eq!(alert.route_ids, vec!["route-C"]);
        assert_eq!(alert.stop_ids, vec!["stop-20"]);
    }

    #[test]
    fn test_vehicle_position_to_source_event() {
        let feed = parse_gtfs_json(sample_feed_json()).unwrap();
        let event = vehicle_position_to_source_event(&feed.vehicle_positions[0], "gtfs-test");
        assert_eq!(event.entity_type, "vehicle");
        assert_eq!(event.entity_id, "gtfs:vehicle:bus-42");
        assert!((event.latitude.unwrap() - 40.7128).abs() < 0.001);
        assert_eq!(event.properties["route_id"], json!("route-A"));
        assert_eq!(event.properties["speed"], json!(12.5));
    }

    #[test]
    fn test_trip_update_to_source_event() {
        let feed = parse_gtfs_json(sample_feed_json()).unwrap();
        let event = trip_update_to_source_event(&feed.trip_updates[0], "gtfs-test");
        assert_eq!(event.entity_type, "vehicle");
        assert_eq!(event.entity_id, "gtfs:trip:trip-200");
        assert_eq!(event.properties["trip_id"], json!("trip-200"));
    }

    #[test]
    fn test_empty_feed() {
        let data = r#"{"header": {"timestamp": 1700000000}, "entity": []}"#;
        let feed = parse_gtfs_json(data).unwrap();
        assert_eq!(feed.vehicle_positions.len(), 0);
        assert_eq!(feed.trip_updates.len(), 0);
    }

    #[test]
    fn test_invalid_json() {
        assert!(parse_gtfs_json("{invalid}").is_err());
    }

    #[test]
    fn test_occupancy_status_codes() {
        assert_eq!(occupancy_status_str(0), "EMPTY");
        assert_eq!(occupancy_status_str(5), "FULL");
        assert_eq!(occupancy_status_str(99), "UNKNOWN");
    }

    #[test]
    fn test_gtfs_connector_id() {
        let config = ConnectorConfig {
            connector_id: "gtfs-1".to_string(),
            connector_type: "gtfs".to_string(),
            url: None,
            entity_type: "vehicle".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = GtfsConnector::new(config);
        assert_eq!(connector.connector_id(), "gtfs-1");
    }

    #[tokio::test]
    async fn test_gtfs_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "gtfs-h".to_string(),
            connector_type: "gtfs".to_string(),
            url: None,
            entity_type: "vehicle".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = GtfsConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }
}
