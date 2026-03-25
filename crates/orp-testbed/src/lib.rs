use orp_proto::{Entity, Event, GeoPoint, Relationship};
use std::collections::HashMap;

/// Generate synthetic ship entities for testing
pub fn generate_synthetic_ships(count: usize) -> Vec<Entity> {
    let ship_names = [
        "Maersk Seatrade",
        "Rotterdam Express",
        "Ever Given",
        "Pacific Pioneer",
        "Atlantic Breeze",
        "Northern Star",
        "Southern Cross",
        "Eastern Wind",
        "Western Horizon",
        "Ocean Voyager",
    ];

    let mut ships = Vec::with_capacity(count);
    for i in 0..count {
        let lat = 50.0 + (i as f64 % 100.0) * 0.05;
        let lon = 2.0 + (i as f64 % 80.0) * 0.05;
        let speed = 5.0 + (i as f64 % 20.0);
        let course = (i as f64 * 37.0) % 360.0;

        let mut properties = HashMap::new();
        properties.insert(
            "mmsi".to_string(),
            serde_json::json!(format!("{:09}", 200000000 + i)),
        );
        properties.insert("speed".to_string(), serde_json::json!(speed));
        properties.insert("course".to_string(), serde_json::json!(course));
        properties.insert("heading".to_string(), serde_json::json!(course));
        properties.insert(
            "ship_type".to_string(),
            serde_json::json!(match i % 4 {
                0 => "container",
                1 => "tanker",
                2 => "bulk",
                _ => "general",
            }),
        );
        properties.insert(
            "flag".to_string(),
            serde_json::json!(match i % 5 {
                0 => "NL",
                1 => "DE",
                2 => "UK",
                3 => "FR",
                _ => "US",
            }),
        );

        ships.push(Entity {
            entity_id: format!("ship-{}", i),
            entity_type: "Ship".to_string(),
            name: Some(
                ship_names
                    .get(i % ship_names.len())
                    .map(|s| format!("{} {}", s, i))
                    .unwrap_or_else(|| format!("Ship {}", i)),
            ),
            geometry: Some(GeoPoint {
                lat,
                lon,
                alt: None,
            }),
            properties,
            ..Entity::default()
        });
    }
    ships
}

/// Generate synthetic port entities for testing
pub fn generate_synthetic_ports(count: usize) -> Vec<Entity> {
    let port_data = vec![
        ("Rotterdam", 51.9225, 4.4792, "NL", 14500),
        ("Antwerp", 51.2194, 4.4025, "BE", 12000),
        ("Hamburg", 53.5511, 9.9937, "DE", 8900),
        ("Amsterdam", 52.3676, 4.9041, "NL", 5500),
        ("London", 51.5074, -0.1278, "UK", 7000),
        ("Marseille", 43.2965, 5.3698, "FR", 4500),
        ("Barcelona", 41.3851, 2.1734, "ES", 3200),
        ("Genoa", 44.4056, 8.9463, "IT", 2800),
        ("Piraeus", 37.9485, 23.6428, "GR", 5100),
        ("Istanbul", 41.0082, 28.9784, "TR", 4700),
    ];

    port_data
        .iter()
        .take(count)
        .enumerate()
        .map(|(i, (name, lat, lon, country, capacity))| {
            let mut properties = HashMap::new();
            properties.insert("country".to_string(), serde_json::json!(country));
            properties.insert("capacity".to_string(), serde_json::json!(capacity));
            properties.insert(
                "congestion".to_string(),
                serde_json::json!((i as f64 * 0.1) % 1.0),
            );

            Entity {
                entity_id: format!("port-{}", name.to_lowercase().replace(' ', "-")),
                entity_type: "Port".to_string(),
                name: Some(name.to_string()),
                geometry: Some(GeoPoint {
                    lat: *lat,
                    lon: *lon,
                    alt: None,
                }),
                properties,
                ..Entity::default()
            }
        })
        .collect()
}

/// Generate synthetic aircraft entities for testing
pub fn generate_synthetic_aircraft(count: usize) -> Vec<Entity> {
    let airline_data = [("A1B2C3", "KLM1234", 52.30, 4.76, 35000.0, 450.0),
        ("D4E5F6", "BAW456", 51.47, -0.46, 28000.0, 380.0),
        ("789ABC", "AFR789", 48.86, 2.35, 40000.0, 500.0),
        ("DEF012", "DLH321", 50.03, 8.57, 32000.0, 420.0),
        ("345678", "AAL100", 40.64, -73.78, 15000.0, 250.0)];

    airline_data
        .iter()
        .take(count)
        .enumerate()
        .map(|(i, (icao, callsign, lat, lon, alt, speed))| {
            let mut properties = HashMap::new();
            properties.insert("icao".to_string(), serde_json::json!(icao));
            properties.insert("callsign".to_string(), serde_json::json!(callsign));
            properties.insert("altitude".to_string(), serde_json::json!(alt));
            properties.insert("speed".to_string(), serde_json::json!(speed));
            properties.insert("heading".to_string(), serde_json::json!((i as f64 * 72.0) % 360.0));
            properties.insert("on_ground".to_string(), serde_json::json!(false));

            Entity {
                entity_id: format!("icao:{}", icao),
                entity_type: "Aircraft".to_string(),
                name: Some(callsign.to_string()),
                geometry: Some(GeoPoint {
                    lat: *lat,
                    lon: *lon,
                    alt: Some(*alt * 0.3048), // feet to meters
                }),
                properties,
                ..Entity::default()
            }
        })
        .collect()
}

