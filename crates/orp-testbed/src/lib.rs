use orp_proto::{Entity, GeoPoint};
use std::collections::HashMap;

/// Generate synthetic ship entities for testing
pub fn generate_synthetic_ships(count: usize) -> Vec<Entity> {
    let mut ships = Vec::with_capacity(count);
    for i in 0..count {
        let lat = 50.0 + (i as f64 % 100.0) * 0.05;
        let lon = 2.0 + (i as f64 % 80.0) * 0.05;
        let speed = 5.0 + (i as f64 % 20.0);
        let course = (i as f64 * 37.0) % 360.0;

        let mut properties = HashMap::new();
        properties.insert("mmsi".to_string(), serde_json::json!(format!("{:09}", 200000000 + i)));
        properties.insert("speed".to_string(), serde_json::json!(speed));
        properties.insert("course".to_string(), serde_json::json!(course));
        properties.insert("heading".to_string(), serde_json::json!(course));
        properties.insert("ship_type".to_string(), serde_json::json!(
            match i % 4 {
                0 => "container",
                1 => "tanker",
                2 => "bulk",
                _ => "general",
            }
        ));

        ships.push(Entity {
            entity_id: format!("ship-{}", i),
            entity_type: "Ship".to_string(),
            name: Some(format!("Ship {}", i)),
            geometry: Some(GeoPoint { lat, lon, alt: None }),
            properties,
            ..Entity::default()
        });
    }
    ships
}

/// Generate synthetic port entities for testing
pub fn generate_synthetic_ports(count: usize) -> Vec<Entity> {
    let port_data = vec![
        ("Rotterdam", 51.9225, 4.4792),
        ("Antwerp", 51.2194, 4.4025),
        ("Hamburg", 53.5511, 9.9937),
        ("Amsterdam", 52.3676, 4.9041),
        ("London", 51.5074, -0.1278),
        ("Marseille", 43.2965, 5.3698),
        ("Barcelona", 41.3851, 2.1734),
        ("Genoa", 44.4056, 8.9463),
        ("Piraeus", 37.9485, 23.6428),
        ("Istanbul", 41.0082, 28.9784),
    ];

    port_data
        .iter()
        .take(count)
        .enumerate()
        .map(|(i, (name, lat, lon))| {
            let mut properties = HashMap::new();
            properties.insert("country".to_string(), serde_json::json!("EU"));
            properties.insert("capacity".to_string(), serde_json::json!(10000 + i * 1000));

            Entity {
                entity_id: format!("port-{}", name.to_lowercase().replace(' ', "-")),
                entity_type: "Port".to_string(),
                name: Some(name.to_string()),
                geometry: Some(GeoPoint { lat: *lat, lon: *lon, alt: None }),
                properties,
                ..Entity::default()
            }
        })
        .collect()
}
