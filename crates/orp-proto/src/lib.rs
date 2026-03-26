//! ORP Protocol Types — Canonical event format for all ORP data.
//!
//! All events flowing through ORP conform to [`OrpEvent`]. The spec is the golden source;
//! this module implements it exactly (Powerful.md + BUILD_CORE_ENGINE.md §5).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

// ── Generated protobuf code ─────────────────────────────────────────────────
pub mod protos {
    include!(concat!(env!("OUT_DIR"), "/orp.event.rs"));
}

// ── Canonical Rust types ────────────────────────────────────────────────────

/// Geospatial point (WGS-84).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    /// Altitude in metres above sea level, if known.
    pub alt: Option<f64>,
}

/// Audit trail metadata attached to every event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditMetadata {
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    /// Unique ID for the operation that produced this event.
    pub operation_id: String,
}

/// How confident ORP is in an event / data point (0.0 = unknown, 1.0 = verified).
///
/// NOTE: `f64` as per spec (Powerful.md canonical struct).
pub type Confidence = f64;

/// Severity levels for alert events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSeverity {
    Info,
    Warning,
    Critical,
}

/// What happened to a relationship.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationshipAction {
    Created,
    Updated,
    Deleted,
}

/// Alert severity alias (re-exported for parity with Powerful.md).
pub type AlertSeverity = EventSeverity;

/// The event payload — exactly one variant is active.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventPayload {
    /// Entity position / movement update.
    PositionUpdate {
        latitude: f64,
        longitude: f64,
        altitude: Option<f64>,
        /// Horizontal accuracy in metres.
        accuracy_meters: Option<f32>,
        speed_knots: Option<f64>,
        heading_degrees: Option<f64>,
        course_degrees: Option<f64>,
    },

    /// A named property on the entity changed value.
    ///
    /// `key` (not `property_key`) per Powerful.md canonical spec.
    PropertyChange {
        key: String,
        old_value: Option<JsonValue>,
        new_value: JsonValue,
        /// `true` if the value was computed/derived rather than directly observed.
        is_derived: bool,
    },

    /// Entity moved from one discrete state to another.
    ///
    /// `from_state`/`to_state` per Powerful.md.
    StateTransition {
        from_state: String,
        to_state: String,
        reason: Option<String>,
    },

    /// A relationship between entities was created, updated, or deleted.
    RelationshipChange {
        relationship_type: String,
        target_entity_id: String,
        /// Created / Updated / Deleted per Powerful.md.
        action: RelationshipAction,
        properties: HashMap<String, JsonValue>,
    },

    /// An automated monitor rule fired.
    AlertTriggered {
        monitor_id: String,
        severity: AlertSeverity,
        message: String,
        /// Supporting data that caused the alert.
        evidence: JsonValue,
    },

    /// Catch-all for connector-specific custom data.
    Custom { data: JsonValue },
}

/// **Canonical ORP Event** — every piece of data flowing through ORP is an `OrpEvent`.
///
/// This struct exactly matches the specification in Powerful.md and
/// BUILD_CORE_ENGINE.md §5 (with the spec-mandated corrections applied):
///
/// - `id` uses UUIDv7 (time-ordered) so events sort chronologically by ID alone.
/// - `geo` is a top-level optional field (not buried in the payload).
/// - `signature` is `Option<Vec<u8>>` (raw Ed25519 bytes, not base64 `String`).
/// - `confidence` is `f64` (not `f32`).
/// - `audit: AuditMetadata` is always present.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrpEvent {
    /// Globally unique event ID — UUIDv7, time-ordered.
    pub id: uuid::Uuid,

    /// What kind of entity is this about? ("ship", "port", "aircraft", …)
    pub entity_type: String,

    /// Which specific entity? ("mmsi:123456789", "icao:A1B2C3", …)
    pub entity_id: String,

    /// When this observation was made (source timestamp, **not** ingestion time).
    pub timestamp: DateTime<Utc>,

    /// When ORP ingested this event.
    pub ingestion_timestamp: DateTime<Utc>,

    /// Geospatial location at the time of this event (optional).
    pub geo: Option<GeoPoint>,

    /// Structured event payload.
    pub payload: EventPayload,

    /// Which connector produced this event.
    pub source_id: String,

    /// Source reliability score (0.0 = unknown, 1.0 = fully verified).
    pub source_trust: f64,

    /// ORP's overall confidence in this event (0.0–1.0).
    pub confidence: f64,

    /// Alert severity, if this is an alert event.
    pub severity: Option<EventSeverity>,

    /// Raw Ed25519 signature bytes set by the connector (None if not signed).
    pub signature: Option<Vec<u8>>,

    /// Audit trail metadata.
    pub audit: AuditMetadata,
}

