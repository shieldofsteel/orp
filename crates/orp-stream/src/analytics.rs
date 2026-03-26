//! Real-time analytics engine — Palantir-grade situational awareness.
//!
//! Implements:
//! - CPA (Closest Point of Approach)
//! - Speed / course change detection
//! - Zone entry/exit detection
//! - Dwell detection
//! - Pattern of life (behavioral baseline)
//! - Anomaly scoring (0–100)
//! - Dark target detection (AIS gap / transmission loss)

use chrono::{DateTime, Duration, Timelike, Utc};
use orp_proto::GeoPoint;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

// ── Geo helpers ───────────────────────────────────────────────────────────────

fn haversine_km(a: &GeoPoint, b: &GeoPoint) -> f64 {
    let r = 6371.0_f64;
    let dlat = (b.lat - a.lat).to_radians();
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2)
        + a.lat.to_radians().cos() * b.lat.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    r * 2.0 * h.sqrt().asin()
}

#[allow(dead_code)]
fn bearing_deg(from: &GeoPoint, to: &GeoPoint) -> f64 {
    let lat1 = from.lat.to_radians();
    let lat2 = to.lat.to_radians();
    let dlon = (to.lon - from.lon).to_radians();
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// Angular difference between two headings (0–180°).
fn heading_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    if d > 180.0 { 360.0 - d } else { d }
}

// ── Entity track ──────────────────────────────────────────────────────────────

/// A single recorded position/state sample for an entity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackPoint {
    pub timestamp: DateTime<Utc>,
    pub position: GeoPoint,
    pub speed_knots: f64,
    pub course_degrees: f64,
}

/// Rolling track history for one entity (capped at `max_len`).
#[derive(Clone, Debug)]
pub struct EntityTrack {
    pub entity_id: String,
    pub points: VecDeque<TrackPoint>,
    pub max_len: usize,
    /// Timestamp of last received transmission (`None` = never seen).
    pub last_seen: Option<DateTime<Utc>>,
}

impl EntityTrack {
    pub fn new(entity_id: impl Into<String>, max_len: usize) -> Self {
        Self {
            entity_id: entity_id.into(),
            points: VecDeque::with_capacity(max_len),
            max_len,
            last_seen: None,
        }
    }

    pub fn push(&mut self, pt: TrackPoint) {
        self.last_seen = Some(pt.timestamp);
        if self.points.len() == self.max_len {
            self.points.pop_front();
        }
        self.points.push_back(pt);
    }

    pub fn latest(&self) -> Option<&TrackPoint> {
        self.points.back()
    }
}

// ── CPA ───────────────────────────────────────────────────────────────────────

/// Result of a Closest Point of Approach calculation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CpaResult {
    pub entity_a: String,
    pub entity_b: String,
    /// Time-to-CPA in minutes from `calculated_at`.
    pub tcpa_minutes: f64,
    /// Distance at CPA in nautical miles.
    pub dcpa_nm: f64,
    /// Projected position of A at CPA.
    pub position_a_at_cpa: GeoPoint,
    /// Projected position of B at CPA.
    pub position_b_at_cpa: GeoPoint,
    pub calculated_at: DateTime<Utc>,
}

impl CpaResult {
    /// Whether the CPA is considered a collision risk (DCPA < threshold, TCPA in future).
    pub fn is_collision_risk(&self, dcpa_threshold_nm: f64) -> bool {
        self.tcpa_minutes > 0.0 && self.dcpa_nm < dcpa_threshold_nm
    }
}

/// Project a GeoPoint forward by `minutes` at `speed_knots` on `course_degrees`.
fn project_position(pos: &GeoPoint, speed_knots: f64, course_deg: f64, minutes: f64) -> GeoPoint {
    // 1 knot = 1 nm/hr; distance in km = speed_knots * (minutes/60) * 1.852
    let distance_km = speed_knots * (minutes / 60.0) * 1.852;
    let r = 6371.0_f64;
    let d = distance_km / r;
    let brng = course_deg.to_radians();
    let lat1 = pos.lat.to_radians();
    let lon1 = pos.lon.to_radians();
    let lat2 = (lat1.sin() * d.cos() + lat1.cos() * d.sin() * brng.cos()).asin();
    let lon2 = lon1 + (brng.sin() * d.sin() * lat1.cos()).atan2(d.cos() - lat1.sin() * lat2.sin());
    GeoPoint { lat: lat2.to_degrees(), lon: lon2.to_degrees(), alt: pos.alt }
}

fn km_to_nm(km: f64) -> f64 {
    km / 1.852
}

