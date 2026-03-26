//! Threat assessment engine — multi-factor risk scoring and classification.
//!
//! Combines:
//! - Sanctions list matching
//! - Anomalous behaviour score (from analytics engine)
//! - Proximity to critical infrastructure
//! - Dark period duration
//! - Speed violations
//!
//! Produces a threat classification: GREEN → YELLOW → ORANGE → RED
//! and auto-alerts on any classification change.

use crate::analytics::DarkTargetAlert;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

// ── Threat classification ─────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ThreatLevel {
    /// Normal operations — no indicators of concern.
    Green,
    /// Suspicious — one or more mild risk indicators.
    Yellow,
    /// Elevated — multiple indicators or one severe indicator.
    Orange,
    /// Active threat — immediate action recommended.
    Red,
}

impl ThreatLevel {
    pub fn label(&self) -> &'static str {
        match self {
            ThreatLevel::Green => "GREEN",
            ThreatLevel::Yellow => "YELLOW",
            ThreatLevel::Orange => "ORANGE",
            ThreatLevel::Red => "RED",
        }
    }

    pub fn from_score(score: f64) -> Self {
        match score as u32 {
            0..=24 => ThreatLevel::Green,
            25..=49 => ThreatLevel::Yellow,
            50..=74 => ThreatLevel::Orange,
            _ => ThreatLevel::Red,
        }
    }
}

impl std::fmt::Display for ThreatLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

// ── Risk factors ──────────────────────────────────────────────────────────────

/// Breakdown of risk contributors (each 0–100 before weighting).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct RiskFactors {
    /// Entity or linked identity appears on a sanctions list (0 or 100).
    pub sanctions_match: f64,
    /// Anomaly score from pattern-of-life analysis (0–100).
    pub behaviour_anomaly: f64,
    /// Proximity score to critical infrastructure (0–100, closer = higher).
    pub infrastructure_proximity: f64,
    /// Dark period score — maps duration to risk (0–100).
    pub dark_period: f64,
    /// Speed violation score (0–100).
    pub speed_violation: f64,
}

impl RiskFactors {
    /// Compute weighted composite risk score (0–100).
    pub fn composite(&self, weights: &RiskWeights) -> f64 {
        let raw = self.sanctions_match * weights.sanctions
            + self.behaviour_anomaly * weights.behaviour
            + self.infrastructure_proximity * weights.infrastructure
            + self.dark_period * weights.dark_period
            + self.speed_violation * weights.speed_violation;
        raw.clamp(0.0, 100.0)
    }
}

/// Configurable weights for each risk factor. All values should sum to ~1.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RiskWeights {
    pub sanctions: f64,
    pub behaviour: f64,
    pub infrastructure: f64,
    pub dark_period: f64,
    pub speed_violation: f64,
}

impl Default for RiskWeights {
    fn default() -> Self {
        Self {
            sanctions: 0.35,
            behaviour: 0.25,
            infrastructure: 0.20,
            dark_period: 0.12,
            speed_violation: 0.08,
        }
    }
}

impl RiskWeights {
    /// Validate that weights are sensible (all non-negative, sum > 0).
    pub fn validate(&self) -> Result<(), String> {
        let weights = [
            self.sanctions,
            self.behaviour,
            self.infrastructure,
            self.dark_period,
            self.speed_violation,
        ];
        if weights.iter().any(|&w| w < 0.0) {
            return Err("Risk weights must not be negative".to_string());
        }
        let sum: f64 = weights.iter().sum();
        if sum < 0.01 {
            return Err("Risk weights must sum to a positive value".to_string());
        }
        Ok(())
    }
}

// ── Critical infrastructure ───────────────────────────────────────────────────

/// A critical infrastructure point (port, terminal, cable landing, etc.).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CriticalInfrastructure {
    pub id: String,
    pub name: String,
    pub infrastructure_type: InfrastructureType,
    pub lat: f64,
    pub lon: f64,
    /// Alert radius in nautical miles.
    pub alert_radius_nm: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfrastructureType {
    Port,
    LngTerminal,
    SubmarineCable,
    OilPlatform,
    NavalBase,
    CoastalRadar,
    PipelineInfrastructure,
}

