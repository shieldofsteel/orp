use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use quick_xml::events::Event as XmlEvent;
use quick_xml::Reader;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CAP (Common Alerting Protocol) 1.2 model
// ---------------------------------------------------------------------------
// CAP is an XML format for emergency alerts (OASIS / ITU-T X.1303).
// Structure:
//   <alert>
//     <identifier>, <sender>, <sent>, <status>, <msgType>, <scope>
//     <info> (0..N)
//       <category>, <event>, <urgency>, <severity>, <certainty>
//       <headline>, <description>, <instruction>
//       <area> (0..N)
//         <areaDesc>, <polygon>, <circle>, <geocode>

/// Parsed CAP alert.
#[derive(Clone, Debug)]
pub struct CapAlert {
    pub identifier: String,
    pub sender: String,
    pub sent: String,
    pub status: CapStatus,
    pub msg_type: CapMsgType,
    pub scope: CapScope,
    pub source: Option<String>,
    pub note: Option<String>,
    pub infos: Vec<CapInfo>,
}

/// Parsed CAP info element.
#[derive(Clone, Debug)]
pub struct CapInfo {
    pub language: Option<String>,
    pub category: CapCategory,
    pub event: String,
    pub urgency: CapUrgency,
    pub severity: CapSeverity,
    pub certainty: CapCertainty,
    pub headline: Option<String>,
    pub description: Option<String>,
    pub instruction: Option<String>,
    pub effective: Option<String>,
    pub expires: Option<String>,
    pub sender_name: Option<String>,
    pub areas: Vec<CapArea>,
}

/// CAP area element.
#[derive(Clone, Debug)]
pub struct CapArea {
    pub area_desc: String,
    pub polygons: Vec<String>,
    pub circles: Vec<String>,
    pub geocodes: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// CAP enumerations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum CapStatus {
    Actual,
    Exercise,
    System,
    Test,
    Draft,
    Unknown(String),
}

