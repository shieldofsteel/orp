use orp_proto::GeoPoint;

/// Haversine distance between two points in kilometers
pub fn haversine_km(p1: &GeoPoint, p2: &GeoPoint) -> f64 {
    let r = 6371.0; // Earth radius in km
    let dlat = (p2.lat - p1.lat).to_radians();
    let dlon = (p2.lon - p1.lon).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + p1.lat.to_radians().cos() * p2.lat.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    r * c
}

/// Check if a point is within a bounding box
pub fn point_in_bbox(point: &GeoPoint, min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> bool {
    point.lat >= min_lat && point.lat <= max_lat && point.lon >= min_lon && point.lon <= max_lon
}

/// Check if a point is within a radius of another point
pub fn point_in_radius(point: &GeoPoint, center: &GeoPoint, radius_km: f64) -> bool {
    haversine_km(point, center) <= radius_km
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_haversine() {
        let rotterdam = GeoPoint { lat: 51.9225, lon: 4.4792, alt: None };
        let amsterdam = GeoPoint { lat: 52.3676, lon: 4.9041, alt: None };
        let dist = haversine_km(&rotterdam, &amsterdam);
        assert!((dist - 57.0).abs() < 5.0, "Expected ~57km, got {}", dist);
    }

    #[test]
    fn test_point_in_bbox() {
        let point = GeoPoint { lat: 51.92, lon: 4.48, alt: None };
        assert!(point_in_bbox(&point, 50.0, 3.0, 53.0, 6.0));
        assert!(!point_in_bbox(&point, 52.0, 5.0, 53.0, 6.0));
    }
}