impl CriticalInfrastructure {
    fn distance_nm(&self, lat: f64, lon: f64) -> f64 {
        let r = 6371.0_f64;
        let dlat = (lat - self.lat).to_radians();
        let dlon = (lon - self.lon).to_radians();
        let h = (dlat / 2.0).sin().powi(2)
            + self.lat.to_radians().cos() * lat.to_radians().cos() * (dlon / 2.0).sin().powi(2);
        let km = r * 2.0 * h.sqrt().asin();
        km / 1.852
    }

    /// Proximity score 0–100 (100 = inside alert radius, 0 = > 2× radius away).
    pub fn proximity_score(&self, lat: f64, lon: f64) -> f64 {
        let dist = self.distance_nm(lat, lon);
        if dist <= self.alert_radius_nm {
            100.0
        } else if dist <= self.alert_radius_nm * 2.0 {
            // Linear fade from 100 → 0 between r and 2r
            (1.0 - (dist - self.alert_radius_nm) / self.alert_radius_nm) * 100.0
        } else {
            0.0
        }
    }
}

// ── Sanctions ─────────────────────────────────────────────────────────────────

/// Minimal sanctions list — maps identifiers (MMSI, IMO, entity name) to entries.
#[derive(Clone, Debug, Default)]
pub struct SanctionsList {
    /// Set of sanctioned identifiers (MMSI, IMO numbers, canonical IDs, names).
    identifiers: std::collections::HashSet<String>,
}

impl SanctionsList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load identifiers from a flat list.
    pub fn load(&mut self, identifiers: impl IntoIterator<Item = String>) {
        self.identifiers.extend(identifiers);
    }

    /// Check if any of the provided identifiers match the sanctions list.
    pub fn is_sanctioned(&self, identifiers: &[&str]) -> bool {
        identifiers.iter().any(|id| {
            self.identifiers.contains(*id) || self.identifiers.contains(&id.to_lowercase())
        })
    }
}

// ── Threat assessment ─────────────────────────────────────────────────────────

/// Full threat assessment for an entity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreatAssessment {
    pub entity_id: String,
    /// Composite risk score 0–100.
    pub risk_score: f64,
    pub threat_level: ThreatLevel,
    pub risk_factors: RiskFactors,
    /// Human-readable justification bullets.
    pub indicators: Vec<String>,
    pub assessed_at: DateTime<Utc>,
    /// Previous threat level (for change detection).
    pub previous_level: Option<ThreatLevel>,
    /// True if this assessment represents a classification change.
    pub level_changed: bool,
}

impl ThreatAssessment {
    /// Create a new assessment with no prior history.
    pub fn new(entity_id: impl Into<String>, risk_score: f64, factors: RiskFactors, indicators: Vec<String>) -> Self {
        let threat_level = ThreatLevel::from_score(risk_score);
        Self {
            entity_id: entity_id.into(),
            risk_score,
            threat_level,
            risk_factors: factors,
            indicators,
            assessed_at: Utc::now(),
            previous_level: None,
            level_changed: false,
        }
    }
}

// ── Alert ─────────────────────────────────────────────────────────────────────

/// Auto-alert emitted on threat level change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreatAlert {
    pub alert_id: String,
    pub entity_id: String,
    pub previous_level: ThreatLevel,
    pub new_level: ThreatLevel,
    pub risk_score: f64,
    pub indicators: Vec<String>,
    pub escalated: bool,
    pub triggered_at: DateTime<Utc>,
}

impl ThreatAlert {
    pub fn escalated_message(&self) -> String {
        format!(
            "THREAT ESCALATION: {} {} → {} (score {:.1}) — {}",
            self.entity_id,
            self.previous_level,
            self.new_level,
            self.risk_score,
            self.indicators.join("; ")
        )
    }
}