impl CapStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "Actual" => CapStatus::Actual,
            "Exercise" => CapStatus::Exercise,
            "System" => CapStatus::System,
            "Test" => CapStatus::Test,
            "Draft" => CapStatus::Draft,
            _ => CapStatus::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            CapStatus::Actual => "Actual",
            CapStatus::Exercise => "Exercise",
            CapStatus::System => "System",
            CapStatus::Test => "Test",
            CapStatus::Draft => "Draft",
            CapStatus::Unknown(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CapMsgType {
    Alert,
    Update,
    Cancel,
    Ack,
    Error,
    Unknown(String),
}

impl CapMsgType {
    pub fn parse(s: &str) -> Self {
        match s {
            "Alert" => CapMsgType::Alert,
            "Update" => CapMsgType::Update,
            "Cancel" => CapMsgType::Cancel,
            "Ack" => CapMsgType::Ack,
            "Error" => CapMsgType::Error,
            _ => CapMsgType::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            CapMsgType::Alert => "Alert",
            CapMsgType::Update => "Update",
            CapMsgType::Cancel => "Cancel",
            CapMsgType::Ack => "Ack",
            CapMsgType::Error => "Error",
            CapMsgType::Unknown(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CapScope {
    Public,
    Restricted,
    Private,
    Unknown(String),
}

impl CapScope {
    pub fn parse(s: &str) -> Self {
        match s {
            "Public" => CapScope::Public,
            "Restricted" => CapScope::Restricted,
            "Private" => CapScope::Private,
            _ => CapScope::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            CapScope::Public => "Public",
            CapScope::Restricted => "Restricted",
            CapScope::Private => "Private",
            CapScope::Unknown(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CapCategory {
    Geo,
    Met,
    Safety,
    Security,
    Rescue,
    Fire,
    Health,
    Env,
    Transport,
    Infra,
    Cbrne,
    Other,
    Unknown(String),
}

impl CapCategory {
    pub fn parse(s: &str) -> Self {
        match s {
            "Geo" => CapCategory::Geo,
            "Met" => CapCategory::Met,
            "Safety" => CapCategory::Safety,
            "Security" => CapCategory::Security,
            "Rescue" => CapCategory::Rescue,
            "Fire" => CapCategory::Fire,
            "Health" => CapCategory::Health,
            "Env" => CapCategory::Env,
            "Transport" => CapCategory::Transport,
            "Infra" => CapCategory::Infra,
            "CBRNE" => CapCategory::Cbrne,
            "Other" => CapCategory::Other,
            _ => CapCategory::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            CapCategory::Geo => "Geo",
            CapCategory::Met => "Met",
            CapCategory::Safety => "Safety",
            CapCategory::Security => "Security",
            CapCategory::Rescue => "Rescue",
            CapCategory::Fire => "Fire",
            CapCategory::Health => "Health",
            CapCategory::Env => "Env",
            CapCategory::Transport => "Transport",
            CapCategory::Infra => "Infra",
            CapCategory::Cbrne => "CBRNE",
            CapCategory::Other => "Other",
            CapCategory::Unknown(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CapSeverity {
    Extreme,
    Severe,
    Moderate,
    Minor,
    Unknown(String),
}

impl CapSeverity {
    pub fn parse(s: &str) -> Self {
        match s {
            "Extreme" => CapSeverity::Extreme,
            "Severe" => CapSeverity::Severe,
            "Moderate" => CapSeverity::Moderate,
            "Minor" => CapSeverity::Minor,
            _ => CapSeverity::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            CapSeverity::Extreme => "Extreme",
            CapSeverity::Severe => "Severe",
            CapSeverity::Moderate => "Moderate",
            CapSeverity::Minor => "Minor",
            CapSeverity::Unknown(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CapUrgency {
    Immediate,
    Expected,
    Future,
    Past,
    Unknown(String),
}

impl CapUrgency {
    pub fn parse(s: &str) -> Self {
        match s {
            "Immediate" => CapUrgency::Immediate,
            "Expected" => CapUrgency::Expected,
            "Future" => CapUrgency::Future,
            "Past" => CapUrgency::Past,
            _ => CapUrgency::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            CapUrgency::Immediate => "Immediate",
            CapUrgency::Expected => "Expected",
            CapUrgency::Future => "Future",
            CapUrgency::Past => "Past",
            CapUrgency::Unknown(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CapCertainty {
    Observed,
    Likely,
    Possible,
    Unlikely,
    Unknown(String),
}

impl CapCertainty {
    pub fn parse(s: &str) -> Self {
        match s {
            "Observed" => CapCertainty::Observed,
            "Likely" => CapCertainty::Likely,
            "Possible" => CapCertainty::Possible,
            "Unlikely" => CapCertainty::Unlikely,
            _ => CapCertainty::Unknown(s.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            CapCertainty::Observed => "Observed",
            CapCertainty::Likely => "Likely",
            CapCertainty::Possible => "Possible",
            CapCertainty::Unlikely => "Unlikely",
            CapCertainty::Unknown(s) => s.as_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// CAP → ORP entity type mapping
// ---------------------------------------------------------------------------

/// Determine ORP entity type from CAP category.
pub fn cap_category_to_entity(category: &CapCategory) -> &'static str {
    match category {
        CapCategory::Met => "weather_alert",
        CapCategory::Geo => "geological_alert",
        CapCategory::Fire => "fire_alert",
        CapCategory::Health => "health_alert",
        CapCategory::Security => "security_alert",
        CapCategory::Safety => "safety_alert",
        CapCategory::Rescue => "rescue_alert",
        CapCategory::Env => "environmental_alert",
        CapCategory::Transport => "transport_alert",
        CapCategory::Infra => "infrastructure_alert",
        CapCategory::Cbrne => "cbrne_alert",
        CapCategory::Other | CapCategory::Unknown(_) => "emergency_alert",
    }
}

// ---------------------------------------------------------------------------
// CAP XML parser
// ---------------------------------------------------------------------------

/// Parse a CAP 1.2 XML document.
pub fn parse_cap_xml(xml: &str) -> Result<CapAlert, ConnectorError> {
    let mut reader = Reader::from_str(xml);

    let mut identifier = String::new();
    let mut sender = String::new();
    let mut sent = String::new();
    let mut status = String::new();
    let mut msg_type = String::new();
    let mut scope = String::new();
    let mut source: Option<String> = None;
    let mut note: Option<String> = None;
    let mut infos: Vec<CapInfo> = Vec::new();

    // Current parse state
    let mut current_tag = String::new();
    let mut in_info = false;
    let mut in_area = false;

    // Current info being built
    let mut info_language: Option<String> = None;
    let mut info_category = String::new();
    let mut info_event = String::new();
    let mut info_urgency = String::new();
    let mut info_severity = String::new();
    let mut info_certainty = String::new();
    let mut info_headline: Option<String> = None;
    let mut info_description: Option<String> = None;
    let mut info_instruction: Option<String> = None;
    let mut info_effective: Option<String> = None;
    let mut info_expires: Option<String> = None;
    let mut info_sender_name: Option<String> = None;
    let mut info_areas: Vec<CapArea> = Vec::new();

    // Current area being built
    let mut area_desc = String::new();
    let mut area_polygons: Vec<String> = Vec::new();
    let mut area_circles: Vec<String> = Vec::new();
    let mut area_geocodes: Vec<(String, String)> = Vec::new();
    let mut geocode_name = String::new();

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                // Strip namespace prefix (e.g., "cap:alert" → "alert")
                let local = tag.rsplit(':').next().unwrap_or(&tag).to_string();
                current_tag = local.clone();
                match local.as_str() {
                    "info" => {
                        in_info = true;
                        info_language = None;
                        info_category.clear();
                        info_event.clear();
                        info_urgency.clear();
                        info_severity.clear();
                        info_certainty.clear();
                        info_headline = None;
                        info_description = None;
                        info_instruction = None;
                        info_effective = None;
                        info_expires = None;
                        info_sender_name = None;
                        info_areas = Vec::new();
                    }
                    "area" if in_info => {
                        in_area = true;
                        area_desc.clear();
                        area_polygons.clear();
                        area_circles.clear();
                        area_geocodes.clear();
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Text(ref t)) => {
                let text = t.unescape().unwrap_or_default().to_string();
                let text = text.trim().to_string();
                if text.is_empty() {
                    buf.clear();
                    continue;
                }

                if in_area {
                    match current_tag.as_str() {
                        "areaDesc" => area_desc = text,
                        "polygon" => area_polygons.push(text),
                        "circle" => area_circles.push(text),
                        "valueName" => geocode_name = text,
                        "value" => {
                            if !geocode_name.is_empty() {
                                area_geocodes
                                    .push((geocode_name.clone(), text));
                                geocode_name.clear();
                            }
                        }
                        _ => {}
                    }
                } else if in_info {
                    match current_tag.as_str() {
                        "language" => info_language = Some(text),
                        "category" => info_category = text,
                        "event" => info_event = text,
                        "urgency" => info_urgency = text,
                        "severity" => info_severity = text,
                        "certainty" => info_certainty = text,
                        "headline" => info_headline = Some(text),
                        "description" => info_description = Some(text),
                        "instruction" => info_instruction = Some(text),
                        "effective" => info_effective = Some(text),
                        "expires" => info_expires = Some(text),
                        "senderName" => info_sender_name = Some(text),
                        _ => {}
                    }
                } else {
                    match current_tag.as_str() {
                        "identifier" => identifier = text,
                        "sender" => sender = text,
                        "sent" => sent = text,
                        "status" => status = text,
                        "msgType" => msg_type = text,
                        "scope" => scope = text,
                        "source" => source = Some(text),
                        "note" => note = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(XmlEvent::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let local = tag.rsplit(':').next().unwrap_or(&tag).to_string();
                match local.as_str() {
                    "area" if in_area => {
                        in_area = false;
                        info_areas.push(CapArea {
                            area_desc: area_desc.clone(),
                            polygons: area_polygons.clone(),
                            circles: area_circles.clone(),
                            geocodes: area_geocodes.clone(),
                        });
                    }
                    "info" if in_info => {
                        in_info = false;
                        infos.push(CapInfo {
                            language: info_language.clone(),
                            category: CapCategory::parse(&info_category),
                            event: info_event.clone(),
                            urgency: CapUrgency::parse(&info_urgency),
                            severity: CapSeverity::parse(&info_severity),
                            certainty: CapCertainty::parse(&info_certainty),
                            headline: info_headline.clone(),
                            description: info_description.clone(),
                            instruction: info_instruction.clone(),
                            effective: info_effective.clone(),
                            expires: info_expires.clone(),
                            sender_name: info_sender_name.clone(),
                            areas: info_areas.clone(),
                        });
                    }
                    _ => {}
                }
                current_tag.clear();
            }
            Ok(XmlEvent::Eof) => break,
            Err(e) => {
                return Err(ConnectorError::ParseError(format!(
                    "CAP XML parse error: {}",
                    e
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    if identifier.is_empty() {
        return Err(ConnectorError::ParseError(
            "CAP alert missing identifier".to_string(),
        ));
    }

    Ok(CapAlert {
        identifier,
        sender,
        sent,
        status: CapStatus::parse(&status),
        msg_type: CapMsgType::parse(&msg_type),
        scope: CapScope::parse(&scope),
        source,
        note,
        infos,
    })
}

// ---------------------------------------------------------------------------
// Extract centroid from CAP area (for ORP position)
// ---------------------------------------------------------------------------

/// Parse a CAP circle string ("lat,lon radius") and return (lat, lon, radius_km).
pub fn parse_cap_circle(circle: &str) -> Option<(f64, f64, f64)> {
    let parts: Vec<&str> = circle.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }
    let coords: Vec<&str> = parts[0].split(',').collect();
    if coords.len() != 2 {
        return None;
    }
    let lat: f64 = coords[0].parse().ok()?;
    let lon: f64 = coords[1].parse().ok()?;
    let radius: f64 = parts[1].parse().ok()?;
    Some((lat, lon, radius))
}

/// Parse a CAP polygon string ("lat,lon lat,lon ...") and return centroid (lat, lon).
pub fn parse_cap_polygon_centroid(polygon: &str) -> Option<(f64, f64)> {
    let points: Vec<&str> = polygon.split_whitespace().collect();
    if points.is_empty() {
        return None;
    }
    let mut sum_lat = 0.0;
    let mut sum_lon = 0.0;
    let mut count = 0;
    for pt in &points {
        let coords: Vec<&str> = pt.split(',').collect();
        if coords.len() == 2 {
            if let (Ok(lat), Ok(lon)) =
                (coords[0].parse::<f64>(), coords[1].parse::<f64>())
            {
                sum_lat += lat;
                sum_lon += lon;
                count += 1;
            }
        }
    }
    if count > 0 {
        Some((sum_lat / count as f64, sum_lon / count as f64))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// CapAlert → SourceEvent
// ---------------------------------------------------------------------------

impl CapAlert {
    /// Convert to ORP SourceEvent(s) — one per info element, or one for the whole
    /// alert if no infos.
    pub fn to_source_events(&self, connector_id: &str) -> Vec<SourceEvent> {
        if self.infos.is_empty() {
            return vec![self.to_single_event(connector_id, None)];
        }
        self.infos
            .iter()
            .enumerate()
            .map(|(i, info)| self.to_single_event(connector_id, Some((i, info))))
            .collect()
    }

    fn to_single_event(
        &self,
        connector_id: &str,
        info: Option<(usize, &CapInfo)>,
    ) -> SourceEvent {
        let mut properties: HashMap<String, serde_json::Value> = HashMap::new();
        properties.insert("cap_id".into(), serde_json::json!(self.identifier));
        properties.insert("sender".into(), serde_json::json!(self.sender));
        properties.insert("sent".into(), serde_json::json!(self.sent));
        properties.insert(
            "status".into(),
            serde_json::json!(self.status.as_str()),
        );
        properties.insert(
            "msg_type".into(),
            serde_json::json!(self.msg_type.as_str()),
        );
        properties.insert(
            "scope".into(),
            serde_json::json!(self.scope.as_str()),
        );
        if let Some(ref src) = self.source {
            properties.insert("source".into(), serde_json::json!(src));
        }
        if let Some(ref n) = self.note {
            properties.insert("note".into(), serde_json::json!(n));
        }

        let mut lat: Option<f64> = None;
        let mut lon: Option<f64> = None;
        let entity_type;
        let entity_suffix;

        if let Some((_idx, inf)) = info {
            entity_type = cap_category_to_entity(&inf.category).to_string();
            entity_suffix = format!(":{}", _idx);

            properties.insert(
                "category".into(),
                serde_json::json!(inf.category.as_str()),
            );
            properties.insert("event".into(), serde_json::json!(inf.event));
            properties.insert(
                "urgency".into(),
                serde_json::json!(inf.urgency.as_str()),
            );
            properties.insert(
                "severity".into(),
                serde_json::json!(inf.severity.as_str()),
            );
            properties.insert(
                "certainty".into(),
                serde_json::json!(inf.certainty.as_str()),
            );
            if let Some(ref h) = inf.headline {
                properties.insert("headline".into(), serde_json::json!(h));
            }
            if let Some(ref d) = inf.description {
                properties.insert("description".into(), serde_json::json!(d));
            }
            if let Some(ref inst) = inf.instruction {
                properties
                    .insert("instruction".into(), serde_json::json!(inst));
            }
            if let Some(ref exp) = inf.expires {
                properties.insert("expires".into(), serde_json::json!(exp));
            }

            // Extract position from first area
            if let Some(area) = inf.areas.first() {
                if let Some(circle) = area.circles.first() {
                    if let Some((clat, clon, radius)) = parse_cap_circle(circle) {
                        lat = Some(clat);
                        lon = Some(clon);
                        properties.insert(
                            "radius_km".into(),
                            serde_json::json!(radius),
                        );
                    }
                } else if let Some(poly) = area.polygons.first() {
                    if let Some((plat, plon)) = parse_cap_polygon_centroid(poly) {
                        lat = Some(plat);
                        lon = Some(plon);
                    }
                }
                properties.insert(
                    "area_desc".into(),
                    serde_json::json!(area.area_desc),
                );
            }
        } else {
            entity_type = "emergency_alert".to_string();
            entity_suffix = String::new();
        }

        let ts = chrono::DateTime::parse_from_rfc3339(&self.sent)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id: format!("cap:{}{}", self.identifier, entity_suffix),
            entity_type,
            properties,
            timestamp: ts,
            latitude: lat,
            longitude: lon,
        }
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// CAP connector — polls ATOM/RSS feeds or HTTP endpoints for CAP XML alerts.
pub struct CapConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl CapConnector {
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
impl Connector for CapConnector {
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
            "CAP connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let connector_id = self.config.connector_id.clone();
        let url = self.config.url.clone();
        let props = self.config.properties.clone();

        tokio::spawn(async move {
            if let Some(ref feed_url) = url {
                let client = reqwest::Client::new();
                let poll_secs = props
                    .get("poll_interval_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(60);

                let mut interval = tokio::time::interval(
                    tokio::time::Duration::from_secs(poll_secs),
                );

                while running.load(Ordering::SeqCst) {
                    interval.tick().await;
                    match client.get(feed_url.as_str()).send().await {
                        Ok(resp) => match resp.text().await {
                            Ok(body) => match parse_cap_xml(&body) {
                                Ok(alert) => {
                                    for event in
                                        alert.to_source_events(&connector_id)
                                    {
                                        if tx.send(event).await.is_err() {
                                            return;
                                        }
                                        events_count
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("CAP parse error: {}", e);
                                    errors_count
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                            },
                            Err(e) => {
                                tracing::warn!("CAP response error: {}", e);
                                errors_count
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Err(e) => {
                            tracing::warn!("CAP request error: {}", e);
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
            "CAP connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "CAP connector not running".to_string(),
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

    fn sample_cap_xml() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<alert xmlns="urn:oasis:names:tc:emergency:cap:1.2">
  <identifier>NWS-IDP-TORNADO-WARNING-2026</identifier>
  <sender>w-nws.webmaster@noaa.gov</sender>
  <sent>2026-03-26T18:30:00-05:00</sent>
  <status>Actual</status>
  <msgType>Alert</msgType>
  <scope>Public</scope>
  <info>
    <category>Met</category>
    <event>Tornado Warning</event>
    <urgency>Immediate</urgency>
    <severity>Extreme</severity>
    <certainty>Observed</certainty>
    <headline>Tornado Warning issued for Johnson County</headline>
    <description>A tornado has been sighted near Overland Park moving northeast at 35 mph.</description>
    <instruction>Take shelter immediately in an interior room on the lowest floor of a sturdy building.</instruction>
    <expires>2026-03-26T19:30:00-05:00</expires>
    <senderName>NWS Kansas City</senderName>
    <area>
      <areaDesc>Johnson County, Kansas</areaDesc>
      <circle>38.9822,-94.6708 15.0</circle>
    </area>
  </info>
</alert>"#
    }

    #[test]
    fn test_parse_cap_basic() {
        let alert = parse_cap_xml(sample_cap_xml()).unwrap();
        assert_eq!(
            alert.identifier,
            "NWS-IDP-TORNADO-WARNING-2026"
        );
        assert_eq!(alert.sender, "w-nws.webmaster@noaa.gov");
        assert_eq!(alert.status, CapStatus::Actual);
        assert_eq!(alert.msg_type, CapMsgType::Alert);
        assert_eq!(alert.scope, CapScope::Public);
    }

    #[test]
    fn test_parse_cap_info() {
        let alert = parse_cap_xml(sample_cap_xml()).unwrap();
        assert_eq!(alert.infos.len(), 1);
        let info = &alert.infos[0];
        assert_eq!(info.category, CapCategory::Met);
        assert_eq!(info.event, "Tornado Warning");
        assert_eq!(info.urgency, CapUrgency::Immediate);
        assert_eq!(info.severity, CapSeverity::Extreme);
        assert_eq!(info.certainty, CapCertainty::Observed);
    }

    #[test]
    fn test_parse_cap_headline() {
        let alert = parse_cap_xml(sample_cap_xml()).unwrap();
        assert_eq!(
            alert.infos[0].headline,
            Some("Tornado Warning issued for Johnson County".into())
        );
    }

    #[test]
    fn test_parse_cap_area() {
        let alert = parse_cap_xml(sample_cap_xml()).unwrap();
        let areas = &alert.infos[0].areas;
        assert_eq!(areas.len(), 1);
        assert_eq!(areas[0].area_desc, "Johnson County, Kansas");
        assert_eq!(areas[0].circles.len(), 1);
        assert!(areas[0].circles[0].contains("38.9822"));
    }

    #[test]
    fn test_parse_cap_circle() {
        let (lat, lon, radius) =
            parse_cap_circle("38.9822,-94.6708 15.0").unwrap();
        assert!((lat - 38.9822).abs() < 0.001);
        assert!((lon - (-94.6708)).abs() < 0.001);
        assert!((radius - 15.0).abs() < 0.1);
    }

    #[test]
    fn test_parse_cap_circle_invalid() {
        assert!(parse_cap_circle("invalid").is_none());
        assert!(parse_cap_circle("").is_none());
    }

    #[test]
    fn test_parse_cap_polygon_centroid() {
        let polygon = "38.0,-95.0 39.0,-95.0 39.0,-94.0 38.0,-94.0";
        let (lat, lon) = parse_cap_polygon_centroid(polygon).unwrap();
        assert!((lat - 38.5).abs() < 0.01);
        assert!((lon - (-94.5)).abs() < 0.01);
    }

    #[test]
    fn test_cap_to_source_events() {
        let alert = parse_cap_xml(sample_cap_xml()).unwrap();
        let events = alert.to_source_events("cap-test");
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.entity_type, "weather_alert");
        assert_eq!(
            event.entity_id,
            "cap:NWS-IDP-TORNADO-WARNING-2026:0"
        );
        assert!(event.latitude.is_some());
        assert!(event.longitude.is_some());
        assert_eq!(
            event.properties.get("severity").unwrap(),
            &serde_json::json!("Extreme")
        );
    }

    #[test]
    fn test_cap_category_to_entity() {
        assert_eq!(
            cap_category_to_entity(&CapCategory::Met),
            "weather_alert"
        );
        assert_eq!(
            cap_category_to_entity(&CapCategory::Geo),
            "geological_alert"
        );
        assert_eq!(
            cap_category_to_entity(&CapCategory::Fire),
            "fire_alert"
        );
        assert_eq!(
            cap_category_to_entity(&CapCategory::Security),
            "security_alert"
        );
        assert_eq!(
            cap_category_to_entity(&CapCategory::Other),
            "emergency_alert"
        );
    }

    #[test]
    fn test_cap_status_roundtrip() {
        assert_eq!(CapStatus::parse("Actual"), CapStatus::Actual);
        assert_eq!(CapStatus::parse("Test"), CapStatus::Test);
        assert_eq!(CapStatus::Actual.as_str(), "Actual");
    }

    #[test]
    fn test_cap_severity_values() {
        assert_eq!(CapSeverity::parse("Extreme"), CapSeverity::Extreme);
        assert_eq!(CapSeverity::parse("Minor"), CapSeverity::Minor);
        assert_eq!(
            CapSeverity::parse("Custom"),
            CapSeverity::Unknown("Custom".into())
        );
    }

    #[test]
    fn test_cap_missing_identifier() {
        let xml = r#"<alert><sender>test</sender></alert>"#;
        assert!(parse_cap_xml(xml).is_err());
    }

    #[test]
    fn test_cap_no_info() {
        let xml = r#"<alert>
  <identifier>TEST-001</identifier>
  <sender>test@test.gov</sender>
  <sent>2026-03-26T00:00:00Z</sent>
  <status>Test</status>
  <msgType>Alert</msgType>
  <scope>Public</scope>
</alert>"#;
        let alert = parse_cap_xml(xml).unwrap();
        assert_eq!(alert.infos.len(), 0);
        let events = alert.to_source_events("cap-test");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].entity_type, "emergency_alert");
    }

    #[test]
    fn test_cap_multiple_infos() {
        let xml = r#"<alert>
  <identifier>MULTI-INFO</identifier>
  <sender>test@test.gov</sender>
  <sent>2026-03-26T00:00:00Z</sent>
  <status>Actual</status>
  <msgType>Alert</msgType>
  <scope>Public</scope>
  <info>
    <category>Met</category>
    <event>Flood Watch</event>
    <urgency>Expected</urgency>
    <severity>Moderate</severity>
    <certainty>Likely</certainty>
  </info>
  <info>
    <category>Geo</category>
    <event>Earthquake Warning</event>
    <urgency>Immediate</urgency>
    <severity>Severe</severity>
    <certainty>Observed</certainty>
  </info>
</alert>"#;
        let alert = parse_cap_xml(xml).unwrap();
        assert_eq!(alert.infos.len(), 2);
        let events = alert.to_source_events("cap-test");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_type, "weather_alert");
        assert_eq!(events[1].entity_type, "geological_alert");
    }

    #[test]
    fn test_cap_connector_id() {
        let config = ConnectorConfig {
            connector_id: "cap-1".to_string(),
            connector_type: "cap".to_string(),
            url: None,
            entity_type: "alert".to_string(),
            enabled: true,
            trust_score: 0.95,
            properties: HashMap::new(),
        };
        let connector = CapConnector::new(config);
        assert_eq!(connector.connector_id(), "cap-1");
    }

    #[tokio::test]
    async fn test_cap_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "cap-health".to_string(),
            connector_type: "cap".to_string(),
            url: None,
            entity_type: "alert".to_string(),
            enabled: true,
            trust_score: 0.95,
            properties: HashMap::new(),
        };
        let connector = CapConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_parse_cap_with_polygon_area() {
        let xml = r#"<alert>
  <identifier>POLY-001</identifier>
  <sender>test@test.gov</sender>
  <sent>2026-03-26T00:00:00Z</sent>
  <status>Actual</status>
  <msgType>Alert</msgType>
  <scope>Public</scope>
  <info>
    <category>Fire</category>
    <event>Wildfire</event>
    <urgency>Immediate</urgency>
    <severity>Severe</severity>
    <certainty>Observed</certainty>
    <area>
      <areaDesc>Hillside Fire Zone</areaDesc>
      <polygon>34.0,-118.0 34.1,-118.0 34.1,-117.9 34.0,-117.9</polygon>
    </area>
  </info>
</alert>"#;
        let alert = parse_cap_xml(xml).unwrap();
        let events = alert.to_source_events("cap-test");
        assert_eq!(events[0].entity_type, "fire_alert");
        assert!(events[0].latitude.is_some());
        let lat = events[0].latitude.unwrap();
        assert!((lat - 34.05).abs() < 0.01);
    }

    #[test]
    fn test_parse_invalid_cap_xml() {
        assert!(parse_cap_xml("<<<not xml>>>").is_err());
    }
}
