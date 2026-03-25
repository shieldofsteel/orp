use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Canonical ORP Event — all events conform to this format
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrpEvent {
    pub event_id: String,
    pub entity_id: String,
    pub entity_type: String,
    pub event_type: String,
    pub event_timestamp: DateTime<Utc>,
    pub ingestion_timestamp: DateTime<Utc>,
    pub source_id: String,
    pub source_trust: f32,
    pub payload: EventPayload,
    pub confidence: f32,
    pub severity: Option<EventSeverity>,
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventPayload {
    PositionUpdate {
        latitude: f64,
        longitude: f64,
        altitude: Option<f64>,
        speed_knots: Option<f32>,
        heading_degrees: Option<f32>,
        course_degrees: Option<f32>,
    },
    PropertyChange {
        property_key: String,
        old_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
    },
    StateTransition {
        old_state: String,
        new_state: String,
        reason: Option<String>,
    },
    RelationshipChange {
        related_entity_id: String,
        relationship_type: String,
        added: bool,
        properties: HashMap<String, serde_json::Value>,
    },
    AlertTriggered {
        alert_rule_id: String,
        alert_rule_name: String,
        condition: String,
        message: String,
    },
    Custom {
        data: serde_json::Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    pub alt: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: String,
    pub entity_type: String,
    pub canonical_id: Option<String>,
    pub name: Option<String>,
    pub properties: HashMap<String, serde_json::Value>,
    pub confidence: f32,
    pub last_updated: DateTime<Utc>,
    pub geometry: Option<GeoPoint>,
    pub is_active: bool,
}

impl Default for Entity {
    fn default() -> Self {
        Self {
            entity_id: String::new(),
            entity_type: String::new(),
            canonical_id: None,
            name: None,
            properties: HashMap::new(),
            confidence: 1.0,
            last_updated: Utc::now(),
            geometry: None,
            is_active: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Relationship {
    pub relationship_id: String,
    pub source_entity_id: String,
    pub target_entity_id: String,
    pub relationship_type: String,
    pub properties: HashMap<String, serde_json::Value>,
    pub confidence: f32,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DataSource {
    pub source_id: String,
    pub source_name: String,
    pub source_type: String,
    pub trust_score: f32,
    pub events_ingested: u64,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StorageStats {
    pub total_entities: u64,
    pub total_events: u64,
    pub total_relationships: u64,
    pub database_size_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub entity_id: String,
    pub event_type: String,
    pub event_timestamp: DateTime<Utc>,
    pub source_id: String,
    pub data: serde_json::Value,
    pub confidence: f32,
}

impl OrpEvent {
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
        }
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_event() {
        let event = OrpEvent::new(
            "mmsi:123456789".to_string(),
            "ship".to_string(),
            "position_update".to_string(),
            EventPayload::PositionUpdate {
                latitude: 51.9225,
                longitude: 4.4792,
                altitude: None,
                speed_knots: Some(12.3),
                heading_degrees: Some(243.0),
                course_degrees: Some(245.0),
            },
            "ais-tap-01".to_string(),
            0.95,
        );

        assert_eq!(event.entity_id, "mmsi:123456789");
        assert_eq!(event.entity_type, "ship");
        assert!(!event.event_id.is_empty());

        let json = event.to_json().unwrap();
        assert!(json.contains("mmsi:123456789"));
    }

    #[test]
    fn test_entity_default() {
        let entity = Entity::default();
        assert!(entity.is_active);
        assert_eq!(entity.confidence, 1.0);
    }

    #[test]
    fn test_event_serialization_roundtrip() {
        let event = OrpEvent::new(
            "test-entity".to_string(),
            "ship".to_string(),
            "property_change".to_string(),
            EventPayload::PropertyChange {
                property_key: "speed".to_string(),
                old_value: Some(serde_json::json!(10.0)),
                new_value: serde_json::json!(15.0),
            },
            "source-1".to_string(),
            0.9,
        );

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: OrpEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.entity_id, event.entity_id);
    }
}