// ── Threat engine ─────────────────────────────────────────────────────────────

/// Thread-safe threat assessment engine.
pub struct ThreatEngine {
    weights: RiskWeights,
    sanctions: Arc<RwLock<SanctionsList>>,
    infrastructure: Arc<Vec<CriticalInfrastructure>>,
    /// Latest assessment per entity.
    assessments: Arc<RwLock<HashMap<String, ThreatAssessment>>>,
    /// Broadcast channel for real-time threat alerts.
    alert_tx: broadcast::Sender<ThreatAlert>,
    /// Speed limit in knots — exceeding triggers violation factor.
    max_speed_knots: f64,
}

impl ThreatEngine {
    pub fn new(weights: RiskWeights, infrastructure: Vec<CriticalInfrastructure>) -> Self {
        let (alert_tx, _) = broadcast::channel(1024);
        Self {
            weights,
            sanctions: Arc::new(RwLock::new(SanctionsList::new())),
            infrastructure: Arc::new(infrastructure),
            assessments: Arc::new(RwLock::new(HashMap::new())),
            alert_tx,
            max_speed_knots: 30.0,
        }
    }

    pub fn with_max_speed(mut self, knots: f64) -> Self {
        self.max_speed_knots = knots;
        self
    }

    /// Subscribe to the threat alert stream.
    pub fn subscribe_alerts(&self) -> broadcast::Receiver<ThreatAlert> {
        self.alert_tx.subscribe()
    }

    /// Load sanctions identifiers.
    pub async fn load_sanctions(&self, identifiers: impl IntoIterator<Item = String>) {
        let mut s = self.sanctions.write().await;
        s.load(identifiers);
    }