/// Generate synthetic weather system entities
pub fn generate_synthetic_weather(count: usize) -> Vec<Entity> {
    let weather_data = [("storm-atlantic-1", "Tropical Storm Alpha", 45.0, -30.0, "tropical_storm", "warning", 200.0),
        ("low-north-sea", "North Sea Low", 58.0, 5.0, "low_pressure", "info", 500.0),
        ("high-med", "Mediterranean High", 38.0, 15.0, "high_pressure", "info", 800.0),
        ("storm-channel", "Channel Storm", 50.0, -2.0, "storm", "critical", 150.0)];

    weather_data
        .iter()
        .take(count)
        .map(|(id, name, lat, lon, sys_type, severity, radius)| {
            let mut properties = HashMap::new();
            properties.insert("system_type".to_string(), serde_json::json!(sys_type));
            properties.insert("severity".to_string(), serde_json::json!(severity));
            properties.insert("radius_km".to_string(), serde_json::json!(radius));

            Entity {
                entity_id: format!("weather:{}", id),
                entity_type: "WeatherSystem".to_string(),
                name: Some(name.to_string()),
                geometry: Some(GeoPoint {
                    lat: *lat,
                    lon: *lon,
                    alt: None,
                }),
                properties,
                ..Entity::default()
            }
        })
        .collect()
}

/// Generate synthetic relationships between ships and ports
pub fn generate_synthetic_relationships(
    ships: &[Entity],
    ports: &[Entity],
) -> Vec<Relationship> {
    let mut relationships = Vec::new();
    let mut rel_id = 0;

    for (i, ship) in ships.iter().enumerate() {
        if let Some(port) = ports.get(i % ports.len()) {
            rel_id += 1;
            relationships.push(Relationship {
                relationship_id: format!("rel-{}", rel_id),
                source_entity_id: ship.entity_id.clone(),
                target_entity_id: port.entity_id.clone(),
                relationship_type: if i % 2 == 0 {
                    "HEADING_TO".to_string()
                } else {
                    "DOCKED_AT".to_string()
                },
                properties: HashMap::new(),
                confidence: 0.9,
                is_active: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            });
        }
    }

    relationships
}

/// Generate synthetic events for testing
pub fn generate_synthetic_events(entity_id: &str, count: usize) -> Vec<Event> {
    let mut events = Vec::with_capacity(count);
    for i in 0..count {
        events.push(Event {
            event_id: format!("evt-{}-{}", entity_id, i),
            entity_id: entity_id.to_string(),
            event_type: if i % 3 == 0 {
                "position_update".to_string()
            } else if i % 3 == 1 {
                "property_change".to_string()
            } else {
                "state_transition".to_string()
            },
            event_timestamp: chrono::Utc::now()
                - chrono::Duration::seconds((count - i) as i64 * 60),
            source_id: "test-source".to_string(),
            data: serde_json::json!({
                "speed": 10.0 + i as f64,
                "heading": (i as f64 * 36.0) % 360.0,
            }),
            confidence: 0.95,
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_ships() {
        let ships = generate_synthetic_ships(50);
        assert_eq!(ships.len(), 50);
        for ship in &ships {
            assert_eq!(ship.entity_type, "Ship");
            assert!(ship.geometry.is_some());
            assert!(ship.properties.contains_key("mmsi"));
            assert!(ship.properties.contains_key("speed"));
        }
    }

    #[test]
    fn test_generate_ports() {
        let ports = generate_synthetic_ports(5);
        assert_eq!(ports.len(), 5);
        for port in &ports {
            assert_eq!(port.entity_type, "Port");
            assert!(port.name.is_some());
            assert!(port.properties.contains_key("country"));
        }
    }

    #[test]
    fn test_generate_aircraft() {
        let aircraft = generate_synthetic_aircraft(3);
        assert_eq!(aircraft.len(), 3);
        for ac in &aircraft {
            assert_eq!(ac.entity_type, "Aircraft");
            assert!(ac.properties.contains_key("icao"));
        }
    }

    #[test]
    fn test_generate_weather() {
        let weather = generate_synthetic_weather(2);
        assert_eq!(weather.len(), 2);
        for w in &weather {
            assert_eq!(w.entity_type, "WeatherSystem");
            assert!(w.properties.contains_key("system_type"));
        }
    }

    #[test]
    fn test_generate_relationships() {
        let ships = generate_synthetic_ships(5);
        let ports = generate_synthetic_ports(3);
        let rels = generate_synthetic_relationships(&ships, &ports);
        assert_eq!(rels.len(), 5);
        for rel in &rels {
            assert!(
                rel.relationship_type == "HEADING_TO"
                    || rel.relationship_type == "DOCKED_AT"
            );
        }
    }

    #[test]
    fn test_generate_events() {
        let events = generate_synthetic_events("ship-1", 10);
        assert_eq!(events.len(), 10);
        for event in &events {
            assert_eq!(event.entity_id, "ship-1");
            assert!(!event.event_id.is_empty());
        }
    }
}