impl OrpEvent {
    /// Create a new `OrpEvent` with sensible defaults.
    ///
    /// The `id` is generated as UUIDv7 (time-ordered). `audit.operation_id` is
    /// also a UUIDv7 so it can be used to correlate a batch of related events.
    pub fn new(
        entity_id: String,
        entity_type: String,
        payload: EventPayload,
        source_id: String,
        source_trust: f64,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::now_v7(),
            entity_type,
            entity_id,
            timestamp: now,
            ingestion_timestamp: now,
            geo: None,
            payload,
            source_id,
            source_trust,
            confidence: source_trust,
            severity: None,
            signature: None,
            audit: AuditMetadata {
                user_id: None,
                api_key_id: None,
                operation_id: uuid::Uuid::now_v7().to_string(),
            },
        }
    }

    /// Attach a geospatial location to this event (builder-style).
    pub fn with_geo(mut self, geo: GeoPoint) -> Self {
        self.geo = Some(geo);
        self
    }

    /// Attach an alert severity to this event (builder-style).
    pub fn with_severity(mut self, severity: EventSeverity) -> Self {
        self.severity = Some(severity);
        self
    }

    /// Attach an Ed25519 signature to this event (builder-style).
    pub fn with_signature(mut self, sig: Vec<u8>) -> Self {
        self.signature = Some(sig);
        self
    }

    /// Serialize the event to a compact JSON string.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    /// Serialize the event to pretty-printed JSON.
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Serialize the event to protobuf bytes using the generated `prost` code.
    pub fn to_protobuf(&self) -> Result<Vec<u8>, prost::EncodeError> {
        use prost::Message;
        let proto = self.to_proto_message();
        let mut buf = Vec::new();
        proto.encode(&mut buf)?;
        Ok(buf)
    }

    /// Deserialize an `OrpEvent` from protobuf bytes.
    pub fn from_protobuf(bytes: &[u8]) -> Result<Self, prost::DecodeError> {
        use prost::Message;
        let proto = protos::OrpEvent::decode(bytes)?;
        Ok(Self::from_proto_message(proto))
    }

    // ── Internal proto conversion helpers ────────────────────────────────────

    fn to_proto_message(&self) -> protos::OrpEvent {
        use prost_types::Timestamp;

        let to_ts = |dt: DateTime<Utc>| Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        };

        let severity = match &self.severity {
            None | Some(EventSeverity::Info) => 0i32,
            Some(EventSeverity::Warning) => 1,
            Some(EventSeverity::Critical) => 2,
        };

        let geo = self.geo.as_ref().map(|g| protos::GeoPoint {
            lat: g.lat,
            lon: g.lon,
            alt: g.alt,
        });

        let audit = {
            let a = &self.audit;
            Some(protos::AuditMetadata {
                user_id: a.user_id.clone(),
                api_key_id: a.api_key_id.clone(),
                operation_id: a.operation_id.clone(),
            })
        };

        let payload = self.build_proto_payload();

        protos::OrpEvent {
            event_id: self.id.to_string(),
            entity_id: self.entity_id.clone(),
            entity_type: self.entity_type.clone(),
            event_type: self.event_type_str().to_string(),
            event_timestamp: Some(to_ts(self.timestamp)),
            ingestion_timestamp: Some(to_ts(self.ingestion_timestamp)),
            source_id: self.source_id.clone(),
            source_trust: self.source_trust as f32,
            payload: Some(payload),
            confidence: self.confidence,
            severity,
            signature: self.signature.clone().unwrap_or_default().into(),
            audit,
            geo,
        }
    }

    fn from_proto_message(proto: protos::OrpEvent) -> Self {
        use prost_types::Timestamp;

        let from_ts = |ts: Option<Timestamp>| -> DateTime<Utc> {
            ts.map(|t| {
                DateTime::from_timestamp(t.seconds, t.nanos as u32)
                    .unwrap_or_else(Utc::now)
            })
            .unwrap_or_else(Utc::now)
        };

        let geo = proto.geo.map(|g| GeoPoint {
            lat: g.lat,
            lon: g.lon,
            alt: g.alt,
        });

        let severity = match proto.severity {
            1 => Some(EventSeverity::Warning),
            2 => Some(EventSeverity::Critical),
            _ => Some(EventSeverity::Info),
        };

        let audit = proto
            .audit
            .map(|a| AuditMetadata {
                user_id: a.user_id,
                api_key_id: a.api_key_id,
                operation_id: a.operation_id,
            })
            .unwrap_or_else(|| AuditMetadata {
                user_id: None,
                api_key_id: None,
                operation_id: uuid::Uuid::now_v7().to_string(),
            });

        let payload = proto
            .payload
            .map(Self::parse_proto_payload)
            .unwrap_or(EventPayload::Custom {
                data: JsonValue::Null,
            });

        let id = uuid::Uuid::parse_str(&proto.event_id)
            .unwrap_or_else(|_| uuid::Uuid::now_v7());

        let sig = if proto.signature.is_empty() {
            None
        } else {
            Some(proto.signature.to_vec())
        };

        Self {
            id,
            entity_type: proto.entity_type,
            entity_id: proto.entity_id,
            timestamp: from_ts(proto.event_timestamp),
            ingestion_timestamp: from_ts(proto.ingestion_timestamp),
            geo,
            payload,
            source_id: proto.source_id,
            source_trust: proto.source_trust as f64,
            confidence: proto.confidence,
            severity,
            signature: sig,
            audit,
        }
    }

    fn build_proto_payload(&self) -> protos::EventPayload {
        use protos::event_payload::Variant;

        let variant = match &self.payload {
            EventPayload::PositionUpdate {
                latitude,
                longitude,
                altitude,
                accuracy_meters,
                speed_knots,
                heading_degrees,
                course_degrees,
            } => Variant::PositionUpdate(protos::PositionUpdate {
                latitude: *latitude,
                longitude: *longitude,
                altitude: *altitude,
                accuracy_meters: *accuracy_meters,
                speed_knots: *speed_knots,
                heading_degrees: *heading_degrees,
                course_degrees: *course_degrees,
            }),

            EventPayload::PropertyChange {
                key,
                old_value,
                new_value,
                is_derived,
            } => Variant::PropertyChange(protos::PropertyChange {
                key: key.clone(),
                old_value: old_value.as_ref().map(|v: &JsonValue| v.to_string()),
                new_value: new_value.to_string(),
                is_derived: *is_derived,
            }),

            EventPayload::StateTransition {
                from_state,
                to_state,
                reason,
            } => Variant::StateTransition(protos::StateTransition {
                from_state: from_state.clone(),
                to_state: to_state.clone(),
                reason: reason.clone(),
            }),

            EventPayload::RelationshipChange {
                relationship_type,
                target_entity_id,
                action,
                properties,
            } => {
                let proto_action = match action {
                    RelationshipAction::Created => 0i32,
                    RelationshipAction::Updated => 1i32,
                    RelationshipAction::Deleted => 2i32,
                };
                Variant::RelationshipChange(protos::RelationshipChange {
                    relationship_type: relationship_type.clone(),
                    target_entity_id: target_entity_id.clone(),
                    action: proto_action,
                    properties: properties
                        .iter()
                        .map(|(k, v): (&String, &JsonValue)| (k.clone(), v.to_string()))
                        .collect(),
                })
            }

            EventPayload::AlertTriggered {
                monitor_id,
                severity,
                message,
                evidence,
            } => {
                let proto_sev = match severity {
                    AlertSeverity::Info => 0i32,
                    AlertSeverity::Warning => 1,
                    AlertSeverity::Critical => 2,
                };
                let evidence_struct = json_value_to_prost_struct(evidence);
                Variant::AlertTriggered(protos::AlertTriggered {
                    monitor_id: monitor_id.clone(),
                    severity: proto_sev,
                    message: message.clone(),
                    evidence: Some(evidence_struct),
                })
            }

            EventPayload::Custom { data } => {
                Variant::Custom(protos::CustomData {
                    data: Some(json_value_to_prost_struct(data)),
                })
            }
        };

        protos::EventPayload {
            variant: Some(variant),
        }
    }

    fn parse_proto_payload(proto: protos::EventPayload) -> EventPayload {
        use protos::event_payload::Variant;

        match proto.variant {
            None => EventPayload::Custom { data: JsonValue::Null },

            Some(Variant::PositionUpdate(p)) => EventPayload::PositionUpdate {
                latitude: p.latitude,
                longitude: p.longitude,
                altitude: p.altitude,
                accuracy_meters: p.accuracy_meters,
                speed_knots: p.speed_knots,
                heading_degrees: p.heading_degrees,
                course_degrees: p.course_degrees,
            },

            Some(Variant::PropertyChange(p)) => EventPayload::PropertyChange {
                key: p.key,
                old_value: p.old_value.map(JsonValue::String),
                new_value: serde_json::from_str(&p.new_value)
                    .unwrap_or(JsonValue::String(p.new_value)),
                is_derived: p.is_derived,
            },

            Some(Variant::StateTransition(p)) => EventPayload::StateTransition {
                from_state: p.from_state,
                to_state: p.to_state,
                reason: p.reason,
            },

            Some(Variant::RelationshipChange(p)) => {
                let action = match p.action {
                    1 => RelationshipAction::Updated,
                    2 => RelationshipAction::Deleted,
                    _ => RelationshipAction::Created,
                };
                EventPayload::RelationshipChange {
                    relationship_type: p.relationship_type,
                    target_entity_id: p.target_entity_id,
                    action,
                    properties: p
                        .properties
                        .into_iter()
                        .map(|(k, v)| {
                            (k, serde_json::from_str(&v).unwrap_or(JsonValue::String(v)))
                        })
                        .collect(),
                }
            }

            Some(Variant::AlertTriggered(p)) => {
                let severity = match p.severity {
                    1 => AlertSeverity::Warning,
                    2 => AlertSeverity::Critical,
                    _ => AlertSeverity::Info,
                };
                let evidence = p
                    .evidence
                    .map(prost_struct_to_json_value)
                    .unwrap_or(JsonValue::Null);
                EventPayload::AlertTriggered {
                    monitor_id: p.monitor_id,
                    severity,
                    message: p.message,
                    evidence,
                }
            }

            Some(Variant::Custom(p)) => {
                let data = p
                    .data
                    .map(prost_struct_to_json_value)
                    .unwrap_or(JsonValue::Null);
                EventPayload::Custom { data }
            }
        }
    }

    /// Return the event type string inferred from the payload.
    pub fn event_type_str(&self) -> &str {
        match &self.payload {
            EventPayload::PositionUpdate { .. } => "position_update",
            EventPayload::PropertyChange { .. } => "property_change",
            EventPayload::StateTransition { .. } => "state_transition",
            EventPayload::RelationshipChange { .. } => "relationship_change",
            EventPayload::AlertTriggered { .. } => "alert_triggered",
            EventPayload::Custom { .. } => "custom",
        }
    }
}