    /// Assess an entity and emit an alert if threat level changes.
    /// Assess an entity and emit an alert if threat level changes.
    ///
    /// - `identifiers`: MMSI, IMO, name, etc. — checked against sanctions list
    /// - `lat`/`lon`: current position
    /// - `speed_knots`: current speed
    /// - `anomaly_score`: 0–100 from analytics engine
    /// - `dark_alert`: optional dark-target alert
    #[allow(clippy::too_many_arguments)]
    pub async fn assess(
        &self,
        entity_id: &str,
        identifiers: &[&str],
        lat: f64,
        lon: f64,
        speed_knots: f64,
        anomaly_score: f64,
        dark_alert: Option<&DarkTargetAlert>,
    ) -> ThreatAssessment {
        let mut factors = RiskFactors::default();
        let mut indicators = Vec::new();

        // ── Sanctions check ──
        {
            let sanctions = self.sanctions.read().await;
            if sanctions.is_sanctioned(identifiers) {
                factors.sanctions_match = 100.0;
                indicators.push("Entity appears on sanctions list".to_string());
            }
        }

        // ── Behaviour anomaly ──
        factors.behaviour_anomaly = anomaly_score;
        if anomaly_score > 70.0 {
            indicators.push(format!("High anomaly score: {:.1}/100", anomaly_score));
        } else if anomaly_score > 40.0 {
            indicators.push(format!("Elevated anomaly score: {:.1}/100", anomaly_score));
        }

        // ── Infrastructure proximity ──
        let mut max_proximity = 0.0_f64;
        let mut closest_infra: Option<&CriticalInfrastructure> = None;
        for infra in self.infrastructure.as_ref() {
            let score = infra.proximity_score(lat, lon);
            if score > max_proximity {
                max_proximity = score;
                closest_infra = Some(infra);
            }
        }
        factors.infrastructure_proximity = max_proximity;
        if max_proximity > 0.0 {
            if let Some(infra) = closest_infra {
                let dist = infra.distance_nm(lat, lon);
                indicators.push(format!(
                    "Within {:.1}nm of critical infrastructure: {}",
                    dist, infra.name
                ));
            }
        }

        // ── Dark period ──
        if let Some(dark) = dark_alert {
            // Map dark duration: 60min=20, 6hr=60, 24hr=100
            let dark_score = (dark.dark_duration_minutes / 1440.0 * 100.0).clamp(0.0, 100.0);
            factors.dark_period = dark_score;
            indicators.push(format!(
                "Dark gap: {:.0} minutes (AIS/transponder off)",
                dark.dark_duration_minutes
            ));
        }

        // ── Speed violation ──
        if speed_knots > self.max_speed_knots {
            let excess_pct = ((speed_knots - self.max_speed_knots) / self.max_speed_knots) * 100.0;
            factors.speed_violation = excess_pct.clamp(0.0, 100.0);
            indicators.push(format!(
                "Speed violation: {:.1}kn (limit {:.1}kn)",
                speed_knots, self.max_speed_knots
            ));
        }

        // ── Composite score ──
        let risk_score = factors.composite(&self.weights);
        let threat_level = ThreatLevel::from_score(risk_score);

        // ── Change detection ──
        let previous_level = {
            let assessments = self.assessments.read().await;
            assessments.get(entity_id).map(|a| a.threat_level.clone())
        };

        let level_changed = previous_level
            .as_ref()
            .map(|prev| prev != &threat_level)
            .unwrap_or(true); // First assessment always counts as a change

        let assessment = ThreatAssessment {
            entity_id: entity_id.to_string(),
            risk_score,
            threat_level: threat_level.clone(),
            risk_factors: factors,
            indicators: indicators.clone(),
            assessed_at: Utc::now(),
            previous_level: previous_level.clone(),
            level_changed,
        };

        // Store assessment
        {
            let mut assessments = self.assessments.write().await;
            assessments.insert(entity_id.to_string(), assessment.clone());
        }

        // ── Auto-alert on level change ──
        if level_changed {
            let prev = previous_level.unwrap_or(ThreatLevel::Green);
            let escalated = threat_level > prev;
            let alert = ThreatAlert {
                alert_id: uuid::Uuid::new_v4().to_string(),
                entity_id: entity_id.to_string(),
                previous_level: prev,
                new_level: threat_level,
                risk_score,
                indicators,
                escalated,
                triggered_at: Utc::now(),
            };

            if escalated {
                tracing::warn!(
                    entity_id = entity_id,
                    score = risk_score,
                    level = assessment.threat_level.label(),
                    "{}",
                    alert.escalated_message()
                );
            }

            // Non-blocking send — subscribers may have lagged
            let _ = self.alert_tx.send(alert);
        }

        assessment
    }

    /// Get the current assessment for an entity.
    pub async fn get(&self, entity_id: &str) -> Option<ThreatAssessment> {
        let assessments = self.assessments.read().await;
        assessments.get(entity_id).cloned()
    }

    /// List all entities at or above a given threat level.
    pub async fn filter_by_level(&self, min_level: ThreatLevel) -> Vec<ThreatAssessment> {
        let assessments = self.assessments.read().await;
        let mut result: Vec<ThreatAssessment> = assessments
            .values()
            .filter(|a| a.threat_level >= min_level)
            .cloned()
            .collect();
        result.sort_by(|a, b| b.risk_score.total_cmp(&a.risk_score));
        result
    }

    /// List all RED entities (immediate threat).
    pub async fn red_entities(&self) -> Vec<ThreatAssessment> {
        self.filter_by_level(ThreatLevel::Red).await
    }

    /// Summarize threat picture across all tracked entities.
    pub async fn threat_summary(&self) -> ThreatSummary {
        let assessments = self.assessments.read().await;
        let mut summary = ThreatSummary::default();
        for a in assessments.values() {
            summary.total += 1;
            match a.threat_level {
                ThreatLevel::Green => summary.green += 1,
                ThreatLevel::Yellow => summary.yellow += 1,
                ThreatLevel::Orange => summary.orange += 1,
                ThreatLevel::Red => summary.red += 1,
            }
        }
        summary.generated_at = Utc::now();
        summary
    }

