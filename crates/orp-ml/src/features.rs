//! Feature extractors that convert raw entity tracks into fixed-length
//! `Vec<f32>` suitable for [`crate::AnomalyScorer`] implementations.
//!
//! Today this module ships one extractor — [`extract_kinematic_features`] —
//! which produces a 6-feature kinematic descriptor of an entity's recent
//! behaviour. Future extractors (acoustic, RF, network-graph) will live
//! beside it under the same `features::` namespace.

use chrono::{DateTime, TimeZone, Timelike, Utc};

/// Number of features produced by [`extract_kinematic_features`].
pub const KINEMATIC_FEATURE_DIM: usize = 6;

/// Minimum history length required for kinematic feature extraction.
const MIN_HISTORY: usize = 5;

/// Extract a 6-feature kinematic descriptor from an entity's recent track.
///
/// Input is a slice of `(timestamp_secs, lat, lon, speed)` samples ordered
/// chronologically. Returns `None` if the history is shorter than 5 samples
/// — there isn't enough signal to compute meaningful jerk / dwell statistics.
///
/// Output features (in order):
///
/// 1. `speed_z` — z-score of the latest speed against the history mean / std.
/// 2. `heading_jerk` — mean absolute change-of-change in inter-sample
///    bearings, in degrees. High values indicate erratic manoeuvring.
/// 3. `dwell_ratio` — fraction of the last 4 samples whose great-circle
///    distance from the latest position is under 200 m. High values
///    indicate the entity is loitering.
/// 4. `hour_of_day_sin` — sin(2π · hour / 24) of the latest timestamp.
/// 5. `hour_of_day_cos` — cos(2π · hour / 24) of the latest timestamp.
///    The (sin, cos) pair encodes hour-of-day cyclically so a model
///    doesn't think 23:00 and 01:00 are 22 hours apart.
/// 6. `distance_to_centroid_km` — great-circle distance from the latest
///    position to the centroid of the historical positions.
pub fn extract_kinematic_features(
    _entity_id: &str,
    history: &[(f64, f64, f64, f64)],
) -> Option<Vec<f32>> {
    if history.len() < MIN_HISTORY {
        return None;
    }

    // 1) speed_z
    let speeds: Vec<f64> = history.iter().map(|(_, _, _, s)| *s).collect();
    let n = speeds.len() as f64;
    let mean = speeds.iter().sum::<f64>() / n;
    let var = speeds.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
    let std = var.sqrt();
    let latest_speed = *speeds.last()?;
    let speed_z = if std > 1e-6 {
        (latest_speed - mean) / std
    } else {
        0.0
    };

    // 2) heading_jerk: mean absolute delta-of-bearings across the track.
    let mut bearings: Vec<f64> = Vec::with_capacity(history.len().saturating_sub(1));
    for w in history.windows(2) {
        bearings.push(bearing_deg(w[0].1, w[0].2, w[1].1, w[1].2));
    }
    let jerk = if bearings.len() >= 2 {
        let mut sum = 0.0;
        for w in bearings.windows(2) {
            let d = angular_diff_deg(w[0], w[1]);
            sum += d.abs();
        }
        sum / (bearings.len() - 1) as f64
    } else {
        0.0
    };

    // 3) dwell_ratio: fraction of last-4 samples within 200 m of the latest.
    let (latest_lat, latest_lon) = (history.last()?.1, history.last()?.2);
    let recent: &[(f64, f64, f64, f64)] = if history.len() >= 5 {
        &history[history.len() - 5..history.len() - 1]
    } else {
        &history[..history.len() - 1]
    };
    let dwell_count = recent
        .iter()
        .filter(|(_, lat, lon, _)| haversine_km(*lat, *lon, latest_lat, latest_lon) < 0.2)
        .count();
    let dwell_ratio = if recent.is_empty() {
        0.0
    } else {
        dwell_count as f64 / recent.len() as f64
    };

    // 4–5) cyclical hour-of-day.
    let latest_ts = history.last()?.0;
    let hour = match Utc.timestamp_opt(latest_ts as i64, 0).single() {
        Some(dt) => dt.hour() as f64,
        None => 0.0,
    };
    let radians = (hour / 24.0) * 2.0 * std::f64::consts::PI;
    let hour_sin = radians.sin();
    let hour_cos = radians.cos();

    // 6) distance to centroid of historical positions.
    let cent_lat = history.iter().map(|(_, lat, _, _)| lat).sum::<f64>() / n;
    let cent_lon = history.iter().map(|(_, _, lon, _)| lon).sum::<f64>() / n;
    let dist_km = haversine_km(latest_lat, latest_lon, cent_lat, cent_lon);

    Some(vec![
        speed_z as f32,
        jerk as f32,
        dwell_ratio as f32,
        hour_sin as f32,
        hour_cos as f32,
        dist_km as f32,
    ])
}

