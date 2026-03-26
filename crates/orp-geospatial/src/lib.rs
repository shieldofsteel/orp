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

/// Calculate bearing from point A to point B in degrees
pub fn bearing(from: &GeoPoint, to: &GeoPoint) -> f64 {
    let lat1 = from.lat.to_radians();
    let lat2 = to.lat.to_radians();
    let dlon = (to.lon - from.lon).to_radians();

    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();

    let bearing = y.atan2(x).to_degrees();
    (bearing + 360.0) % 360.0
}

/// Calculate a destination point given a start, bearing, and distance
pub fn destination_point(start: &GeoPoint, bearing_deg: f64, distance_km: f64) -> GeoPoint {
    let r = 6371.0;
    let d = distance_km / r;
    let brng = bearing_deg.to_radians();
    let lat1 = start.lat.to_radians();
    let lon1 = start.lon.to_radians();

    let lat2 = (lat1.sin() * d.cos() + lat1.cos() * d.sin() * brng.cos()).asin();
    let lon2 =
        lon1 + (brng.sin() * d.sin() * lat1.cos()).atan2(d.cos() - lat1.sin() * lat2.sin());

    GeoPoint {
        lat: lat2.to_degrees(),
        lon: lon2.to_degrees(),
        alt: start.alt,
    }
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
    fn test_haversine_same_point() {
        let point = GeoPoint { lat: 51.92, lon: 4.48, alt: None };
        let dist = haversine_km(&point, &point);
        assert!(dist.abs() < 0.001);
    }

    #[test]
    fn test_haversine_long_distance() {
        let london = GeoPoint { lat: 51.5074, lon: -0.1278, alt: None };
        let new_york = GeoPoint { lat: 40.7128, lon: -74.0060, alt: None };
        let dist = haversine_km(&london, &new_york);
        assert!((dist - 5570.0).abs() < 50.0, "Expected ~5570km, got {}", dist);
    }

    #[test]
    fn test_point_in_bbox() {
        let point = GeoPoint { lat: 51.92, lon: 4.48, alt: None };
        assert!(point_in_bbox(&point, 50.0, 3.0, 53.0, 6.0));
        assert!(!point_in_bbox(&point, 52.0, 5.0, 53.0, 6.0));
    }

    #[test]
    fn test_point_in_bbox_edge() {
        let point = GeoPoint { lat: 50.0, lon: 3.0, alt: None };
        assert!(point_in_bbox(&point, 50.0, 3.0, 53.0, 6.0));
    }

    #[test]
    fn test_point_in_radius() {
        let rotterdam = GeoPoint { lat: 51.9225, lon: 4.4792, alt: None };
        let nearby = GeoPoint { lat: 51.93, lon: 4.48, alt: None };
        let far = GeoPoint { lat: 35.0, lon: 139.0, alt: None };

        assert!(point_in_radius(&nearby, &rotterdam, 10.0));
        assert!(!point_in_radius(&far, &rotterdam, 100.0));
    }

    #[test]
    fn test_bearing() {
        let north = GeoPoint { lat: 0.0, lon: 0.0, alt: None };
        let east_point = GeoPoint { lat: 0.0, lon: 1.0, alt: None };
        let brg = bearing(&north, &east_point);
        assert!((brg - 90.0).abs() < 1.0, "Expected ~90°, got {}", brg);
    }

    #[test]
    fn test_destination_point() {
        let start = GeoPoint { lat: 51.92, lon: 4.48, alt: None };
        let dest = destination_point(&start, 0.0, 100.0); // 100km north
        assert!(dest.lat > start.lat);
        assert!((dest.lon - start.lon).abs() < 0.1);
    }

    #[test]
    fn test_destination_point_east() {
        let start = GeoPoint { lat: 0.0, lon: 0.0, alt: None };
        let dest = destination_point(&start, 90.0, 111.0); // ~1 degree east at equator
        assert!((dest.lat).abs() < 0.1);
        assert!((dest.lon - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_destination_point_zero_distance() {
        let start = GeoPoint { lat: 51.92, lon: 4.48, alt: None };
        let dest = destination_point(&start, 90.0, 0.0);
        assert!((dest.lat - start.lat).abs() < 0.001);
        assert!((dest.lon - start.lon).abs() < 0.001);
    }

    #[test]
    fn test_bearing_north() {
        let a = GeoPoint { lat: 0.0, lon: 0.0, alt: None };
        let b = GeoPoint { lat: 1.0, lon: 0.0, alt: None };
        let brg = bearing(&a, &b);
        assert!(brg.abs() < 1.0, "Expected ~0°, got {}", brg);
    }

    #[test]
    fn test_bearing_south() {
        let a = GeoPoint { lat: 1.0, lon: 0.0, alt: None };
        let b = GeoPoint { lat: 0.0, lon: 0.0, alt: None };
        let brg = bearing(&a, &b);
        assert!((brg - 180.0).abs() < 1.0, "Expected ~180°, got {}", brg);
    }

    #[test]
    fn test_bearing_west() {
        let a = GeoPoint { lat: 0.0, lon: 1.0, alt: None };
        let b = GeoPoint { lat: 0.0, lon: 0.0, alt: None };
        let brg = bearing(&a, &b);
        assert!((brg - 270.0).abs() < 1.0, "Expected ~270°, got {}", brg);
    }

    #[test]
    fn test_point_in_radius_exact_boundary() {
        let center = GeoPoint { lat: 0.0, lon: 0.0, alt: None };
        let on_boundary = destination_point(&center, 0.0, 10.0);
        assert!(point_in_radius(&on_boundary, &center, 10.1));
    }

    #[test]
    fn test_point_outside_bbox() {
        let point = GeoPoint { lat: 60.0, lon: 10.0, alt: None };
        assert!(!point_in_bbox(&point, 50.0, 3.0, 53.0, 6.0));
    }

    #[test]
    fn test_haversine_antipodal() {
        let a = GeoPoint { lat: 0.0, lon: 0.0, alt: None };
        let b = GeoPoint { lat: 0.0, lon: 180.0, alt: None };
        let dist = haversine_km(&a, &b);
        // Half Earth circumference ~20015 km
        assert!((dist - 20015.0).abs() < 50.0);
    }

    #[test]
    fn test_haversine_polar() {
        let north_pole = GeoPoint { lat: 90.0, lon: 0.0, alt: None };
        let south_pole = GeoPoint { lat: -90.0, lon: 0.0, alt: None };
        let dist = haversine_km(&north_pole, &south_pole);
        assert!((dist - 20015.0).abs() < 50.0);
    }

    #[test]
    fn test_point_preserves_altitude() {
        let start = GeoPoint { lat: 51.92, lon: 4.48, alt: Some(100.0) };
        let dest = destination_point(&start, 0.0, 10.0);
        assert_eq!(dest.alt, Some(100.0));
    }
}
