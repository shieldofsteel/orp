use chrono::{DateTime, Utc};
use orp_proto::{Entity, EventPayload, EventSeverity, OrpEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A monitor rule that watches for conditions on entities
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonitorRule {
    pub rule_id: String,
    pub name: String,
    pub description: String,
    pub entity_type: String,
    pub condition: MonitorCondition,
    pub action: MonitorAction,
    pub enabled: bool,
    pub cooldown_seconds: u64,
    pub severity: AlertSeverity,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MonitorCondition {
    /// Property exceeds a threshold
    PropertyThreshold {
        property: String,
        operator: ThresholdOp,
        value: f64,
    },
    /// Entity enters or exits a geofenced area
    Geofence {
        lat: f64,
        lon: f64,
        radius_km: f64,
        trigger_on: GeofenceTrigger,
    },
    /// Entity has not been updated in a certain duration
    Stale {
        max_age_seconds: u64,
    },
    /// Speed change exceeds threshold
    SpeedAnomaly {
        max_change_knots: f64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ThresholdOp {
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Equal,
    NotEqual,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GeofenceTrigger {
    Enter,
    Exit,
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MonitorAction {
    Alert,
    Log,
    Webhook { url: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

/// An alert triggered by a monitor rule
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert {
    pub alert_id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub entity_id: String,
    pub entity_type: String,
    pub severity: AlertSeverity,
    pub message: String,
    pub evidence: serde_json::Value,
    pub triggered_at: DateTime<Utc>,
    pub acknowledged: bool,
}

/// Monitor engine that evaluates rules against incoming events
pub struct MonitorEngine {
    rules: Arc<Mutex<Vec<MonitorRule>>>,
    alerts: Arc<Mutex<Vec<Alert>>>,
    cooldowns: Arc<Mutex<HashMap<String, DateTime<Utc>>>>,
    alert_counter: Arc<Mutex<u64>>,
}

impl MonitorEngine {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(Mutex::new(Vec::new())),
            alerts: Arc::new(Mutex::new(Vec::new())),
            cooldowns: Arc::new(Mutex::new(HashMap::new())),
            alert_counter: Arc::new(Mutex::new(0)),
        }
    }

    /// Add a monitor rule
    pub async fn add_rule(&self, rule: MonitorRule) {
        self.rules.lock().await.push(rule);
    }

    /// Remove a monitor rule by ID
    pub async fn remove_rule(&self, rule_id: &str) -> bool {
        let mut rules = self.rules.lock().await;
        let len = rules.len();
        rules.retain(|r| r.rule_id != rule_id);
        rules.len() < len
    }

    /// Get all rules
    pub async fn get_rules(&self) -> Vec<MonitorRule> {
        self.rules.lock().await.clone()
    }

    /// Get a rule by ID
    pub async fn get_rule(&self, rule_id: &str) -> Option<MonitorRule> {
        self.rules.lock().await.iter().find(|r| r.rule_id == rule_id).cloned()
    }

    /// Update a rule
    pub async fn update_rule(&self, rule: MonitorRule) -> bool {
        let mut rules = self.rules.lock().await;
        if let Some(existing) = rules.iter_mut().find(|r| r.rule_id == rule.rule_id) {
            *existing = rule;
            true
        } else {
            false
        }
    }

    /// Get all alerts (most recent first)
    pub async fn get_alerts(&self, limit: usize) -> Vec<Alert> {
        let alerts = self.alerts.lock().await;
        alerts.iter().rev().take(limit).cloned().collect()
    }

    /// Acknowledge an alert
    pub async fn acknowledge_alert(&self, alert_id: &str) -> bool {
        let mut alerts = self.alerts.lock().await;
        if let Some(alert) = alerts.iter_mut().find(|a| a.alert_id == alert_id) {
            alert.acknowledged = true;
            true
        } else {
            false
        }
    }

    /// Evaluate all rules against an entity and return any triggered alerts
    pub async fn evaluate(&self, entity: &Entity) -> Vec<Alert> {
        let rules = self.rules.lock().await.clone();
        let mut triggered = Vec::new();

        for rule in &rules {
            if !rule.enabled {
                continue;
            }
            if rule.entity_type != entity.entity_type && rule.entity_type != "*" {
                continue;
            }

            // Check cooldown
            let cooldown_key = format!("{}:{}", rule.rule_id, entity.entity_id);
            {
                let cooldowns = self.cooldowns.lock().await;
                if let Some(last_triggered) = cooldowns.get(&cooldown_key) {
                    let elapsed = (Utc::now() - *last_triggered).num_seconds() as u64;
                    if elapsed < rule.cooldown_seconds {
                        continue;
                    }
                }
            }

            if self.check_condition(&rule.condition, entity) {
                let mut counter = self.alert_counter.lock().await;
                *counter += 1;
                let alert_id = format!("alert-{}", *counter);

                let message = self.format_alert_message(rule, entity);
                let evidence = self.build_evidence(rule, entity);

                let alert = Alert {
                    alert_id,
                    rule_id: rule.rule_id.clone(),
                    rule_name: rule.name.clone(),
                    entity_id: entity.entity_id.clone(),
                    entity_type: entity.entity_type.clone(),
                    severity: rule.severity.clone(),
                    message,
                    evidence,
                    triggered_at: Utc::now(),
                    acknowledged: false,
                };

                triggered.push(alert.clone());
                self.alerts.lock().await.push(alert);

                // Update cooldown
                self.cooldowns
                    .lock()
                    .await
                    .insert(cooldown_key, Utc::now());
            }
        }

        triggered
    }

    /// Create an OrpEvent from an alert
    pub fn alert_to_event(alert: &Alert, source_id: &str) -> OrpEvent {
        let severity = match alert.severity {
            AlertSeverity::Info => EventSeverity::Info,
            AlertSeverity::Warning => EventSeverity::Warning,
            AlertSeverity::Critical => EventSeverity::Critical,
        };

        let alert_severity = match alert.severity {
            AlertSeverity::Info => orp_proto::AlertSeverity::Info,
            AlertSeverity::Warning => orp_proto::AlertSeverity::Warning,
            AlertSeverity::Critical => orp_proto::AlertSeverity::Critical,
        };

        let mut event = OrpEvent::new(
            alert.entity_id.clone(),
            alert.entity_type.clone(),
            EventPayload::AlertTriggered {
                monitor_id: alert.rule_id.clone(),
                severity: alert_severity,
                message: alert.message.clone(),
                evidence: alert.evidence.clone(),
            },
            source_id.to_string(),
            1.0,
        );
        event.severity = Some(severity);
        event
    }

    fn check_condition(&self, condition: &MonitorCondition, entity: &Entity) -> bool {
        match condition {
            MonitorCondition::PropertyThreshold {
                property,
                operator,
                value,
            } => {
                if let Some(prop_val) = entity.properties.get(property).and_then(|v| v.as_f64()) {
                    match operator {
                        ThresholdOp::GreaterThan => prop_val > *value,
                        ThresholdOp::LessThan => prop_val < *value,
                        ThresholdOp::GreaterThanOrEqual => prop_val >= *value,
                        ThresholdOp::LessThanOrEqual => prop_val <= *value,
                        ThresholdOp::Equal => (prop_val - value).abs() < f64::EPSILON,
                        ThresholdOp::NotEqual => (prop_val - value).abs() >= f64::EPSILON,
                    }
                } else {
                    false
                }
            }
            MonitorCondition::Geofence {
                lat,
                lon,
                radius_km,
                trigger_on: _,
            } => {
                if let Some(ref geo) = entity.geometry {
                    let dist = haversine_km(geo.lat, geo.lon, *lat, *lon);
                    dist <= *radius_km
                } else {
                    false
                }
            }
            MonitorCondition::Stale { max_age_seconds } => {
                let age = (Utc::now() - entity.last_updated).num_seconds() as u64;
                age > *max_age_seconds
            }
            MonitorCondition::SpeedAnomaly {
                max_change_knots: _,
            } => {
                // Placeholder — would need previous speed to compare
                false
            }
        }
    }

    fn format_alert_message(&self, rule: &MonitorRule, entity: &Entity) -> String {
        let name = entity
            .name
            .as_deref()
            .unwrap_or(&entity.entity_id);
        format!(
            "[{}] {} triggered for '{}' ({})",
            match rule.severity {
                AlertSeverity::Info => "INFO",
                AlertSeverity::Warning => "WARNING",
                AlertSeverity::Critical => "CRITICAL",
            },
            rule.name,
            name,
            entity.entity_id,
        )
    }

    fn build_evidence(&self, rule: &MonitorRule, entity: &Entity) -> serde_json::Value {
        match &rule.condition {
            MonitorCondition::PropertyThreshold { property, .. } => {
                serde_json::json!({
                    "rule_id": rule.rule_id,
                    "property": property,
                    "current_value": entity.properties.get(property),
                    "entity_id": entity.entity_id,
                })
            }
            MonitorCondition::Geofence {
                lat, lon, radius_km, ..
            } => {
                serde_json::json!({
                    "rule_id": rule.rule_id,
                    "geofence_center": [lat, lon],
                    "radius_km": radius_km,
                    "entity_position": entity.geometry.as_ref().map(|g| [g.lat, g.lon]),
                    "entity_id": entity.entity_id,
                })
            }
            _ => {
                serde_json::json!({
                    "rule_id": rule.rule_id,
                    "entity_id": entity.entity_id,
                })
            }
        }
    }
}

impl Default for MonitorEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    r * c
}

#[cfg(test)]
mod tests {
    use super::*;
    use orp_proto::GeoPoint;

    fn make_ship(id: &str, speed: f64, lat: f64, lon: f64) -> Entity {
        let mut properties = HashMap::new();
        properties.insert("speed".to_string(), serde_json::json!(speed));
        Entity {
            entity_id: id.to_string(),
            entity_type: "ship".to_string(),
            name: Some(format!("Ship {}", id)),
            geometry: Some(GeoPoint {
                lat,
                lon,
                alt: None,
            }),
            properties,
            ..Entity::default()
        }
    }

    #[tokio::test]
    async fn test_speed_threshold_alert() {
        let engine = MonitorEngine::new();
        engine
            .add_rule(MonitorRule {
                rule_id: "speed-alert".to_string(),
                name: "High speed alert".to_string(),
                description: "Alert when ship exceeds 25 knots".to_string(),
                entity_type: "ship".to_string(),
                condition: MonitorCondition::PropertyThreshold {
                    property: "speed".to_string(),
                    operator: ThresholdOp::GreaterThan,
                    value: 25.0,
                },
                action: MonitorAction::Alert,
                enabled: true,
                cooldown_seconds: 0,
                severity: AlertSeverity::Warning,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;

        // Ship under threshold
        let slow = make_ship("ship-1", 10.0, 51.92, 4.48);
        let alerts = engine.evaluate(&slow).await;
        assert!(alerts.is_empty());

        // Ship over threshold
        let fast = make_ship("ship-2", 30.0, 51.92, 4.48);
        let alerts = engine.evaluate(&fast).await;
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "speed-alert");
    }

    #[tokio::test]
    async fn test_geofence_alert() {
        let engine = MonitorEngine::new();
        engine
            .add_rule(MonitorRule {
                rule_id: "geofence-rotterdam".to_string(),
                name: "Rotterdam port area".to_string(),
                description: "Alert when ship enters Rotterdam port".to_string(),
                entity_type: "ship".to_string(),
                condition: MonitorCondition::Geofence {
                    lat: 51.92,
                    lon: 4.48,
                    radius_km: 10.0,
                    trigger_on: GeofenceTrigger::Enter,
                },
                action: MonitorAction::Alert,
                enabled: true,
                cooldown_seconds: 0,
                severity: AlertSeverity::Info,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;

        // Ship inside geofence
        let inside = make_ship("ship-1", 10.0, 51.92, 4.48);
        let alerts = engine.evaluate(&inside).await;
        assert_eq!(alerts.len(), 1);

        // Ship outside geofence
        let outside = make_ship("ship-2", 10.0, 35.0, 139.0);
        let alerts = engine.evaluate(&outside).await;
        assert!(alerts.is_empty());
    }

    #[tokio::test]
    async fn test_cooldown() {
        let engine = MonitorEngine::new();
        engine
            .add_rule(MonitorRule {
                rule_id: "speed-alert".to_string(),
                name: "Speed alert".to_string(),
                description: "".to_string(),
                entity_type: "ship".to_string(),
                condition: MonitorCondition::PropertyThreshold {
                    property: "speed".to_string(),
                    operator: ThresholdOp::GreaterThan,
                    value: 20.0,
                },
                action: MonitorAction::Alert,
                enabled: true,
                cooldown_seconds: 3600, // 1 hour cooldown
                severity: AlertSeverity::Warning,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;

        let fast = make_ship("ship-1", 25.0, 51.92, 4.48);

        // First evaluation should trigger
        let alerts = engine.evaluate(&fast).await;
        assert_eq!(alerts.len(), 1);

        // Second evaluation should be suppressed by cooldown
        let alerts = engine.evaluate(&fast).await;
        assert!(alerts.is_empty());
    }

    #[tokio::test]
    async fn test_disabled_rule() {
        let engine = MonitorEngine::new();
        engine
            .add_rule(MonitorRule {
                rule_id: "disabled".to_string(),
                name: "Disabled".to_string(),
                description: "".to_string(),
                entity_type: "ship".to_string(),
                condition: MonitorCondition::PropertyThreshold {
                    property: "speed".to_string(),
                    operator: ThresholdOp::GreaterThan,
                    value: 0.0,
                },
                action: MonitorAction::Alert,
                enabled: false,
                cooldown_seconds: 0,
                severity: AlertSeverity::Info,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;

        let ship = make_ship("ship-1", 10.0, 51.92, 4.48);
        let alerts = engine.evaluate(&ship).await;
        assert!(alerts.is_empty());
    }

    #[tokio::test]
    async fn test_alert_to_event() {
        let alert = Alert {
            alert_id: "alert-1".to_string(),
            rule_id: "rule-1".to_string(),
            rule_name: "Speed alert".to_string(),
            entity_id: "ship-1".to_string(),
            entity_type: "ship".to_string(),
            severity: AlertSeverity::Critical,
            message: "Test alert".to_string(),
            evidence: serde_json::json!({}),
            triggered_at: Utc::now(),
            acknowledged: false,
        };

        let event = MonitorEngine::alert_to_event(&alert, "monitor-engine");
        assert_eq!(event.entity_id, "ship-1");
        assert_eq!(event.event_type_str(), "alert_triggered");
        assert!(event.severity.is_some());
    }

    #[tokio::test]
    async fn test_rule_crud() {
        let engine = MonitorEngine::new();

        let rule = MonitorRule {
            rule_id: "test-rule".to_string(),
            name: "Test".to_string(),
            description: "".to_string(),
            entity_type: "ship".to_string(),
            condition: MonitorCondition::PropertyThreshold {
                property: "speed".to_string(),
                operator: ThresholdOp::GreaterThan,
                value: 10.0,
            },
            action: MonitorAction::Alert,
            enabled: true,
            cooldown_seconds: 0,
            severity: AlertSeverity::Info,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        engine.add_rule(rule).await;
        assert_eq!(engine.get_rules().await.len(), 1);

        assert!(engine.get_rule("test-rule").await.is_some());
        assert!(engine.get_rule("nonexistent").await.is_none());

        assert!(engine.remove_rule("test-rule").await);
        assert!(engine.get_rules().await.is_empty());
    }

    #[tokio::test]
    async fn test_remove_nonexistent_rule() {
        let engine = MonitorEngine::new();
        assert!(!engine.remove_rule("nonexistent").await);
    }

    #[tokio::test]
    async fn test_wrong_entity_type_no_alert() {
        let engine = MonitorEngine::new();
        engine
            .add_rule(MonitorRule {
                rule_id: "ship-only".to_string(),
                name: "Ship speed".to_string(),
                description: "".to_string(),
                entity_type: "ship".to_string(),
                condition: MonitorCondition::PropertyThreshold {
                    property: "speed".to_string(),
                    operator: ThresholdOp::GreaterThan,
                    value: 0.0,
                },
                action: MonitorAction::Alert,
                enabled: true,
                cooldown_seconds: 0,
                severity: AlertSeverity::Info,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;

        let aircraft = Entity {
            entity_id: "aircraft-1".to_string(),
            entity_type: "aircraft".to_string(),
            properties: {
                let mut p = HashMap::new();
                p.insert("speed".to_string(), serde_json::json!(500.0));
                p
            },
            ..Entity::default()
        };
        let alerts = engine.evaluate(&aircraft).await;
        assert!(alerts.is_empty());
    }

    #[tokio::test]
    async fn test_less_than_operator() {
        let engine = MonitorEngine::new();
        engine
            .add_rule(MonitorRule {
                rule_id: "slow".to_string(),
                name: "Slow ship".to_string(),
                description: "".to_string(),
                entity_type: "ship".to_string(),
                condition: MonitorCondition::PropertyThreshold {
                    property: "speed".to_string(),
                    operator: ThresholdOp::LessThan,
                    value: 5.0,
                },
                action: MonitorAction::Alert,
                enabled: true,
                cooldown_seconds: 0,
                severity: AlertSeverity::Warning,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;

        let slow = make_ship("slow-1", 2.0, 51.92, 4.48);
        let alerts = engine.evaluate(&slow).await;
        assert_eq!(alerts.len(), 1);

        let fast = make_ship("fast-1", 10.0, 51.92, 4.48);
        let alerts = engine.evaluate(&fast).await;
        assert!(alerts.is_empty());
    }

    #[tokio::test]
    async fn test_acknowledge_alert() {
        let engine = MonitorEngine::new();
        engine
            .add_rule(MonitorRule {
                rule_id: "ack-test".to_string(),
                name: "Ack test".to_string(),
                description: "".to_string(),
                entity_type: "ship".to_string(),
                condition: MonitorCondition::PropertyThreshold {
                    property: "speed".to_string(),
                    operator: ThresholdOp::GreaterThan,
                    value: 0.0,
                },
                action: MonitorAction::Alert,
                enabled: true,
                cooldown_seconds: 0,
                severity: AlertSeverity::Info,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;

        let ship = make_ship("ack-ship", 10.0, 51.92, 4.48);
        let alerts = engine.evaluate(&ship).await;
        assert!(!alerts.is_empty());

        let alert_id = &alerts[0].alert_id;
        assert!(engine.acknowledge_alert(alert_id).await);
        assert!(!engine.acknowledge_alert("nonexistent").await);
    }

    #[tokio::test]
    async fn test_get_alerts_limit() {
        let engine = MonitorEngine::new();
        engine
            .add_rule(MonitorRule {
                rule_id: "many".to_string(),
                name: "Many alerts".to_string(),
                description: "".to_string(),
                entity_type: "ship".to_string(),
                condition: MonitorCondition::PropertyThreshold {
                    property: "speed".to_string(),
                    operator: ThresholdOp::GreaterThan,
                    value: 0.0,
                },
                action: MonitorAction::Alert,
                enabled: true,
                cooldown_seconds: 0,
                severity: AlertSeverity::Info,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;

        for i in 0..5 {
            let ship = make_ship(&format!("ship-{}", i), 10.0 + i as f64, 51.92, 4.48);
            engine.evaluate(&ship).await;
        }

        let alerts = engine.get_alerts(3).await;
        assert!(alerts.len() <= 3);
    }
}