// ── Entity / storage types (kept here for downstream convenience) ────────────

/// Canonical entity representation used by the storage trait.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: String,
    pub entity_type: String,
    pub canonical_id: Option<String>,
    pub name: Option<String>,
    pub properties: HashMap<String, JsonValue>,
    /// Confidence in f64 per spec (Powerful.md).
    pub confidence: f64,
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub geometry: Option<GeoPoint>,
    pub is_active: bool,
}

impl Default for Entity {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            entity_id: String::new(),
            entity_type: String::new(),
            canonical_id: None,
            name: None,
            properties: HashMap::new(),
            confidence: 1.0,
            created_at: now,
            last_updated: now,
            geometry: None,
            is_active: true,
        }
    }
}

/// Relationship between two entities.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Relationship {
    pub relationship_id: String,
    pub source_entity_id: String,
    pub target_entity_id: String,
    pub relationship_type: String,
    pub properties: HashMap<String, JsonValue>,
    pub confidence: f64,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Registered data source / connector metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DataSource {
    pub source_id: String,
    pub source_name: String,
    pub source_type: String,
    pub trust_score: f64,
    pub events_ingested: u64,
    pub enabled: bool,
}

/// Storage layer statistics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StorageStats {
    pub total_entities: u64,
    pub total_events: u64,
    pub total_relationships: u64,
    pub database_size_bytes: u64,
}