    /// Update risk weights (hot-reload without restart).
    pub fn update_weights(&mut self, weights: RiskWeights) -> Result<(), String> {
        weights.validate()?;
        self.weights = weights;
        Ok(())
    }
}

/// High-level threat picture.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ThreatSummary {
    pub total: usize,
    pub green: usize,
    pub yellow: usize,
    pub orange: usize,
    pub red: usize,
    pub generated_at: DateTime<Utc>,
}

impl ThreatSummary {
    pub fn threat_pct(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.orange + self.red) as f64 / self.total as f64 * 100.0
    }
}

// ── Built-in critical infrastructure ─────────────────────────────────────────

/// Returns a baseline set of globally significant critical infrastructure points.
pub fn default_critical_infrastructure() -> Vec<CriticalInfrastructure> {
    vec![
        CriticalInfrastructure {
            id: "sg-port".to_string(),
            name: "Port of Singapore".to_string(),
            infrastructure_type: InfrastructureType::Port,
            lat: 1.2655,
            lon: 103.8200,
            alert_radius_nm: 5.0,
        },
        CriticalInfrastructure {
            id: "rotterdam-port".to_string(),
            name: "Port of Rotterdam".to_string(),
            infrastructure_type: InfrastructureType::Port,
            lat: 51.9225,
            lon: 4.4792,
            alert_radius_nm: 5.0,
        },
        CriticalInfrastructure {
            id: "hormuz-lng".to_string(),
            name: "Ras Laffan LNG Terminal".to_string(),
            infrastructure_type: InfrastructureType::LngTerminal,
            lat: 25.9177,
            lon: 51.5508,
            alert_radius_nm: 3.0,
        },
        CriticalInfrastructure {
            id: "suez-north".to_string(),
            name: "Port Said (Suez Canal North)".to_string(),
            infrastructure_type: InfrastructureType::Port,
            lat: 31.2565,
            lon: 32.3088,
            alert_radius_nm: 4.0,
        },
        CriticalInfrastructure {
            id: "malacca-pipe".to_string(),
            name: "Malacca Strait Pipeline Crossing".to_string(),
            infrastructure_type: InfrastructureType::PipelineInfrastructure,
            lat: 2.5,
            lon: 101.5,
            alert_radius_nm: 2.0,
        },
        CriticalInfrastructure {
            id: "dover-cable".to_string(),
            name: "Cross-Channel Submarine Cable Cluster".to_string(),
            infrastructure_type: InfrastructureType::SubmarineCable,
            lat: 51.0,
            lon: 1.5,
            alert_radius_nm: 2.0,
        },
        CriticalInfrastructure {
            id: "north-sea-platform".to_string(),
            name: "Brent Oil Field (North Sea)".to_string(),
            infrastructure_type: InfrastructureType::OilPlatform,
            lat: 61.03,
            lon: 1.70,
            alert_radius_nm: 3.0,
        },
    ]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_engine() -> ThreatEngine {
        ThreatEngine::new(RiskWeights::default(), default_critical_infrastructure())
    }

    // ── ThreatLevel ──

    #[test]
    fn test_threat_level_from_score() {
        assert_eq!(ThreatLevel::from_score(0.0), ThreatLevel::Green);
        assert_eq!(ThreatLevel::from_score(24.9), ThreatLevel::Green);
        assert_eq!(ThreatLevel::from_score(25.0), ThreatLevel::Yellow);
        assert_eq!(ThreatLevel::from_score(49.9), ThreatLevel::Yellow);
        assert_eq!(ThreatLevel::from_score(50.0), ThreatLevel::Orange);
        assert_eq!(ThreatLevel::from_score(74.9), ThreatLevel::Orange);
        assert_eq!(ThreatLevel::from_score(75.0), ThreatLevel::Red);
        assert_eq!(ThreatLevel::from_score(100.0), ThreatLevel::Red);
    }

    #[test]
    fn test_threat_level_ordering() {
        assert!(ThreatLevel::Green < ThreatLevel::Yellow);
        assert!(ThreatLevel::Yellow < ThreatLevel::Orange);
        assert!(ThreatLevel::Orange < ThreatLevel::Red);
    }

    // ── RiskWeights ──

    #[test]
    fn test_weights_validate() {
        assert!(RiskWeights::default().validate().is_ok());
        let bad = RiskWeights {
            sanctions: -0.1,
            behaviour: 0.5,
            infrastructure: 0.2,
            dark_period: 0.1,
            speed_violation: 0.1,
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_composite_score_sanctions_dominant() {
        let weights = RiskWeights::default();
        let factors = RiskFactors {
            sanctions_match: 100.0,
            behaviour_anomaly: 0.0,
            infrastructure_proximity: 0.0,
            dark_period: 0.0,
            speed_violation: 0.0,
        };
        // sanctions weight = 0.35 → score = 35
        let score = factors.composite(&weights);
        assert!((score - 35.0).abs() < 0.01, "Got {:.2}", score);
    }

    #[test]
    fn test_full_sanctions_plus_behaviour_raises_to_red() {
        let weights = RiskWeights::default();
        let factors = RiskFactors {
            sanctions_match: 100.0,  // × 0.35 = 35
            behaviour_anomaly: 100.0, // × 0.25 = 25
            infrastructure_proximity: 100.0, // × 0.20 = 20
            dark_period: 100.0,      // × 0.12 = 12
            speed_violation: 100.0,  // × 0.08 = 8
        };
        let score = factors.composite(&weights);
        assert!((score - 100.0).abs() < 0.01);
        assert_eq!(ThreatLevel::from_score(score), ThreatLevel::Red);
    }

    // ── Critical infrastructure ──

    #[test]
    fn test_proximity_inside_radius() {
        let infra = CriticalInfrastructure {
            id: "test".to_string(),
            name: "Test".to_string(),
            infrastructure_type: InfrastructureType::Port,
            lat: 1.265,
            lon: 103.82,
            alert_radius_nm: 5.0,
        };
        // Same position → 100
        let score = infra.proximity_score(1.265, 103.82);
        assert_eq!(score, 100.0);
    }

    #[test]
    fn test_proximity_outside_double_radius() {
        let infra = CriticalInfrastructure {
            id: "test".to_string(),
            name: "Test".to_string(),
            infrastructure_type: InfrastructureType::Port,
            lat: 1.265,
            lon: 103.82,
            alert_radius_nm: 5.0,
        };
        // ~1000nm away → 0
        let score = infra.proximity_score(50.0, 103.82);
        assert_eq!(score, 0.0);
    }

    // ── Sanctions ──

    #[test]
    fn test_sanctions_match() {
        let mut list = SanctionsList::new();
        list.load(vec!["123456789".to_string(), "IMO1234567".to_string()]);
        assert!(list.is_sanctioned(&["123456789"]));
        assert!(list.is_sanctioned(&["IMO1234567"]));
        assert!(!list.is_sanctioned(&["999999999"]));
    }

    #[test]
    fn test_sanctions_case_insensitive() {
        let mut list = SanctionsList::new();
        list.load(vec!["sanctioned_vessel".to_string()]);
        assert!(list.is_sanctioned(&["SANCTIONED_VESSEL"]));
    }

    // ── Threat engine ──

    #[tokio::test]
    async fn test_normal_entity_is_green() {
        let engine = default_engine();
        let assessment = engine
            .assess(
                "vessel_normal",
                &["normal_mmsi"],
                51.92, // Rotterdam area but 50nm away
                3.0,
                12.0,  // normal speed
                5.0,   // low anomaly
                None,  // no dark gap
            )
            .await;
        assert_eq!(assessment.threat_level, ThreatLevel::Green);
        assert!(assessment.risk_score < 25.0);
    }

    #[tokio::test]
    async fn test_sanctioned_entity_is_at_least_yellow() {
        let engine = default_engine();
        engine
            .load_sanctions(vec!["SANCTIONED_MMSI".to_string()])
            .await;
        let assessment = engine
            .assess(
                "vessel_sanctioned",
                &["SANCTIONED_MMSI"],
                10.0,
                50.0,
                10.0,
                0.0,
                None,
            )
            .await;
        assert!(
            assessment.threat_level >= ThreatLevel::Yellow,
            "Got {:?}",
            assessment.threat_level
        );
        assert!(assessment.indicators.iter().any(|i| i.contains("sanctions")));
    }

    #[tokio::test]
    async fn test_dark_target_raises_threat() {
        let engine = default_engine();
        let dark = DarkTargetAlert {
            entity_id: "dark_vessel".to_string(),
            last_seen: Utc::now() - chrono::Duration::hours(12),
            dark_duration_minutes: 720.0,
            dark_threshold_minutes: 60.0,
            last_position: orp_proto::GeoPoint { lat: 25.0, lon: 56.0, alt: None },
            detected_at: Utc::now(),
        };
        let assessment = engine
            .assess("dark_vessel", &[], 25.0, 56.0, 8.0, 10.0, Some(&dark))
            .await;
        assert!(assessment.indicators.iter().any(|i| i.contains("Dark gap")));
        assert!(assessment.risk_score > 5.0); // dark_period factor contributed
    }

    #[tokio::test]
    async fn test_threat_level_escalation_emits_alert() {
        let engine = default_engine();
        engine
            .load_sanctions(vec!["HIGH_RISK_MMSI".to_string()])
            .await;
        let mut alert_rx = engine.subscribe_alerts();

        // First assessment (from nothing → whatever) emits
        let _a1 = engine
            .assess("esc_vessel", &[], 10.0, 50.0, 8.0, 5.0, None)
            .await;

        // Second with sanctions → escalate
        let _a2 = engine
            .assess("esc_vessel", &["HIGH_RISK_MMSI"], 10.0, 50.0, 40.0, 80.0, None)
            .await;

        // At least one alert should be in the channel
        let alert = alert_rx.try_recv();
        assert!(alert.is_ok(), "Expected threat alert");
    }

    #[tokio::test]
    async fn test_filter_by_level() {
        let engine = default_engine();
        engine
            .load_sanctions(vec!["S1".to_string(), "S2".to_string()])
            .await;

        // Force two entities to high risk
        engine.assess("e1", &["S1"], 1.265, 103.82, 35.0, 90.0, None).await;
        engine.assess("e2", &["S2"], 1.265, 103.82, 35.0, 90.0, None).await;
        engine.assess("e3", &[], 50.0, 10.0, 5.0, 2.0, None).await;

        let orangeplus = engine.filter_by_level(ThreatLevel::Orange).await;
        // e3 should be green, not in result
        assert!(!orangeplus.iter().any(|a| a.entity_id == "e3"));
    }

    #[tokio::test]
    async fn test_threat_summary() {
        let engine = default_engine();
        engine.assess("g1", &[], 50.0, 10.0, 3.0, 2.0, None).await;
        engine.assess("g2", &[], 51.0, 11.0, 4.0, 3.0, None).await;
        let summary = engine.threat_summary().await;
        assert_eq!(summary.total, 2);
        assert_eq!(summary.green, 2);
        assert_eq!(summary.red, 0);
    }

    #[test]
    fn test_threat_summary_threat_pct() {
        let summary = ThreatSummary {
            total: 10,
            green: 5,
            yellow: 2,
            orange: 2,
            red: 1,
            generated_at: Utc::now(),
        };
        assert!((summary.threat_pct() - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_default_infrastructure_is_non_empty() {
        let infra = default_critical_infrastructure();
        assert!(!infra.is_empty());
        // All should have positive alert radii
        for i in &infra {
            assert!(i.alert_radius_nm > 0.0);
        }
    }
}