/// Calculate the Closest Point of Approach for two entities.
/// Uses linear projection — valid for short time horizons (~30 min).
#[allow(clippy::too_many_arguments)]
pub fn calculate_cpa(
    entity_a: &str,
    pos_a: &GeoPoint,
    speed_a_knots: f64,
    course_a: f64,
    entity_b: &str,
    pos_b: &GeoPoint,
    speed_b_knots: f64,
    course_b: f64,
    horizon_minutes: f64,
) -> CpaResult {
    let now = Utc::now();
    let step = 0.5_f64; // sample every 30 seconds
    let steps = (horizon_minutes / step) as usize;

    let mut min_dist = f64::MAX;
    let mut best_t = 0.0_f64;
    let mut best_a = pos_a.clone();
    let mut best_b = pos_b.clone();

    for i in 0..=steps {
        let t = i as f64 * step;
        let pa = project_position(pos_a, speed_a_knots, course_a, t);
        let pb = project_position(pos_b, speed_b_knots, course_b, t);
        let dist = haversine_km(&pa, &pb);
        if dist < min_dist {
            min_dist = dist;
            best_t = t;
            best_a = pa;
            best_b = pb;
        }
    }

    CpaResult {
        entity_a: entity_a.to_string(),
        entity_b: entity_b.to_string(),
        tcpa_minutes: best_t,
        dcpa_nm: km_to_nm(min_dist),
        position_a_at_cpa: best_a,
        position_b_at_cpa: best_b,
        calculated_at: now,
    }
}

// ── Speed / course change detection ──────────────────────────────────────────

/// Alert produced when a significant manoeuvre is detected.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManoeuvreAlert {
    pub entity_id: String,
    pub alert_type: ManoeuvreType,
    pub old_value: f64,
    pub new_value: f64,
    pub change_pct: f64,
    pub detected_at: DateTime<Utc>,
    pub position: GeoPoint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManoeuvreType {
    /// Speed changed by more than `threshold`% within 5 minutes.
    SpeedChange,
    /// Course changed by more than 30° within 5 minutes.
    CourseChange,
}

/// Analyse the last 5 minutes of track and detect sudden manoeuvres.
pub fn detect_manoeuvres(
    entity_id: &str,
    track: &EntityTrack,
    speed_threshold_pct: f64,
    course_threshold_deg: f64,
    window_minutes: i64,
) -> Vec<ManoeuvreAlert> {
    let mut alerts = Vec::new();
    let cutoff = Utc::now() - Duration::minutes(window_minutes);

    // Collect points within window
    let window: Vec<&TrackPoint> = track
        .points
        .iter()
        .filter(|p| p.timestamp >= cutoff)
        .collect();

    if window.len() < 2 {
        return alerts;
    }

    let oldest = window.first().unwrap();
    let newest = window.last().unwrap();

    // Speed change check
    let old_speed = oldest.speed_knots;
    let new_speed = newest.speed_knots;
    if old_speed > 0.1 {
        let change_pct = ((new_speed - old_speed) / old_speed).abs() * 100.0;
        if change_pct > speed_threshold_pct {
            alerts.push(ManoeuvreAlert {
                entity_id: entity_id.to_string(),
                alert_type: ManoeuvreType::SpeedChange,
                old_value: old_speed,
                new_value: new_speed,
                change_pct,
                detected_at: newest.timestamp,
                position: newest.position.clone(),
            });
        }
    }

    // Course change check
    let course_delta = heading_diff(oldest.course_degrees, newest.course_degrees);
    if course_delta > course_threshold_deg {
        alerts.push(ManoeuvreAlert {
            entity_id: entity_id.to_string(),
            alert_type: ManoeuvreType::CourseChange,
            old_value: oldest.course_degrees,
            new_value: newest.course_degrees,
            change_pct: course_delta,
            detected_at: newest.timestamp,
            position: newest.position.clone(),
        });
    }

    alerts
}

// ── Zone entry/exit ───────────────────────────────────────────────────────────

/// A geographic zone defined as a polygon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Zone {
    pub zone_id: String,
    pub name: String,
    /// Polygon vertices (closed ring — first == last not required).
    pub polygon: Vec<GeoPoint>,
}