/// A single persisted event record (flat form stored in DuckDB).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub entity_id: String,
    pub event_type: String,
    pub event_timestamp: DateTime<Utc>,
    pub source_id: String,
    pub data: JsonValue,
    pub confidence: f64,
}

// ── Proto ↔ serde_json conversion helpers ───────────────────────────────────

fn json_value_to_prost_struct(value: &JsonValue) -> prost_types::Struct {
    match value {
        JsonValue::Object(map) => {
            let fields = map
                .iter()
                .map(|(k, v)| (k.clone(), json_value_to_prost_value(v)))
                .collect();
            prost_types::Struct { fields }
        }
        _ => prost_types::Struct {
            fields: std::collections::BTreeMap::new(),
        },
    }
}

fn json_value_to_prost_value(value: &JsonValue) -> prost_types::Value {
    use prost_types::value::Kind;
    let kind = match value {
        JsonValue::Null => Kind::NullValue(0),
        JsonValue::Bool(b) => Kind::BoolValue(*b),
        JsonValue::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
        JsonValue::String(s) => Kind::StringValue(s.clone()),
        JsonValue::Array(arr) => Kind::ListValue(prost_types::ListValue {
            values: arr.iter().map(json_value_to_prost_value).collect(),
        }),
        JsonValue::Object(_) => Kind::StructValue(json_value_to_prost_struct(value)),
    };
    prost_types::Value { kind: Some(kind) }
}

