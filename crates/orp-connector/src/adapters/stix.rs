use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// STIX 2.1 object model
// ---------------------------------------------------------------------------
// STIX (Structured Threat Information Expression) is a JSON-based format for
// cyber threat intelligence. Key object types:
//   - indicator: IOC (Indicator of Compromise)
//   - malware: malware description
//   - threat-actor: attributed threat actor/group
//   - attack-pattern: TTP (Tactic, Technique, Procedure)
//   - vulnerability: CVE or similar weakness
//   - campaign: coordinated threat activity
//   - intrusion-set: grouped threat activity
//   - tool: legitimate software used maliciously
//   - observed-data: raw observations
//   - relationship: links between STIX objects

/// A STIX 2.1 bundle containing multiple objects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StixBundle {
    #[serde(rename = "type")]
    pub bundle_type: String,
    pub id: String,
    #[serde(default)]
    pub objects: Vec<StixObject>,
}

fn default_spec_version() -> String {
    "2.1".to_string()
}

/// A single STIX 2.1 object (generic representation).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StixObject {
    #[serde(rename = "type")]
    pub object_type: String,
    pub id: String,
    /// STIX 2.1 spec_version field (required per spec, defaults to "2.1" for leniency).
    #[serde(default = "default_spec_version")]
    pub spec_version: String,
    /// Created timestamp (required per STIX 2.1 spec for SDOs).
    #[serde(default)]
    pub created: String,
    /// Modified timestamp (required per STIX 2.1 spec for SDOs).
    #[serde(default)]
    pub modified: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    // Indicator-specific
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub pattern_type: Option<String>,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_until: Option<String>,
    // Malware-specific
    #[serde(default)]
    pub is_family: Option<bool>,
    #[serde(default)]
    pub malware_types: Option<Vec<String>>,
    // Vulnerability-specific
    #[serde(default)]
    pub external_references: Option<Vec<ExternalReference>>,
    // Threat-actor-specific
    #[serde(default)]
    pub threat_actor_types: Option<Vec<String>>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub sophistication: Option<String>,
    // Attack-pattern-specific
    #[serde(default)]
    pub kill_chain_phases: Option<Vec<KillChainPhase>>,
    // Relationship
    #[serde(default)]
    pub relationship_type: Option<String>,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub target_ref: Option<String>,
    // Campaign
    #[serde(default)]
    pub first_seen: Option<String>,
    #[serde(default)]
    pub last_seen: Option<String>,
    // Confidence
    #[serde(default)]
    pub confidence: Option<u32>,
    // Labels / tags
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    // Catch-all for extension fields
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalReference {
    pub source_name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KillChainPhase {
    pub kill_chain_name: String,
    pub phase_name: String,
}

// ---------------------------------------------------------------------------
// STIX type → ORP entity type mapping
// ---------------------------------------------------------------------------

/// Map STIX object type to ORP entity type.
pub fn stix_type_to_entity(stix_type: &str) -> &'static str {
    match stix_type {
        "indicator" => "threat_indicator",
        "malware" => "threat",
        "threat-actor" => "threat_actor",
        "attack-pattern" => "attack_pattern",
        "vulnerability" => "vulnerability",
        "campaign" => "campaign",
        "intrusion-set" => "intrusion_set",
        "tool" => "tool",
        "observed-data" => "observation",
        "identity" => "identity",
        "infrastructure" => "infrastructure",
        "location" => "location",
        "note" => "note",
        "opinion" => "opinion",
        "report" => "report",
        "relationship" => "relationship",
        "sighting" => "sighting",
        _ => "stix_object",
    }
}

// ---------------------------------------------------------------------------
// STIX JSON parser
// ---------------------------------------------------------------------------

/// Parse a STIX 2.1 bundle from a JSON string.
pub fn parse_stix_bundle(json: &str) -> Result<StixBundle, ConnectorError> {
    serde_json::from_str(json).map_err(|e| {
        ConnectorError::ParseError(format!("STIX bundle parse error: {}", e))
    })
}

/// Parse a single STIX 2.1 object from a JSON string.
pub fn parse_stix_object(json: &str) -> Result<StixObject, ConnectorError> {
    serde_json::from_str(json).map_err(|e| {
        ConnectorError::ParseError(format!("STIX object parse error: {}", e))
    })
}