impl Zone {
    /// Point-in-polygon test using ray casting.
    pub fn contains(&self, point: &GeoPoint) -> bool {
        let n = self.polygon.len();
        if n < 3 {
            return false;
        }
        let (px, py) = (point.lon, point.lat);
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = (self.polygon[i].lon, self.polygon[i].lat);
            let (xj, yj) = (self.polygon[j].lon, self.polygon[j].lat);
            if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneEventType {
    Entry,
    Exit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZoneEvent {
    pub entity_id: String,
    pub zone_id: String,
    pub zone_name: String,
    pub event_type: ZoneEventType,
    pub position: GeoPoint,
    pub timestamp: DateTime<Utc>,
}

/// Zone tracker — remembers which zones each entity was inside last sample.
#[derive(Default)]
pub struct ZoneTracker {
    /// entity_id → set of zone_ids currently inside
    state: HashMap<String, std::collections::HashSet<String>>,
}

impl ZoneTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the tracker with a new position and return any entry/exit events.
    pub fn update(
        &mut self,
        entity_id: &str,
        position: &GeoPoint,
        zones: &[Zone],
        timestamp: DateTime<Utc>,
    ) -> Vec<ZoneEvent> {
        let mut events = Vec::new();
        let prev_inside = self
            .state
            .entry(entity_id.to_string())
            .or_default();

        let mut now_inside = std::collections::HashSet::new();

        for zone in zones {
            if zone.contains(position) {
                now_inside.insert(zone.zone_id.clone());
                if !prev_inside.contains(&zone.zone_id) {
                    events.push(ZoneEvent {
                        entity_id: entity_id.to_string(),
                        zone_id: zone.zone_id.clone(),
                        zone_name: zone.name.clone(),
                        event_type: ZoneEventType::Entry,
                        position: position.clone(),
                        timestamp,
                    });
                }
            }
        }

        for prev_zone_id in prev_inside.iter() {
            if !now_inside.contains(prev_zone_id) {
                // Find zone name
                let zone_name = zones
                    .iter()
                    .find(|z| &z.zone_id == prev_zone_id)
                    .map(|z| z.name.clone())
                    .unwrap_or_default();
                events.push(ZoneEvent {
                    entity_id: entity_id.to_string(),
                    zone_id: prev_zone_id.clone(),
                    zone_name,
                    event_type: ZoneEventType::Exit,
                    position: position.clone(),
                    timestamp,
                });
            }
        }

        *prev_inside = now_inside;
        events
    }
}

// ── Dwell detection ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DwellAlert {
    pub entity_id: String,
    pub center: GeoPoint,
    pub radius_km: f64,
    pub dwell_start: DateTime<Utc>,
    pub dwell_duration_minutes: f64,
    pub threshold_minutes: f64,
}

/// Detect if an entity has dwelled in a small area beyond a time threshold.
pub fn detect_dwell(
    entity_id: &str,
    track: &EntityTrack,
    radius_km: f64,
    threshold_minutes: f64,
) -> Option<DwellAlert> {
    if track.points.is_empty() {
        return None;
    }

    let latest = track.latest()?;
    let center = &latest.position;

    // Find the earliest point within `radius_km` of the latest position
    // that forms a continuous dwell segment (scan backwards)
    let mut dwell_start = latest.timestamp;

    for pt in track.points.iter().rev() {
        if haversine_km(center, &pt.position) <= radius_km {
            dwell_start = pt.timestamp;
        } else {
            break;
        }
    }

    let dwell_minutes = (latest.timestamp - dwell_start).num_seconds() as f64 / 60.0;

    if dwell_minutes >= threshold_minutes {
        Some(DwellAlert {
            entity_id: entity_id.to_string(),
            center: center.clone(),
            radius_km,
            dwell_start,
            dwell_duration_minutes: dwell_minutes,
            threshold_minutes,
        })
    } else {
        None
    }
}

// ── Pattern of life ───────────────────────────────────────────────────────────

/// Behavioral baseline built from historical observations.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PatternOfLife {
    pub entity_id: String,
    /// Mean speed (knots).
    pub mean_speed_knots: f64,
    /// Standard deviation of speed.
    pub std_speed_knots: f64,
    /// Most frequent operating area — bounding box.
    pub typical_bbox: Option<BoundingBox>,
    /// Typical operating hours (UTC), 0–23.
    pub typical_hours: Vec<u8>,
    /// Total observations used to build baseline.
    pub sample_count: usize,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

impl BoundingBox {
    pub fn contains(&self, p: &GeoPoint) -> bool {
        p.lat >= self.min_lat
            && p.lat <= self.max_lat
            && p.lon >= self.min_lon
            && p.lon <= self.max_lon
    }
}

impl PatternOfLife {
    /// Build a pattern from a track's history.
    pub fn build_from_track(track: &EntityTrack) -> Self {
        if track.points.is_empty() {
            return Self {
                entity_id: track.entity_id.clone(),
                updated_at: Utc::now(),
                ..Default::default()
            };
        }

        let speeds: Vec<f64> = track.points.iter().map(|p| p.speed_knots).collect();
        let n = speeds.len() as f64;
        let mean = speeds.iter().sum::<f64>() / n;
        let variance = speeds.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
        let std = variance.sqrt();

        let lats: Vec<f64> = track.points.iter().map(|p| p.position.lat).collect();
        let lons: Vec<f64> = track.points.iter().map(|p| p.position.lon).collect();
        let bbox = BoundingBox {
            min_lat: lats.iter().cloned().fold(f64::MAX, f64::min),
            max_lat: lats.iter().cloned().fold(f64::MIN, f64::max),
            min_lon: lons.iter().cloned().fold(f64::MAX, f64::min),
            max_lon: lons.iter().cloned().fold(f64::MIN, f64::max),
        };

        // Count activity by hour
        let mut hour_counts = [0u32; 24];
        for pt in &track.points {
            hour_counts[pt.timestamp.hour() as usize] += 1;
        }
        let max_count = *hour_counts.iter().max().unwrap_or(&1).max(&1);
        // Include hours where activity is > 50% of peak
        let typical_hours: Vec<u8> = hour_counts
            .iter()
            .enumerate()
            .filter(|(_, &c)| c as f32 / max_count as f32 > 0.5)
            .map(|(h, _)| h as u8)
            .collect();

        Self {
            entity_id: track.entity_id.clone(),
            mean_speed_knots: mean,
            std_speed_knots: std,
            typical_bbox: Some(bbox),
            typical_hours,
            sample_count: track.points.len(),
            updated_at: Utc::now(),
        }
    }
}

// ── Anomaly scoring ───────────────────────────────────────────────────────────

/// Score 0–100 representing how anomalous current behaviour is vs. the baseline.
/// 0 = completely normal, 100 = maximally anomalous.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnomalyScore {
    pub entity_id: String,
    /// Composite score 0–100.
    pub score: f64,
    /// Breakdown of individual contributors.
    pub factors: AnomalyFactors,
    pub scored_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct AnomalyFactors {
    /// Speed deviation from baseline (0–100).
    pub speed_anomaly: f64,
    /// Operating outside typical bbox (0 or 50).
    pub location_anomaly: f64,
    /// Operating outside typical hours (0 or 30).
    pub time_anomaly: f64,
    /// Course change anomaly (0–20).
    pub course_anomaly: f64,
}

impl AnomalyFactors {
    pub fn composite(&self) -> f64 {
        let raw = self.speed_anomaly * 0.40
            + self.location_anomaly * 0.30
            + self.time_anomaly * 0.20
            + self.course_anomaly * 0.10;
        raw.clamp(0.0, 100.0)
    }
}

/// Compute an anomaly score for an entity against its pattern of life.
pub fn score_anomaly(
    entity_id: &str,
    track: &EntityTrack,
    pattern: &PatternOfLife,
) -> AnomalyScore {
    let now = Utc::now();
    let mut factors = AnomalyFactors::default();

    if let Some(latest) = track.latest() {
        // Speed anomaly: z-score mapped to 0–100
        // If std is near zero but speed differs, that's maximally anomalous.
        if pattern.std_speed_knots > 0.01 {
            let z = ((latest.speed_knots - pattern.mean_speed_knots) / pattern.std_speed_knots).abs();
            factors.speed_anomaly = (z / 3.0 * 100.0).clamp(0.0, 100.0);
        } else if pattern.mean_speed_knots > 0.1 {
            let pct_diff = ((latest.speed_knots - pattern.mean_speed_knots) / pattern.mean_speed_knots).abs();
            factors.speed_anomaly = (pct_diff * 100.0).clamp(0.0, 100.0);
        }

        // Location anomaly
        if let Some(bbox) = &pattern.typical_bbox {
            if !bbox.contains(&latest.position) {
                factors.location_anomaly = 50.0;
            }
        }

        // Time anomaly
        let current_hour = now.hour() as u8;
        if !pattern.typical_hours.is_empty() && !pattern.typical_hours.contains(&current_hour) {
            factors.time_anomaly = 30.0;
        }

        // Course change anomaly — flag if recent change > 45°
        let alerts = detect_manoeuvres(entity_id, track, 999.0, 45.0, 5);
        if alerts.iter().any(|a| a.alert_type == ManoeuvreType::CourseChange) {
            factors.course_anomaly = 20.0;
        }
    }

    let score = factors.composite();
    AnomalyScore {
        entity_id: entity_id.to_string(),
        score,
        factors,
        scored_at: now,
    }
}

// ── Dark target detection ─────────────────────────────────────────────────────

/// A "dark" target has stopped transmitting for longer than `dark_threshold`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DarkTargetAlert {
    pub entity_id: String,
    pub last_seen: DateTime<Utc>,
    pub dark_duration_minutes: f64,
    pub dark_threshold_minutes: f64,
    pub last_position: GeoPoint,
    pub detected_at: DateTime<Utc>,
}