fn prost_struct_to_json_value(s: prost_types::Struct) -> JsonValue {
    let map: serde_json::Map<String, JsonValue> = s
        .fields
        .into_iter()
        .map(|(k, v)| (k, prost_value_to_json_value(v)))
        .collect();
    JsonValue::Object(map)
}

fn prost_value_to_json_value(v: prost_types::Value) -> JsonValue {
    use prost_types::value::Kind;
    match v.kind {
        None => JsonValue::Null,
        Some(Kind::NullValue(_)) => JsonValue::Null,
        Some(Kind::BoolValue(b)) => JsonValue::Bool(b),
        Some(Kind::NumberValue(n)) => {
            serde_json::Number::from_f64(n)
                .map(JsonValue::Number)
                .unwrap_or(JsonValue::Null)
        }
        Some(Kind::StringValue(s)) => JsonValue::String(s),
        Some(Kind::ListValue(list)) => {
            JsonValue::Array(list.values.into_iter().map(prost_value_to_json_value).collect())
        }
        Some(Kind::StructValue(s)) => prost_struct_to_json_value(s),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_position_event() -> OrpEvent {
        OrpEvent::new(
            "mmsi:123456789".to_string(),
            "ship".to_string(),
            EventPayload::PositionUpdate {
                latitude: 51.9225,
                longitude: 4.4792,
                altitude: None,
                accuracy_meters: None,
                speed_knots: Some(12.3),
                heading_degrees: Some(243.0),
                course_degrees: Some(245.0),
            },
            "ais-tap-01".to_string(),
            0.95,
        )
    }

    // ── 1. UUIDv7 IDs are generated ─────────────────────────────────────────
    #[test]
    fn test_event_id_is_uuid_v7() {
        let event = make_position_event();
        // UUIDv7 version nibble is 7
        assert_eq!(event.id.get_version_num(), 7);
    }

    // ── 2. IDs are time-ordered ──────────────────────────────────────────────
    #[test]
    fn test_uuid_v7_time_ordered() {
        let e1 = make_position_event();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let e2 = make_position_event();
        assert!(e2.id > e1.id, "UUIDv7 must be monotonically increasing");
    }

    // ── 3. JSON serialization roundtrip (PositionUpdate) ─────────────────────
    #[test]
    fn test_json_roundtrip_position() {
        let event = make_position_event();
        let json = event.to_json().expect("serialization failed");
        let back: OrpEvent = serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(back.entity_id, event.entity_id);
        assert_eq!(back.entity_type, event.entity_type);
        assert_eq!(back.id, event.id);
    }

    // ── 4. JSON roundtrip (PropertyChange) ──────────────────────────────────
    #[test]
    fn test_json_roundtrip_property_change() {
        let event = OrpEvent::new(
            "ship-1".to_string(),
            "ship".to_string(),
            EventPayload::PropertyChange {
                key: "speed".to_string(),
                old_value: Some(json!(10.0)),
                new_value: json!(15.5),
                is_derived: false,
            },
            "ais".to_string(),
            0.9,
        );
        let json = event.to_json().unwrap();
        let back: OrpEvent = serde_json::from_str(&json).unwrap();
        if let EventPayload::PropertyChange { key, new_value, .. } = &back.payload {
            assert_eq!(key, "speed");
            assert_eq!(new_value, &json!(15.5));
        } else {
            panic!("wrong payload variant");
        }
    }

    // ── 5. JSON roundtrip (StateTransition) ──────────────────────────────────
    #[test]
    fn test_json_roundtrip_state_transition() {
        let event = OrpEvent::new(
            "ship-2".to_string(),
            "ship".to_string(),
            EventPayload::StateTransition {
                from_state: "underway".to_string(),
                to_state: "moored".to_string(),
                reason: Some("arrived at berth".to_string()),
            },
            "port-system".to_string(),
            0.99,
        );
        let json = event.to_json().unwrap();
        let back: OrpEvent = serde_json::from_str(&json).unwrap();
        if let EventPayload::StateTransition { from_state, to_state, .. } = &back.payload {
            assert_eq!(from_state, "underway");
            assert_eq!(to_state, "moored");
        } else {
            panic!("wrong payload variant");
        }
    }

    // ── 6. JSON roundtrip (RelationshipChange with action enum) ─────────────
    #[test]
    fn test_json_roundtrip_relationship_change() {
        let mut props = HashMap::new();
        props.insert("eta".to_string(), json!("2026-04-01T12:00:00Z"));

        let event = OrpEvent::new(
            "ship-3".to_string(),
            "ship".to_string(),
            EventPayload::RelationshipChange {
                relationship_type: "heading_to".to_string(),
                target_entity_id: "port-rotterdam".to_string(),
                action: RelationshipAction::Created,
                properties: props,
            },
            "ais".to_string(),
            0.88,
        );
        let json = event.to_json().unwrap();
        let back: OrpEvent = serde_json::from_str(&json).unwrap();
        if let EventPayload::RelationshipChange { action, target_entity_id, .. } = &back.payload {
            assert_eq!(*action, RelationshipAction::Created);
            assert_eq!(target_entity_id, "port-rotterdam");
        } else {
            panic!("wrong payload variant");
        }
    }

    // ── 7. JSON roundtrip (AlertTriggered with severity + evidence) ──────────
    #[test]
    fn test_json_roundtrip_alert_triggered() {
        let event = OrpEvent::new(
            "ship-4".to_string(),
            "ship".to_string(),
            EventPayload::AlertTriggered {
                monitor_id: "rule-001".to_string(),
                severity: AlertSeverity::Critical,
                message: "Course deviation > 30°".to_string(),
                evidence: json!({"deviation_deg": 52, "last_heading": 270}),
            },
            "monitor".to_string(),
            1.0,
        )
        .with_severity(EventSeverity::Critical);

        let json = event.to_json().unwrap();
        let back: OrpEvent = serde_json::from_str(&json).unwrap();
        if let EventPayload::AlertTriggered { severity, evidence, .. } = &back.payload {
            assert_eq!(*severity, AlertSeverity::Critical);
            assert_eq!(evidence["deviation_deg"], json!(52));
        } else {
            panic!("wrong payload variant");
        }
    }

    // ── 8. Geo field roundtrip ────────────────────────────────────────────────
    #[test]
    fn test_geo_field_roundtrip() {
        let event = make_position_event().with_geo(GeoPoint {
            lat: 51.9225,
            lon: 4.4792,
            alt: Some(0.0),
        });
        let json = event.to_json().unwrap();
        let back: OrpEvent = serde_json::from_str(&json).unwrap();
        let geo = back.geo.expect("geo should be present");
        assert!((geo.lat - 51.9225).abs() < 1e-6);
        assert!((geo.lon - 4.4792).abs() < 1e-6);
    }

    // ── 9. Signature is Vec<u8>, not String ──────────────────────────────────
    #[test]
    fn test_signature_is_bytes() {
        let sig_bytes: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let event = make_position_event().with_signature(sig_bytes.clone());
        let json = event.to_json().unwrap();
        let back: OrpEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.signature, Some(sig_bytes));
    }

    // ── 10. confidence is f64 ─────────────────────────────────────────────────
    #[test]
    fn test_confidence_is_f64() {
        let event = make_position_event();
        // Just verify it round-trips as f64 precision
        let c: f64 = 0.9500000000000001_f64;
        let mut e2 = event;
        e2.confidence = c;
        let json = e2.to_json().unwrap();
        let back: OrpEvent = serde_json::from_str(&json).unwrap();
        assert!((back.confidence - c).abs() < 1e-15);
    }

    // ── 11. AuditMetadata is always present ───────────────────────────────────
    #[test]
    fn test_audit_metadata_present() {
        let event = make_position_event();
        assert!(!event.audit.operation_id.is_empty());
        assert!(event.audit.user_id.is_none());
    }

    // ── 12. Custom payload roundtrip ─────────────────────────────────────────
    #[test]
    fn test_json_roundtrip_custom() {
        let event = OrpEvent::new(
            "sensor-42".to_string(),
            "sensor".to_string(),
            EventPayload::Custom {
                data: json!({"temperature": 37.2, "unit": "C"}),
            },
            "mqtt-sensor".to_string(),
            0.7,
        );
        let json = event.to_json().unwrap();
        let back: OrpEvent = serde_json::from_str(&json).unwrap();
        if let EventPayload::Custom { data } = &back.payload {
            assert_eq!(data["temperature"], json!(37.2));
        } else {
            panic!("wrong payload variant");
        }
    }

    // ── 13. Protobuf roundtrip (PositionUpdate) ───────────────────────────────
    #[test]
    fn test_protobuf_roundtrip_position() {
        let event = make_position_event();
        let bytes = event.to_protobuf().expect("encode failed");
        let back = OrpEvent::from_protobuf(&bytes).expect("decode failed");
        assert_eq!(back.entity_id, event.entity_id);
        assert_eq!(back.source_id, event.source_id);
    }

    // ── 14. Protobuf roundtrip (AlertTriggered) ───────────────────────────────
    #[test]
    fn test_protobuf_roundtrip_alert() {
        let event = OrpEvent::new(
            "ship-99".to_string(),
            "ship".to_string(),
            EventPayload::AlertTriggered {
                monitor_id: "rule-999".to_string(),
                severity: AlertSeverity::Warning,
                message: "Speed limit exceeded".to_string(),
                evidence: json!({"speed": 35.0, "limit": 25.0}),
            },
            "monitor-engine".to_string(),
            0.95,
        );
        let bytes = event.to_protobuf().expect("encode failed");
        let back = OrpEvent::from_protobuf(&bytes).expect("decode failed");
        if let EventPayload::AlertTriggered { severity, .. } = &back.payload {
            assert_eq!(*severity, AlertSeverity::Warning);
        } else {
            panic!("wrong payload variant after proto roundtrip");
        }
    }

    // ── 15. Entity default ───────────────────────────────────────────────────
    #[test]
    fn test_entity_default() {
        let entity = Entity::default();
        assert!(entity.is_active);
        assert_eq!(entity.confidence, 1.0_f64);
    }

    // ── 16. Event type string ────────────────────────────────────────────────
    #[test]
    fn test_event_type_str_position() {
        let event = make_position_event();
        assert_eq!(event.event_type_str(), "position_update");
    }

    #[test]
    fn test_event_type_str_property_change() {
        let event = OrpEvent::new(
            "e1".to_string(),
            "ship".to_string(),
            EventPayload::PropertyChange {
                key: "speed".to_string(),
                old_value: None,
                new_value: json!(10),
                is_derived: false,
            },
            "src".to_string(),
            0.9,
        );
        assert_eq!(event.event_type_str(), "property_change");
    }

    #[test]
    fn test_event_type_str_state_transition() {
        let event = OrpEvent::new(
            "e1".to_string(),
            "ship".to_string(),
            EventPayload::StateTransition {
                from_state: "a".to_string(),
                to_state: "b".to_string(),
                reason: None,
            },
            "src".to_string(),
            0.9,
        );
        assert_eq!(event.event_type_str(), "state_transition");
    }

    #[test]
    fn test_event_type_str_custom() {
        let event = OrpEvent::new(
            "e1".to_string(),
            "sensor".to_string(),
            EventPayload::Custom { data: json!(null) },
            "src".to_string(),
            0.5,
        );
        assert_eq!(event.event_type_str(), "custom");
    }

    // ── 17. With methods builder pattern ─────────────────────────────────────
    #[test]
    fn test_with_geo_builder() {
        let event = make_position_event()
            .with_geo(GeoPoint { lat: 1.0, lon: 2.0, alt: Some(3.0) });
        assert!(event.geo.is_some());
        let geo = event.geo.unwrap();
        assert!((geo.lat - 1.0).abs() < 0.01);
        assert_eq!(geo.alt, Some(3.0));
    }

    #[test]
    fn test_with_severity_builder() {
        let event = make_position_event().with_severity(EventSeverity::Warning);
        assert_eq!(event.severity, Some(EventSeverity::Warning));
    }

    // ── 18. JSON pretty ──────────────────────────────────────────────────────
    #[test]
    fn test_to_json_pretty() {
        let event = make_position_event();
        let pretty = event.to_json_pretty().unwrap();
        assert!(pretty.contains('\n'));
        assert!(pretty.contains("entity_id"));
    }

    // ── 19. Protobuf roundtrip (PropertyChange) ──────────────────────────────
    #[test]
    fn test_protobuf_roundtrip_property_change() {
        let event = OrpEvent::new(
            "e1".to_string(),
            "ship".to_string(),
            EventPayload::PropertyChange {
                key: "speed".to_string(),
                old_value: Some(json!(10)),
                new_value: json!(20),
                is_derived: true,
            },
            "src".to_string(),
            0.9,
        );
        let bytes = event.to_protobuf().unwrap();
        let back = OrpEvent::from_protobuf(&bytes).unwrap();
        if let EventPayload::PropertyChange { key, is_derived, .. } = &back.payload {
            assert_eq!(key, "speed");
            assert!(is_derived);
        } else {
            panic!("wrong variant");
        }
    }

    // ── 20. Protobuf roundtrip (StateTransition) ─────────────────────────────
    #[test]
    fn test_protobuf_roundtrip_state_transition() {
        let event = OrpEvent::new(
            "e1".to_string(),
            "ship".to_string(),
            EventPayload::StateTransition {
                from_state: "underway".to_string(),
                to_state: "moored".to_string(),
                reason: Some("arrived".to_string()),
            },
            "src".to_string(),
            0.9,
        );
        let bytes = event.to_protobuf().unwrap();
        let back = OrpEvent::from_protobuf(&bytes).unwrap();
        if let EventPayload::StateTransition { from_state, to_state, .. } = &back.payload {
            assert_eq!(from_state, "underway");
            assert_eq!(to_state, "moored");
        } else {
            panic!("wrong variant");
        }
    }

    // ── 21. Protobuf roundtrip (RelationshipChange) ──────────────────────────
    #[test]
    fn test_protobuf_roundtrip_relationship() {
        let mut props = HashMap::new();
        props.insert("eta".to_string(), json!("2026-04-01T12:00:00Z"));
        let event = OrpEvent::new(
            "e1".to_string(),
            "ship".to_string(),
            EventPayload::RelationshipChange {
                relationship_type: "heading_to".to_string(),
                target_entity_id: "port-1".to_string(),
                action: RelationshipAction::Updated,
                properties: props,
            },
            "src".to_string(),
            0.9,
        );
        let bytes = event.to_protobuf().unwrap();
        let back = OrpEvent::from_protobuf(&bytes).unwrap();
        if let EventPayload::RelationshipChange { action, .. } = &back.payload {
            assert_eq!(*action, RelationshipAction::Updated);
        } else {
            panic!("wrong variant");
        }
    }

    // ── 22. Protobuf roundtrip (Custom) ──────────────────────────────────────
    #[test]
    fn test_protobuf_roundtrip_custom() {
        let event = OrpEvent::new(
            "s1".to_string(),
            "sensor".to_string(),
            EventPayload::Custom { data: json!({"temp": 22.5}) },
            "mqtt".to_string(),
            0.7,
        );
        let bytes = event.to_protobuf().unwrap();
        let back = OrpEvent::from_protobuf(&bytes).unwrap();
        if let EventPayload::Custom { data } = &back.payload {
            assert_eq!(data["temp"], json!(22.5));
        } else {
            panic!("wrong variant");
        }
    }

    // ── 23. Entity with properties ───────────────────────────────────────────
    #[test]
    fn test_entity_with_properties() {
        let mut props = HashMap::new();
        props.insert("speed".to_string(), json!(15.0));
        props.insert("mmsi".to_string(), json!("123456789"));
        let entity = Entity {
            entity_id: "test-entity".to_string(),
            entity_type: "ship".to_string(),
            properties: props,
            ..Entity::default()
        };
        assert_eq!(entity.properties.len(), 2);
        assert_eq!(entity.properties["speed"], json!(15.0));
    }

    // ── 24. Relationship struct ──────────────────────────────────────────────
    #[test]
    fn test_relationship_struct() {
        let rel = Relationship {
            relationship_id: "r1".to_string(),
            source_entity_id: "ship-1".to_string(),
            target_entity_id: "port-1".to_string(),
            relationship_type: "docked_at".to_string(),
            properties: HashMap::new(),
            confidence: 0.95,
            is_active: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert_eq!(rel.relationship_type, "docked_at");
        assert!(rel.is_active);
    }

    // ── 25. DataSource struct ────────────────────────────────────────────────
    #[test]
    fn test_datasource_struct() {
        let ds = DataSource {
            source_id: "ais-1".to_string(),
            source_name: "AIS Feed".to_string(),
            source_type: "ais".to_string(),
            trust_score: 0.95,
            events_ingested: 1000,
            enabled: true,
        };
        assert!(ds.enabled);
        assert_eq!(ds.events_ingested, 1000);
    }
}