/// Initial bearing from `(lat1, lon1)` to `(lat2, lon2)` in degrees [0, 360).
fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let y = dlon.sin() * phi2.cos();
    let x = phi1.cos() * phi2.sin() - phi1.sin() * phi2.cos() * dlon.cos();
    let theta = y.atan2(x).to_degrees();
    (theta + 360.0) % 360.0
}

/// Smallest unsigned angular difference in degrees, in [0, 180].
fn angular_diff_deg(a: f64, b: f64) -> f64 {
    let d = ((a - b).abs()) % 360.0;
    if d > 180.0 {
        360.0 - d
    } else {
        d
    }
}

/// Great-circle distance in kilometres between two WGS-84 points.
fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R_KM: f64 = 6371.0088;
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * R_KM * a.sqrt().asin()
}

/// Cast a UTC `DateTime<Utc>` to a `f64` second timestamp — convenience
/// for callers that already have `chrono` types in hand.
pub fn ts_secs(dt: DateTime<Utc>) -> f64 {
    dt.timestamp() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_short_history_returns_none() {
        let history = vec![
            (1_700_000_000.0, 51.0, 4.0, 10.0),
            (1_700_000_060.0, 51.001, 4.0, 10.0),
        ];
        assert!(extract_kinematic_features("e1", &history).is_none());
    }

    #[test]
    fn constant_speed_history_has_low_speed_z() {
        // Constant speed and constant position => z=0, jerk=0, dwell=1.
        let history: Vec<(f64, f64, f64, f64)> = (0..10)
            .map(|i| (1_700_000_000.0 + (i as f64) * 60.0, 51.0, 4.0, 12.5))
            .collect();
        let feats = extract_kinematic_features("e", &history).unwrap();
        assert_eq!(feats.len(), KINEMATIC_FEATURE_DIM);
        assert!(feats[0].abs() < 0.01, "speed_z should be ~0, got {}", feats[0]);
    }

    #[test]
    fn manoeuvring_history_has_elevated_speed_z() {
        // Nine baseline samples at 10 knots, then a final sample at 50 knots.
        let mut history: Vec<(f64, f64, f64, f64)> = (0..9)
            .map(|i| (1_700_000_000.0 + (i as f64) * 60.0, 51.0, 4.0, 10.0))
            .collect();
        history.push((1_700_000_540.0, 51.001, 4.001, 50.0));
        let feats = extract_kinematic_features("e", &history).unwrap();
        assert!(
            feats[0].abs() > 1.5,
            "speed_z should be elevated, got {}",
            feats[0]
        );
    }

    #[test]
    fn moving_history_has_low_dwell_ratio() {
        // Each sample is ~1 km north of the previous — nothing should dwell.
        let history: Vec<(f64, f64, f64, f64)> = (0..6)
            .map(|i| {
                (
                    1_700_000_000.0 + (i as f64) * 60.0,
                    51.0 + (i as f64) * 0.01,
                    4.0,
                    20.0,
                )
            })
            .collect();
        let feats = extract_kinematic_features("e", &history).unwrap();
        // Index 2 is dwell_ratio.
        assert!(feats[2] < 0.5, "dwell_ratio should be low, got {}", feats[2]);
    }

    #[test]
    fn cyclical_hour_features_are_unit_circle() {
        let history: Vec<(f64, f64, f64, f64)> = (0..6)
            .map(|i| (1_700_000_000.0 + (i as f64) * 60.0, 51.0, 4.0, 10.0))
            .collect();
        let feats = extract_kinematic_features("e", &history).unwrap();
        let s = feats[3];
        let c = feats[4];
        let r = (s * s + c * c).sqrt();
        assert!((r - 1.0).abs() < 1e-4, "sin^2+cos^2 should be 1, got {}", r);
    }

    #[test]
    fn haversine_zero_for_same_point() {
        assert!(haversine_km(51.0, 4.0, 51.0, 4.0).abs() < 1e-9);
    }

    #[test]
    fn bearing_north_is_zero() {
        let b = bearing_deg(0.0, 0.0, 1.0, 0.0);
        assert!(b.abs() < 1e-6 || (b - 360.0).abs() < 1e-6);
    }

    #[test]
    fn test_extract_handles_identical_points() {
        // Six samples at the same location, same speed, only timestamp differs.
        let history: Vec<(f64, f64, f64, f64)> = (0..6)
            .map(|i| (1_700_000_000.0 + (i as f64) * 60.0, 51.0, 4.0, 12.5))
            .collect();
        let feats = extract_kinematic_features("e", &history).unwrap();
        assert_eq!(feats.len(), KINEMATIC_FEATURE_DIM);
        // speed_z must be exactly 0: zero variance triggers the std-guard.
        assert_eq!(feats[0], 0.0, "speed_z should be 0 for constant speed");
        // distance_to_centroid_km must be ~0: every point is the centroid.
        assert!(
            feats[5].abs() < 1e-3,
            "distance_to_centroid_km should be ~0, got {}",
            feats[5]
        );
        // All features must be finite.
        for (i, v) in feats.iter().enumerate() {
            assert!(v.is_finite(), "feature {} not finite: {}", i, v);
        }
    }

    #[test]
    fn test_extract_handles_antipodal_coordinates() {
        // Alternate between (0, 0) and (0, 180) — antipodal points; the
        // great-circle distance is ~half the Earth's circumference (~20015 km).
        let history: Vec<(f64, f64, f64, f64)> = (0..6)
            .map(|i| {
                let lon = if i % 2 == 0 { 0.0 } else { 180.0 };
                (1_700_000_000.0 + (i as f64) * 60.0, 0.0, lon, 10.0)
            })
            .collect();
        // Sanity-check our setup: the haversine between antipodal lon points
        // at the equator is roughly half the Earth's circumference.
        let d = haversine_km(0.0, 0.0, 0.0, 180.0);
        assert!(
            (d - 20015.0).abs() < 50.0,
            "antipodal haversine should be ~20015 km, got {}",
            d
        );
        // The extractor must not panic and must return finite values.
        let feats = extract_kinematic_features("e", &history).unwrap();
        assert_eq!(feats.len(), KINEMATIC_FEATURE_DIM);
        for (i, v) in feats.iter().enumerate() {
            assert!(v.is_finite(), "feature {} not finite: {}", i, v);
        }
    }

    #[test]
    fn test_extract_handles_min_history_boundary() {
        // Exactly MIN_HISTORY samples -> Some.
        let exact: Vec<(f64, f64, f64, f64)> = (0..MIN_HISTORY)
            .map(|i| (1_700_000_000.0 + (i as f64) * 60.0, 51.0, 4.0, 10.0))
            .collect();
        assert!(
            extract_kinematic_features("e", &exact).is_some(),
            "MIN_HISTORY samples should produce features"
        );
        // One less than MIN_HISTORY -> None.
        let short: Vec<(f64, f64, f64, f64)> = (0..MIN_HISTORY - 1)
            .map(|i| (1_700_000_000.0 + (i as f64) * 60.0, 51.0, 4.0, 10.0))
            .collect();
        assert!(
            extract_kinematic_features("e", &short).is_none(),
            "MIN_HISTORY-1 samples should return None"
        );
    }

    #[test]
    fn test_extract_handles_negative_unix_time() {
        // Pre-epoch timestamps must still produce a sensible hour-of-day.
        let history: Vec<(f64, f64, f64, f64)> = (0..6)
            .map(|i| (-3600.0 + (i as f64) * 60.0, 51.0, 4.0, 10.0))
            .collect();
        let feats = extract_kinematic_features("e", &history).unwrap();
        let s = feats[3];
        let c = feats[4];
        assert!((-1.0..=1.0).contains(&s), "hour_sin must be in [-1, 1], got {}", s);
        assert!((-1.0..=1.0).contains(&c), "hour_cos must be in [-1, 1], got {}", c);
        for (i, v) in feats.iter().enumerate() {
            assert!(v.is_finite(), "feature {} not finite: {}", i, v);
        }
    }

    #[test]
    fn test_extract_handles_far_future_time() {
        // 1e18 seconds is past chrono's representable range; the extractor
        // must not panic and must return finite features (the hour-of-day
        // path falls back to 0 when chrono refuses to convert).
        let history: Vec<(f64, f64, f64, f64)> = (0..6)
            .map(|i| (1.0e18 + (i as f64) * 60.0, 51.0, 4.0, 10.0))
            .collect();
        let result = extract_kinematic_features("e", &history);
        // Either Some(finite) or None; never panic, never NaN.
        if let Some(feats) = result {
            assert_eq!(feats.len(), KINEMATIC_FEATURE_DIM);
            for (i, v) in feats.iter().enumerate() {
                assert!(v.is_finite(), "feature {} not finite: {}", i, v);
            }
        }
    }
}