impl DarkTargetAlert {
    /// How suspicious the dark gap is (1–3 severity tiers).
    pub fn severity(&self) -> &'static str {
        if self.dark_duration_minutes < 60.0 {
            "low"
        } else if self.dark_duration_minutes < 360.0 {
            "medium"
        } else {
            "high"
        }
    }
}

/// Scan all tracks and return alerts for entities that have gone dark.
pub fn detect_dark_targets(
    tracks: &HashMap<String, EntityTrack>,
    dark_threshold_minutes: f64,
) -> Vec<DarkTargetAlert> {
    let now = Utc::now();
    let mut alerts = Vec::new();

    for (entity_id, track) in tracks {
        if let Some(last_seen) = track.last_seen {
            let gap_minutes = (now - last_seen).num_seconds() as f64 / 60.0;
            if gap_minutes >= dark_threshold_minutes {
                if let Some(last_pt) = track.latest() {
                    alerts.push(DarkTargetAlert {
                        entity_id: entity_id.clone(),
                        last_seen,
                        dark_duration_minutes: gap_minutes,
                        dark_threshold_minutes,
                        last_position: last_pt.position.clone(),
                        detected_at: now,
                    });
                }
            }
        }
    }

    // Most suspicious first
    alerts.sort_by(|a, b| b.dark_duration_minutes.total_cmp(&a.dark_duration_minutes));
    alerts
}