// ---------------------------------------------------------------------------
// StixObject → SourceEvent
// ---------------------------------------------------------------------------

impl StixObject {
    /// Convert to ORP SourceEvent.
    pub fn to_source_event(&self, connector_id: &str) -> SourceEvent {
        let entity_type = stix_type_to_entity(&self.object_type);

        let mut properties: HashMap<String, serde_json::Value> = HashMap::new();
        properties.insert(
            "stix_type".into(),
            serde_json::json!(self.object_type),
        );
        properties.insert("stix_id".into(), serde_json::json!(self.id));
        properties.insert("spec_version".into(), serde_json::json!(self.spec_version));

        if let Some(ref name) = self.name {
            properties.insert("name".into(), serde_json::json!(name));
        }
        if let Some(ref desc) = self.description {
            properties.insert("description".into(), serde_json::json!(desc));
        }
        if let Some(ref pattern) = self.pattern {
            properties.insert("pattern".into(), serde_json::json!(pattern));
        }
        if let Some(ref pt) = self.pattern_type {
            properties.insert("pattern_type".into(), serde_json::json!(pt));
        }
        if let Some(ref vf) = self.valid_from {
            properties.insert("valid_from".into(), serde_json::json!(vf));
        }
        if let Some(ref vu) = self.valid_until {
            properties.insert("valid_until".into(), serde_json::json!(vu));
        }
        if let Some(is_fam) = self.is_family {
            properties.insert("is_family".into(), serde_json::json!(is_fam));
        }
        if let Some(ref mt) = self.malware_types {
            properties.insert("malware_types".into(), serde_json::json!(mt));
        }
        if let Some(ref ta) = self.threat_actor_types {
            properties.insert(
                "threat_actor_types".into(),
                serde_json::json!(ta),
            );
        }
        if let Some(ref aliases) = self.aliases {
            properties.insert("aliases".into(), serde_json::json!(aliases));
        }
        if let Some(ref soph) = self.sophistication {
            properties.insert(
                "sophistication".into(),
                serde_json::json!(soph),
            );
        }
        if let Some(ref kc) = self.kill_chain_phases {
            let kc_json: Vec<serde_json::Value> = kc
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "kill_chain_name": p.kill_chain_name,
                        "phase_name": p.phase_name,
                    })
                })
                .collect();
            properties.insert("kill_chain_phases".into(), serde_json::json!(kc_json));
        }
        if let Some(ref rt) = self.relationship_type {
            properties.insert(
                "relationship_type".into(),
                serde_json::json!(rt),
            );
        }
        if let Some(ref sr) = self.source_ref {
            properties.insert("source_ref".into(), serde_json::json!(sr));
        }
        if let Some(ref tr) = self.target_ref {
            properties.insert("target_ref".into(), serde_json::json!(tr));
        }
        if let Some(conf) = self.confidence {
            properties.insert("confidence".into(), serde_json::json!(conf));
        }
        if let Some(ref labels) = self.labels {
            properties.insert("labels".into(), serde_json::json!(labels));
        }
        if let Some(ref refs) = self.external_references {
            let refs_json: Vec<serde_json::Value> = refs
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "source_name": r.source_name,
                        "url": r.url,
                        "external_id": r.external_id,
                    })
                })
                .collect();
            properties.insert(
                "external_references".into(),
                serde_json::json!(refs_json),
            );
        }
        if let Some(ref fs) = self.first_seen {
            properties.insert("first_seen".into(), serde_json::json!(fs));
        }
        if let Some(ref ls) = self.last_seen {
            properties.insert("last_seen".into(), serde_json::json!(ls));
        }

        let ts = if self.created.is_empty() {
            Utc::now()
        } else {
            chrono::DateTime::parse_from_rfc3339(&self.created)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now)
        };

        SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id: format!("stix:{}", self.id),
            entity_type: entity_type.to_string(),
            properties,
            timestamp: ts,
            latitude: None,
            longitude: None,
        }
    }
}

// ---------------------------------------------------------------------------
// TAXII 2.1 response models
// ---------------------------------------------------------------------------

/// TAXII 2.1 discovery response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaxiiDiscovery {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub api_roots: Option<Vec<String>>,
}

/// TAXII 2.1 collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaxiiCollection {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub can_read: Option<bool>,
    #[serde(default)]
    pub can_write: Option<bool>,
}