// ── Analytics engine (orchestrates all above) ─────────────────────────────────

/// Aggregated analytics result for a single entity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityAnalytics {
    pub entity_id: String,
    pub anomaly_score: AnomalyScore,
    pub manoeuvre_alerts: Vec<ManoeuvreAlert>,
    pub dwell_alert: Option<DwellAlert>,
    pub is_dark: bool,
    pub zone_events: Vec<ZoneEvent>,
    pub analyzed_at: DateTime<Utc>,
}

/// Shared, thread-safe analytics state.
pub struct AnalyticsEngine {
    tracks: Arc<RwLock<HashMap<String, EntityTrack>>>,
    patterns: Arc<RwLock<HashMap<String, PatternOfLife>>>,
    zone_tracker: Arc<RwLock<ZoneTracker>>,
    /// Track ring-buffer length per entity.
    track_len: usize,
    /// Minutes without transmission → dark alert.
    dark_threshold_minutes: f64,
    /// Speed change % that triggers manoeuvre alert.
    speed_threshold_pct: f64,
    /// Course change degrees that triggers manoeuvre alert.
    course_threshold_deg: f64,
    /// Dwell radius (km) and time (minutes).
    dwell_radius_km: f64,
    dwell_threshold_minutes: f64,
}

impl AnalyticsEngine {
    pub fn new() -> Self {
        Self::with_config(500, 30.0, 50.0, 30.0, 0.5, 20.0)
    }

    pub fn with_config(
        track_len: usize,
        dark_threshold_minutes: f64,
        speed_threshold_pct: f64,
        course_threshold_deg: f64,
        dwell_radius_km: f64,
        dwell_threshold_minutes: f64,
    ) -> Self {
        Self {
            tracks: Arc::new(RwLock::new(HashMap::new())),
            patterns: Arc::new(RwLock::new(HashMap::new())),
            zone_tracker: Arc::new(RwLock::new(ZoneTracker::new())),
            track_len,
            dark_threshold_minutes,
            speed_threshold_pct,
            course_threshold_deg,
            dwell_radius_km,
            dwell_threshold_minutes,
        }
    }

    /// Ingest a new track point for an entity.
    pub async fn ingest(
        &self,
        entity_id: &str,
        position: GeoPoint,
        speed_knots: f64,
        course_degrees: f64,
        timestamp: DateTime<Utc>,
        zones: &[Zone],
    ) -> Vec<ZoneEvent> {
        // Update track
        {
            let mut tracks = self.tracks.write().await;
            let track = tracks
                .entry(entity_id.to_string())
                .or_insert_with(|| EntityTrack::new(entity_id, self.track_len));
            track.push(TrackPoint {
                timestamp,
                position: position.clone(),
                speed_knots,
                course_degrees,
            });
        }

        // Zone tracking
        let zone_events = {
            let mut zt = self.zone_tracker.write().await;
            zt.update(entity_id, &position, zones, timestamp)
        };

        // Periodically rebuild pattern of life (every 100 new points)
        {
            let tracks = self.tracks.read().await;
            if let Some(track) = tracks.get(entity_id) {
                if track.points.len() % 100 == 0 {
                    let pattern = PatternOfLife::build_from_track(track);
                    drop(tracks);
                    let mut patterns = self.patterns.write().await;
                    patterns.insert(entity_id.to_string(), pattern);
                }
            }
        }

        zone_events
    }

    /// Run full analytics for an entity and return the result.
    pub async fn analyze(&self, entity_id: &str) -> Option<EntityAnalytics> {
        let tracks = self.tracks.read().await;
        let track = tracks.get(entity_id)?;

        let patterns = self.patterns.read().await;
        let default_pattern = PatternOfLife {
            entity_id: entity_id.to_string(),
            updated_at: Utc::now(),
            ..Default::default()
        };
        let pattern = patterns.get(entity_id).unwrap_or(&default_pattern);

        let anomaly_score = score_anomaly(entity_id, track, pattern);
        let manoeuvre_alerts =
            detect_manoeuvres(entity_id, track, self.speed_threshold_pct, self.course_threshold_deg, 5);
        let dwell_alert = detect_dwell(entity_id, track, self.dwell_radius_km, self.dwell_threshold_minutes);

        let is_dark = track.last_seen.is_some_and(|ls| {
            (Utc::now() - ls).num_seconds() as f64 / 60.0 >= self.dark_threshold_minutes
        });

        Some(EntityAnalytics {
            entity_id: entity_id.to_string(),
            anomaly_score,
            manoeuvre_alerts,
            dwell_alert,
            is_dark,
            zone_events: Vec::new(), // populated during ingest
            analyzed_at: Utc::now(),
        })
    }

    /// Detect all dark targets across all known entities.
    pub async fn dark_targets(&self) -> Vec<DarkTargetAlert> {
        let tracks = self.tracks.read().await;
        detect_dark_targets(&tracks, self.dark_threshold_minutes)
    }

    /// Compute CPA between two entities using their latest track points.
    pub async fn cpa(
        &self,
        entity_a: &str,
        entity_b: &str,
        horizon_minutes: f64,
    ) -> Option<CpaResult> {
        let tracks = self.tracks.read().await;
        let a = tracks.get(entity_a)?.latest()?;
        let b = tracks.get(entity_b)?.latest()?;
        Some(calculate_cpa(
            entity_a,
            &a.position,
            a.speed_knots,
            a.course_degrees,
            entity_b,
            &b.position,
            b.speed_knots,
            b.course_degrees,
            horizon_minutes,
        ))
    }

    /// Get the current pattern of life for an entity.
    pub async fn get_pattern(&self, entity_id: &str) -> Option<PatternOfLife> {
        let patterns = self.patterns.read().await;
        patterns.get(entity_id).cloned()
    }

    /// Force-rebuild pattern of life from current track.
    pub async fn rebuild_pattern(&self, entity_id: &str) {
        let tracks = self.tracks.read().await;
        if let Some(track) = tracks.get(entity_id) {
            let pattern = PatternOfLife::build_from_track(track);
            drop(tracks);
            let mut patterns = self.patterns.write().await;
            patterns.insert(entity_id.to_string(), pattern);
        }
    }
}