/// TAXII 2.1 collections response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaxiiCollections {
    pub collections: Vec<TaxiiCollection>,
}

/// Parse TAXII discovery response.
pub fn parse_taxii_discovery(json: &str) -> Result<TaxiiDiscovery, ConnectorError> {
    serde_json::from_str(json).map_err(|e| {
        ConnectorError::ParseError(format!("TAXII discovery parse error: {}", e))
    })
}

/// Parse TAXII collections response.
pub fn parse_taxii_collections(
    json: &str,
) -> Result<TaxiiCollections, ConnectorError> {
    serde_json::from_str(json).map_err(|e| {
        ConnectorError::ParseError(format!(
            "TAXII collections parse error: {}",
            e
        ))
    })
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// STIX/TAXII connector — polls TAXII 2.1 server for STIX 2.1 threat intel.
pub struct StixConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl StixConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl Connector for StixConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "STIX/TAXII connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let connector_id = self.config.connector_id.clone();
        let url = self.config.url.clone();
        let props = self.config.properties.clone();

        tokio::spawn(async move {
            // If a TAXII server URL is configured, poll it
            if let Some(ref base_url) = url {
                let client = reqwest::Client::new();
                let poll_secs = props
                    .get("poll_interval_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(300);
                let collection_id = props
                    .get("collection_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let api_root = props
                    .get("api_root")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let objects_url = format!(
                    "{}{}/collections/{}/objects/",
                    base_url, api_root, collection_id
                );

                let mut interval = tokio::time::interval(
                    tokio::time::Duration::from_secs(poll_secs),
                );

                while running.load(Ordering::SeqCst) {
                    interval.tick().await;
                    match client
                        .get(&objects_url)
                        .header("Accept", "application/stix+json;version=2.1")
                        .send()
                        .await
                    {
                        Ok(resp) => match resp.text().await {
                            Ok(body) => match parse_stix_bundle(&body) {
                                Ok(bundle) => {
                                    for obj in &bundle.objects {
                                        let event =
                                            obj.to_source_event(&connector_id);
                                        if tx.send(event).await.is_err() {
                                            return;
                                        }
                                        events_count
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("STIX parse error: {}", e);
                                    errors_count
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                            },
                            Err(e) => {
                                tracing::warn!("TAXII response error: {}", e);
                                errors_count
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Err(e) => {
                            tracing::warn!("TAXII request error: {}", e);
                            errors_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                return;
            }

            // Demo mode: idle
            while running.load(Ordering::SeqCst) {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "STIX/TAXII connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "STIX/TAXII connector not running".to_string(),
            ))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
            last_event_timestamp: Some(Utc::now()),
            uptime_seconds: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bundle() -> &'static str {
        r#"{
  "type": "bundle",
  "id": "bundle--a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "objects": [
    {
      "type": "indicator",
      "id": "indicator--1234",
      "created": "2026-03-26T10:00:00Z",
      "modified": "2026-03-26T10:00:00Z",
      "name": "Malicious IP",
      "description": "Known C2 server",
      "pattern": "[ipv4-addr:value = '203.0.113.1']",
      "pattern_type": "stix",
      "valid_from": "2026-03-25T00:00:00Z",
      "confidence": 85
    },
    {
      "type": "malware",
      "id": "malware--5678",
      "created": "2026-03-26T08:00:00Z",
      "modified": "2026-03-26T08:00:00Z",
      "name": "DarkRat",
      "is_family": true,
      "malware_types": ["remote-access-trojan", "backdoor"],
      "description": "Multi-stage RAT targeting energy sector"
    },
    {
      "type": "threat-actor",
      "id": "threat-actor--9999",
      "created": "2026-03-20T00:00:00Z",
      "modified": "2026-03-25T00:00:00Z",
      "name": "APT-Phantom",
      "threat_actor_types": ["nation-state"],
      "aliases": ["Phantom Group", "TA-505X"],
      "sophistication": "expert"
    },
    {
      "type": "relationship",
      "id": "relationship--rel1",
      "created": "2026-03-26T10:00:00Z",
      "relationship_type": "uses",
      "source_ref": "threat-actor--9999",
      "target_ref": "malware--5678"
    }
  ]
}"#
    }

    #[test]
    fn test_parse_stix_bundle() {
        let bundle = parse_stix_bundle(sample_bundle()).unwrap();
        assert_eq!(bundle.bundle_type, "bundle");
        assert_eq!(bundle.objects.len(), 4);
    }

    #[test]
    fn test_parse_indicator() {
        let bundle = parse_stix_bundle(sample_bundle()).unwrap();
        let indicator = &bundle.objects[0];
        assert_eq!(indicator.object_type, "indicator");
        assert_eq!(indicator.name, Some("Malicious IP".into()));
        assert_eq!(
            indicator.pattern,
            Some("[ipv4-addr:value = '203.0.113.1']".into())
        );
        assert_eq!(indicator.pattern_type, Some("stix".into()));
        assert_eq!(indicator.confidence, Some(85));
    }

    #[test]
    fn test_parse_malware() {
        let bundle = parse_stix_bundle(sample_bundle()).unwrap();
        let malware = &bundle.objects[1];
        assert_eq!(malware.object_type, "malware");
        assert_eq!(malware.name, Some("DarkRat".into()));
        assert_eq!(malware.is_family, Some(true));
        assert_eq!(
            malware.malware_types,
            Some(vec![
                "remote-access-trojan".into(),
                "backdoor".into()
            ])
        );
    }

    #[test]
    fn test_parse_threat_actor() {
        let bundle = parse_stix_bundle(sample_bundle()).unwrap();
        let ta = &bundle.objects[2];
        assert_eq!(ta.object_type, "threat-actor");
        assert_eq!(ta.name, Some("APT-Phantom".into()));
        assert_eq!(
            ta.threat_actor_types,
            Some(vec!["nation-state".into()])
        );
        assert_eq!(ta.sophistication, Some("expert".into()));
    }

    #[test]
    fn test_parse_relationship() {
        let bundle = parse_stix_bundle(sample_bundle()).unwrap();
        let rel = &bundle.objects[3];
        assert_eq!(rel.object_type, "relationship");
        assert_eq!(rel.relationship_type, Some("uses".into()));
        assert_eq!(
            rel.source_ref,
            Some("threat-actor--9999".into())
        );
        assert_eq!(rel.target_ref, Some("malware--5678".into()));
    }

    #[test]
    fn test_stix_type_to_entity_mapping() {
        assert_eq!(stix_type_to_entity("indicator"), "threat_indicator");
        assert_eq!(stix_type_to_entity("malware"), "threat");
        assert_eq!(stix_type_to_entity("threat-actor"), "threat_actor");
        assert_eq!(stix_type_to_entity("attack-pattern"), "attack_pattern");
        assert_eq!(stix_type_to_entity("vulnerability"), "vulnerability");
        assert_eq!(stix_type_to_entity("campaign"), "campaign");
        assert_eq!(stix_type_to_entity("relationship"), "relationship");
        assert_eq!(stix_type_to_entity("unknown-type"), "stix_object");
    }

    #[test]
    fn test_indicator_to_source_event() {
        let bundle = parse_stix_bundle(sample_bundle()).unwrap();
        let event = bundle.objects[0].to_source_event("stix-test");
        assert_eq!(event.entity_id, "stix:indicator--1234");
        assert_eq!(event.entity_type, "threat_indicator");
        assert_eq!(
            event.properties.get("pattern").unwrap(),
            &serde_json::json!("[ipv4-addr:value = '203.0.113.1']")
        );
        assert_eq!(
            event.properties.get("confidence").unwrap(),
            &serde_json::json!(85)
        );
    }

    #[test]
    fn test_malware_to_source_event() {
        let bundle = parse_stix_bundle(sample_bundle()).unwrap();
        let event = bundle.objects[1].to_source_event("stix-test");
        assert_eq!(event.entity_type, "threat");
        assert_eq!(
            event.properties.get("name").unwrap(),
            &serde_json::json!("DarkRat")
        );
        assert_eq!(
            event.properties.get("is_family").unwrap(),
            &serde_json::json!(true)
        );
    }

    #[test]
    fn test_threat_actor_to_source_event() {
        let bundle = parse_stix_bundle(sample_bundle()).unwrap();
        let event = bundle.objects[2].to_source_event("stix-test");
        assert_eq!(event.entity_type, "threat_actor");
        assert!(event.properties.contains_key("aliases"));
    }

    #[test]
    fn test_parse_stix_object_single() {
        let json = r#"{
            "type": "vulnerability",
            "id": "vulnerability--CVE-2026-0001",
            "created": "2026-01-15T00:00:00Z",
            "name": "CVE-2026-0001",
            "description": "Critical RCE in widget framework",
            "external_references": [
                {
                    "source_name": "cve",
                    "external_id": "CVE-2026-0001",
                    "url": "https://cve.mitre.org/cgi-bin/cvename.cgi?name=CVE-2026-0001"
                }
            ]
        }"#;
        let obj = parse_stix_object(json).unwrap();
        assert_eq!(obj.object_type, "vulnerability");
        assert_eq!(obj.name, Some("CVE-2026-0001".into()));
        assert!(obj.external_references.is_some());
    }

    #[test]
    fn test_vulnerability_to_source_event() {
        let json = r#"{
            "type": "vulnerability",
            "id": "vulnerability--CVE-2026-0001",
            "created": "2026-01-15T00:00:00Z",
            "name": "CVE-2026-0001",
            "external_references": [
                {"source_name": "cve", "external_id": "CVE-2026-0001"}
            ]
        }"#;
        let obj = parse_stix_object(json).unwrap();
        let event = obj.to_source_event("stix-test");
        assert_eq!(event.entity_type, "vulnerability");
        assert!(event.properties.contains_key("external_references"));
    }

    #[test]
    fn test_parse_attack_pattern() {
        let json = r#"{
            "type": "attack-pattern",
            "id": "attack-pattern--T1059",
            "created": "2026-01-01T00:00:00Z",
            "name": "Command and Scripting Interpreter",
            "kill_chain_phases": [
                {"kill_chain_name": "mitre-attack", "phase_name": "execution"}
            ]
        }"#;
        let obj = parse_stix_object(json).unwrap();
        assert_eq!(obj.object_type, "attack-pattern");
        let event = obj.to_source_event("stix-test");
        assert_eq!(event.entity_type, "attack_pattern");
        assert!(event.properties.contains_key("kill_chain_phases"));
    }

    #[test]
    fn test_parse_invalid_json() {
        assert!(parse_stix_bundle("{invalid}").is_err());
        assert!(parse_stix_object("{invalid}").is_err());
    }

    #[test]
    fn test_parse_taxii_discovery() {
        let json = r#"{
            "title": "TAXII Server",
            "description": "Test TAXII 2.1 Server",
            "default": "https://example.com/taxii/",
            "api_roots": ["https://example.com/api1/", "https://example.com/api2/"]
        }"#;
        let disc = parse_taxii_discovery(json).unwrap();
        assert_eq!(disc.title, "TAXII Server");
        assert_eq!(disc.api_roots.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_parse_taxii_collections() {
        let json = r#"{
            "collections": [
                {"id": "col-1", "title": "Threat Intel Feed", "can_read": true, "can_write": false},
                {"id": "col-2", "title": "Internal IOCs", "can_read": true, "can_write": true}
            ]
        }"#;
        let cols = parse_taxii_collections(json).unwrap();
        assert_eq!(cols.collections.len(), 2);
        assert_eq!(cols.collections[0].title, "Threat Intel Feed");
        assert_eq!(cols.collections[1].can_write, Some(true));
    }

    #[test]
    fn test_stix_connector_id() {
        let config = ConnectorConfig {
            connector_id: "stix-1".to_string(),
            connector_type: "stix".to_string(),
            url: None,
            entity_type: "threat".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = StixConnector::new(config);
        assert_eq!(connector.connector_id(), "stix-1");
    }

    #[tokio::test]
    async fn test_stix_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "stix-health".to_string(),
            connector_type: "stix".to_string(),
            url: None,
            entity_type: "threat".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = StixConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_campaign_to_source_event() {
        let json = r#"{
            "type": "campaign",
            "id": "campaign--c1",
            "created": "2026-03-01T00:00:00Z",
            "name": "Operation Moonlight",
            "first_seen": "2025-12-01T00:00:00Z",
            "last_seen": "2026-03-15T00:00:00Z",
            "labels": ["apt", "espionage"]
        }"#;
        let obj = parse_stix_object(json).unwrap();
        let event = obj.to_source_event("stix-test");
        assert_eq!(event.entity_type, "campaign");
        assert!(event.properties.contains_key("first_seen"));
        assert!(event.properties.contains_key("labels"));
    }
}