impl Default for AnalyticsEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_point(lat: f64, lon: f64) -> GeoPoint {
        GeoPoint { lat, lon, alt: None }
    }

    fn make_track_with_points(entity_id: &str, points: Vec<(f64, f64, f64, f64)>) -> EntityTrack {
        // points: (lat, lon, speed_knots, course_deg)
        let mut track = EntityTrack::new(entity_id, 500);
        let base = Utc::now() - Duration::minutes(points.len() as i64);
        for (i, (lat, lon, spd, crs)) in points.into_iter().enumerate() {
            track.push(TrackPoint {
                timestamp: base + Duration::minutes(i as i64),
                position: make_point(lat, lon),
                speed_knots: spd,
                course_degrees: crs,
            });
        }
        track
    }

    // ── Haversine ──

    #[test]
    fn test_haversine_known_distance() {
        // London to Paris ≈ 344 km
        let london = make_point(51.5074, -0.1278);
        let paris = make_point(48.8566, 2.3522);
        let dist = haversine_km(&london, &paris);
        assert!((dist - 344.0).abs() < 5.0, "Got {:.1} km", dist);
    }

    // ── CPA ──

    #[test]
    fn test_cpa_same_direction_no_collision() {
        // Two ships heading north, A behind B, same speed → DCPA ≈ lateral separation
        let pos_a = make_point(1.0, 103.8);
        let pos_b = make_point(1.5, 103.8);
        let result = calculate_cpa("A", &pos_a, 12.0, 0.0, "B", &pos_b, 12.0, 0.0, 60.0);
        // They never get closer, so DCPA ~ initial separation
        assert!(result.dcpa_nm > 0.0);
        assert!(!result.is_collision_risk(0.5));
    }

    #[test]
    fn test_cpa_head_on_collision_risk() {
        // Ship A heading north, Ship B heading south, same longitude, 20nm apart
        let pos_a = make_point(0.0, 104.0);
        let pos_b = make_point(0.3, 104.0); // ~33km ≈ 18nm north of A
        let result = calculate_cpa("A", &pos_a, 15.0, 0.0, "B", &pos_b, 15.0, 180.0, 60.0);
        // They converge — DCPA should be very small
        assert!(result.dcpa_nm < 2.0, "Got {} nm", result.dcpa_nm);
        assert!(result.is_collision_risk(3.0));
    }

    // ── Manoeuvre detection ──

    #[test]
    fn test_speed_change_alert() {
        // 10 knots increasing to 25 knots (150% change)
        let track = make_track_with_points(
            "vessel1",
            vec![
                (1.3, 103.8, 10.0, 45.0),
                (1.31, 103.81, 12.0, 45.0),
                (1.32, 103.82, 18.0, 45.0),
                (1.33, 103.83, 25.0, 45.0),
            ],
        );
        let alerts = detect_manoeuvres("vessel1", &track, 50.0, 30.0, 10);
        assert!(alerts.iter().any(|a| a.alert_type == ManoeuvreType::SpeedChange));
    }

    #[test]
    fn test_no_alert_when_stable() {
        let track = make_track_with_points(
            "stable",
            vec![
                (1.3, 103.8, 12.0, 45.0),
                (1.31, 103.81, 12.1, 44.5),
                (1.32, 103.82, 11.9, 45.5),
            ],
        );
        let alerts = detect_manoeuvres("stable", &track, 50.0, 30.0, 10);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_course_change_alert() {
        // 45° → 120° = 75° change
        let track = make_track_with_points(
            "vessel2",
            vec![
                (2.0, 104.0, 12.0, 45.0),
                (2.01, 104.01, 12.0, 60.0),
                (2.02, 104.02, 12.0, 90.0),
                (2.03, 104.03, 12.0, 120.0),
            ],
        );
        let alerts = detect_manoeuvres("vessel2", &track, 50.0, 30.0, 10);
        assert!(alerts.iter().any(|a| a.alert_type == ManoeuvreType::CourseChange));
    }

    // ── Zone detection ──

    #[test]
    fn test_zone_entry_exit() {
        let zone = Zone {
            zone_id: "z1".to_string(),
            name: "Test Zone".to_string(),
            polygon: vec![
                make_point(0.0, 0.0),
                make_point(1.0, 0.0),
                make_point(1.0, 1.0),
                make_point(0.0, 1.0),
            ],
        };

        let mut tracker = ZoneTracker::new();
        let zones = vec![zone];

        // Enter zone
        let events = tracker.update("e1", &make_point(0.5, 0.5), &zones, Utc::now());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, ZoneEventType::Entry);

        // Still inside
        let events = tracker.update("e1", &make_point(0.6, 0.6), &zones, Utc::now());
        assert!(events.is_empty());

        // Exit zone
        let events = tracker.update("e1", &make_point(2.0, 2.0), &zones, Utc::now());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, ZoneEventType::Exit);
    }

    #[test]
    fn test_point_in_polygon() {
        let zone = Zone {
            zone_id: "sq".to_string(),
            name: "Square".to_string(),
            polygon: vec![
                make_point(0.0, 0.0),
                make_point(10.0, 0.0),
                make_point(10.0, 10.0),
                make_point(0.0, 10.0),
            ],
        };
        assert!(zone.contains(&make_point(5.0, 5.0)));
        assert!(!zone.contains(&make_point(15.0, 5.0)));
        assert!(!zone.contains(&make_point(-1.0, 5.0)));
    }

    // ── Dwell detection ──

    #[test]
    fn test_dwell_detected() {
        let mut track = EntityTrack::new("dwell_vessel", 500);
        let base = Utc::now() - Duration::minutes(60);
        for i in 0..60 {
            track.push(TrackPoint {
                timestamp: base + Duration::minutes(i),
                // Tiny movement — all within 0.1km of each other
                position: make_point(1.3 + i as f64 * 0.0001, 103.8),
                speed_knots: 0.1,
                course_degrees: 0.0,
            });
        }
        let alert = detect_dwell("dwell_vessel", &track, 0.5, 30.0);
        assert!(alert.is_some());
        let a = alert.unwrap();
        assert!(a.dwell_duration_minutes >= 30.0);
    }

    #[test]
    fn test_no_dwell_when_moving() {
        let track = make_track_with_points(
            "moving",
            vec![
                (0.0, 104.0, 15.0, 0.0),
                (0.1, 104.0, 15.0, 0.0),
                (0.2, 104.0, 15.0, 0.0),
                (0.5, 104.0, 15.0, 0.0),
            ],
        );
        let alert = detect_dwell("moving", &track, 0.1, 30.0);
        // Points are spread > 0.1km apart, so no dwell
        assert!(alert.is_none());
    }

    // ── Pattern of life ──

    #[test]
    fn test_pattern_of_life_basic() {
        let track = make_track_with_points(
            "pol_vessel",
            (0..100)
                .map(|i| (1.3 + i as f64 * 0.001, 103.8 + i as f64 * 0.001, 12.0, 45.0))
                .collect(),
        );
        let pattern = PatternOfLife::build_from_track(&track);
        assert_eq!(pattern.sample_count, 100);
        assert!((pattern.mean_speed_knots - 12.0).abs() < 0.1);
        assert!(pattern.std_speed_knots < 0.1); // All same speed
        assert!(pattern.typical_bbox.is_some());
    }

    // ── Anomaly scoring ──

    #[test]
    fn test_normal_behavior_low_score() {
        let track = make_track_with_points(
            "normal",
            (0..50)
                .map(|i| (1.3 + i as f64 * 0.001, 103.8 + i as f64 * 0.001, 12.0, 45.0))
                .collect(),
        );
        let pattern = PatternOfLife::build_from_track(&track);
        let score = score_anomaly("normal", &track, &pattern);
        // Operating within bbox, normal speed, consistent course → low anomaly
        assert!(score.score < 40.0, "Got {:.1}", score.score);
    }

    #[test]
    fn test_anomalous_speed_raises_score() {
        let mut track = make_track_with_points(
            "anomalous",
            (0..50)
                .map(|i| (1.3 + i as f64 * 0.001, 103.8 + i as f64 * 0.001, 12.0, 45.0))
                .collect(),
        );
        let pattern = PatternOfLife::build_from_track(&track);
        // Inject sudden 60-knot sprint (5 sigma above 12-knot baseline)
        track.push(TrackPoint {
            timestamp: Utc::now(),
            position: make_point(1.35, 103.85),
            speed_knots: 60.0,
            course_degrees: 45.0,
        });
        let score = score_anomaly("anomalous", &track, &pattern);
        assert!(score.score > 50.0, "Got {:.1}", score.score);
    }

    // ── Dark target ──

    #[test]
    fn test_dark_target_detection() {
        let mut tracks = HashMap::new();
        let mut old_track = EntityTrack::new("dark_ship", 100);
        old_track.push(TrackPoint {
            timestamp: Utc::now() - Duration::hours(3), // 3 hours ago
            position: make_point(25.0, 56.0),
            speed_knots: 8.0,
            course_degrees: 90.0,
        });
        tracks.insert("dark_ship".to_string(), old_track);

        // Fresh vessel — not dark
        let mut fresh_track = EntityTrack::new("fresh_ship", 100);
        fresh_track.push(TrackPoint {
            timestamp: Utc::now(),
            position: make_point(26.0, 57.0),
            speed_knots: 10.0,
            course_degrees: 180.0,
        });
        tracks.insert("fresh_ship".to_string(), fresh_track);

        let dark = detect_dark_targets(&tracks, 60.0); // 60 min threshold
        assert_eq!(dark.len(), 1);
        assert_eq!(dark[0].entity_id, "dark_ship");
        assert_eq!(dark[0].severity(), "medium"); // 3hrs = medium
    }

    // ── Heading diff ──

    #[test]
    fn test_heading_diff() {
        assert!((heading_diff(350.0, 10.0) - 20.0).abs() < 0.01);
        assert!((heading_diff(90.0, 270.0) - 180.0).abs() < 0.01);
        assert!((heading_diff(45.0, 45.0)).abs() < 0.01);
    }
}
