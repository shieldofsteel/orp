use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// NFFI (NATO Friendly Force Information) parser
// ---------------------------------------------------------------------------
// NFFI (STANAG 5527) is a NATO XML format for friendly force tracking.
// It provides situational awareness of own and allied forces.
//
// XML structure:
//   <nffi> or <NFFFeedMessage>
//     <track> (one or more)
//       <trackId>unique-id</trackId>
//       <name>Unit Name</name>
//       <position>
//         <latitude>51.5074</latitude>
//         <longitude>-0.1278</longitude>
//       </position>
//       <altitude>100.0</altitude>
//       <speed>5.0</speed>
//       <course>270.0</course>
//       <identity>FRIENDLY</identity>
//       <platformType>GROUND_VEHICLE</platformType>
//       <nationality>GBR</nationality>
//       <symbology>SFGPUCI----</symbology>
//       <operationalStatus>OPERATIONAL</operationalStatus>
//       <timestamp>2026-03-26T12:00:00Z</timestamp>
//       <remarks>Patrol unit on route</remarks>
//     </track>
//   </nffi>

/// Force affiliation / identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NffiAffiliation {
    Friendly,
    Hostile,
    Neutral,
    Unknown,
    AssumedFriendly,
    Suspect,
    Pending,
}

impl NffiAffiliation {
    pub fn as_str(&self) -> &'static str {
        match self {
            NffiAffiliation::Friendly => "FRIENDLY",
            NffiAffiliation::Hostile => "HOSTILE",
            NffiAffiliation::Neutral => "NEUTRAL",
            NffiAffiliation::Unknown => "UNKNOWN",
            NffiAffiliation::AssumedFriendly => "ASSUMED_FRIENDLY",
            NffiAffiliation::Suspect => "SUSPECT",
            NffiAffiliation::Pending => "PENDING",
        }
    }
}

/// Parse affiliation string.
pub fn parse_affiliation(s: &str) -> NffiAffiliation {
    match s.trim().to_uppercase().as_str() {
        "FRIENDLY" | "FRIEND" | "F" => NffiAffiliation::Friendly,
        "HOSTILE" | "ENEMY" | "H" => NffiAffiliation::Hostile,
        "NEUTRAL" | "N" => NffiAffiliation::Neutral,
        "ASSUMED_FRIENDLY" | "ASSUMED FRIENDLY" | "ASSUMEDFRIENDLY" | "A" => {
            NffiAffiliation::AssumedFriendly
        }
        "SUSPECT" | "S" => NffiAffiliation::Suspect,
        "PENDING" | "P" => NffiAffiliation::Pending,
        _ => NffiAffiliation::Unknown,
    }
}

/// Platform type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NffiPlatformType {
    GroundVehicle,
    Aircraft,
    Ship,
    Dismounted,
    Helicopter,
    Uav,
    Artillery,
    CommandPost,
    Unknown(String),
}

impl NffiPlatformType {
    pub fn as_str(&self) -> &str {
        match self {
            NffiPlatformType::GroundVehicle => "GROUND_VEHICLE",
            NffiPlatformType::Aircraft => "AIRCRAFT",
            NffiPlatformType::Ship => "SHIP",
            NffiPlatformType::Dismounted => "DISMOUNTED",
            NffiPlatformType::Helicopter => "HELICOPTER",
            NffiPlatformType::Uav => "UAV",
            NffiPlatformType::Artillery => "ARTILLERY",
            NffiPlatformType::CommandPost => "COMMAND_POST",
            NffiPlatformType::Unknown(s) => s.as_str(),
        }
    }
}

/// Parse platform type string.
pub fn parse_platform_type(s: &str) -> NffiPlatformType {
    match s.trim().to_uppercase().as_str() {
        "GROUND_VEHICLE" | "GROUNDVEHICLE" | "GROUND VEHICLE" | "VEHICLE" => {
            NffiPlatformType::GroundVehicle
        }
        "AIRCRAFT" | "FIXED_WING" | "FIXED WING" => NffiPlatformType::Aircraft,
        "SHIP" | "VESSEL" | "SURFACE" => NffiPlatformType::Ship,
        "DISMOUNTED" | "INFANTRY" | "PERSON" | "SOLDIER" => NffiPlatformType::Dismounted,
        "HELICOPTER" | "ROTARY_WING" | "ROTARY WING" | "HELO" => NffiPlatformType::Helicopter,
        "UAV" | "UAS" | "DRONE" | "RPAS" => NffiPlatformType::Uav,
        "ARTILLERY" | "FIRES" => NffiPlatformType::Artillery,
        "COMMAND_POST" | "COMMANDPOST" | "COMMAND POST" | "CP" | "HQ" => {
            NffiPlatformType::CommandPost
        }
        _ => NffiPlatformType::Unknown(s.trim().to_string()),
    }
}

/// Operational status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NffiOperationalStatus {
    Operational,
    Damaged,
    Destroyed,
    Unknown,
}

impl NffiOperationalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            NffiOperationalStatus::Operational => "OPERATIONAL",
            NffiOperationalStatus::Damaged => "DAMAGED",
            NffiOperationalStatus::Destroyed => "DESTROYED",
            NffiOperationalStatus::Unknown => "UNKNOWN",
        }
    }
}

/// Parse operational status.
pub fn parse_operational_status(s: &str) -> NffiOperationalStatus {
    match s.trim().to_uppercase().as_str() {
        "OPERATIONAL" | "OP" | "ACTIVE" => NffiOperationalStatus::Operational,
        "DAMAGED" | "DMG" => NffiOperationalStatus::Damaged,
        "DESTROYED" | "DES" | "DEAD" => NffiOperationalStatus::Destroyed,
        _ => NffiOperationalStatus::Unknown,
    }
}

/// Parsed NFFI track.
#[derive(Clone, Debug)]
pub struct NffiTrack {
    pub track_id: String,
    pub name: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: Option<f64>,
    pub speed: Option<f64>,
    pub course: Option<f64>,
    pub affiliation: NffiAffiliation,
    pub platform_type: NffiPlatformType,
    pub nationality: Option<String>,
    pub symbology: Option<String>,
    pub operational_status: NffiOperationalStatus,
    pub timestamp: Option<String>,
    pub remarks: Option<String>,
}

// ---------------------------------------------------------------------------
// XML Parser
// ---------------------------------------------------------------------------

/// Parse NFFI XML string into a list of tracks.
pub fn parse_nffi_xml(xml_data: &str) -> Result<Vec<NffiTrack>, ConnectorError> {
    let mut reader = Reader::from_str(xml_data);

    let mut tracks = Vec::new();
    let mut buf = Vec::new();

    // State machine
    let mut in_track = false;
    let mut in_position = false;
    let mut current_tag = String::new();

    // Current track fields
    let mut track_id = String::new();
    let mut name: Option<String> = None;
    let mut latitude: Option<f64> = None;
    let mut longitude: Option<f64> = None;
    let mut altitude: Option<f64> = None;
    let mut speed: Option<f64> = None;
    let mut course: Option<f64> = None;
    let mut affiliation = NffiAffiliation::Unknown;
    let mut platform_type = NffiPlatformType::Unknown("UNKNOWN".into());
    let mut nationality: Option<String> = None;
    let mut symbology: Option<String> = None;
    let mut operational_status = NffiOperationalStatus::Unknown;
    let mut timestamp: Option<String> = None;
    let mut remarks: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                match tag_name.as_str() {
                    "track" | "trackmessage" | "trackentry" => {
                        in_track = true;
                        // Reset fields
                        track_id = String::new();
                        name = None;
                        latitude = None;
                        longitude = None;
                        altitude = None;
                        speed = None;
                        course = None;
                        affiliation = NffiAffiliation::Unknown;
                        platform_type = NffiPlatformType::Unknown("UNKNOWN".into());
                        nationality = None;
                        symbology = None;
                        operational_status = NffiOperationalStatus::Unknown;
                        timestamp = None;
                        remarks = None;

                        // Check for id attribute
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.as_ref()).to_lowercase();
                            if key == "id" || key == "trackid" {
                                track_id =
                                    String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    "position" | "pos" | "location" => {
                        if in_track {
                            in_position = true;
                        }
                    }
                    _ => {}
                }
                if in_track {
                    current_tag = tag_name;
                }
            }
            Ok(Event::End(e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_lowercase();
                match tag_name.as_str() {
                    "track" | "trackmessage" | "trackentry" => {
                        if in_track {
                            let lat = latitude.unwrap_or(0.0);
                            let lon = longitude.unwrap_or(0.0);

                            if track_id.is_empty() {
                                track_id = format!("track-{}", tracks.len());
                            }

                            tracks.push(NffiTrack {
                                track_id: track_id.clone(),
                                name: name.clone(),
                                latitude: lat,
                                longitude: lon,
                                altitude,
                                speed,
                                course,
                                affiliation: affiliation.clone(),
                                platform_type: platform_type.clone(),
                                nationality: nationality.clone(),
                                symbology: symbology.clone(),
                                operational_status: operational_status.clone(),
                                timestamp: timestamp.clone(),
                                remarks: remarks.clone(),
                            });
                            in_track = false;
                        }
                    }
                    "position" | "pos" | "location" => {
                        in_position = false;
                    }
                    _ => {}
                }
                current_tag.clear();
            }
            Ok(Event::Text(e)) => {
                if in_track {
                    let text = e.unescape().unwrap_or_default().trim().to_string();
                    if text.is_empty() {
                        buf.clear();
                        continue;
                    }

                    match current_tag.as_str() {
                        "trackid" | "id" => {
                            if track_id.is_empty() {
                                track_id = text;
                            }
                        }
                        "name" | "callsign" | "designation" => {
                            name = Some(text);
                        }
                        "latitude" | "lat" => {
                            latitude = text.parse::<f64>().ok();
                        }
                        "longitude" | "lon" | "lng" => {
                            longitude = text.parse::<f64>().ok();
                        }
                        "altitude" | "alt" | "elevation" => {
                            altitude = text.parse::<f64>().ok();
                        }
                        "speed" | "velocity" => {
                            speed = text.parse::<f64>().ok();
                        }
                        "course" | "heading" | "bearing" => {
                            course = text.parse::<f64>().ok();
                        }
                        "identity" | "affiliation" | "hostility" => {
                            affiliation = parse_affiliation(&text);
                        }
                        "platformtype" | "platform" | "type" | "category" => {
                            if !in_position {
                                platform_type = parse_platform_type(&text);
                            }
                        }
                        "nationality" | "country" | "nation" => {
                            nationality = Some(text);
                        }
                        "symbology" | "sidc" | "symbol" => {
                            symbology = Some(text);
                        }
                        "operationalstatus" | "status" | "condition" => {
                            operational_status = parse_operational_status(&text);
                        }
                        "timestamp" | "time" | "datetime" => {
                            timestamp = Some(text);
                        }
                        "remarks" | "comment" | "notes" => {
                            remarks = Some(text);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(ConnectorError::ParseError(format!(
                    "NFFI: XML parse error: {}",
                    e
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(tracks)
}

// ---------------------------------------------------------------------------
// NFFI → SourceEvent
// ---------------------------------------------------------------------------

/// Convert an NFFI track to a SourceEvent.
pub fn nffi_track_to_source_event(
    track: &NffiTrack,
    connector_id: &str,
) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert("track_id".into(), json!(track.track_id));
    properties.insert("affiliation".into(), json!(track.affiliation.as_str()));
    properties.insert("platform_type".into(), json!(track.platform_type.as_str()));
    properties.insert(
        "operational_status".into(),
        json!(track.operational_status.as_str()),
    );

    if let Some(ref n) = track.name {
        properties.insert("name".into(), json!(n));
    }
    if let Some(alt) = track.altitude {
        properties.insert("altitude_m".into(), json!(alt));
    }
    if let Some(spd) = track.speed {
        properties.insert("speed_mps".into(), json!(spd));
    }
    if let Some(crs) = track.course {
        properties.insert("course_deg".into(), json!(crs));
    }
    if let Some(ref nat) = track.nationality {
        properties.insert("nationality".into(), json!(nat));
    }
    if let Some(ref sym) = track.symbology {
        properties.insert("symbology".into(), json!(sym));
    }
    if let Some(ref ts) = track.timestamp {
        properties.insert("source_timestamp".into(), json!(ts));
    }
    if let Some(ref rem) = track.remarks {
        properties.insert("remarks".into(), json!(rem));
    }

    let entity_id = format!("nffi:{}", track.track_id);

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "force_element".to_string(),
        properties,
        timestamp: Utc::now(),
        latitude: Some(track.latitude),
        longitude: Some(track.longitude),
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct NffiConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl NffiConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_processed: Arc::new(AtomicU64::new(0)),
            errors: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl Connector for NffiConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        let path = self
            .config
            .url
            .as_deref()
            .ok_or_else(|| {
                ConnectorError::ConfigError("NFFI: url (XML file path) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        match parse_nffi_xml(&content) {
            Ok(tracks) => {
                for track in tracks {
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }
                    let event = nffi_track_to_source_event(&track, &connector_id);
                    if tx.send(event).await.is_err() {
                        break;
                    }
                    events_processed.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(ConnectorError::ConnectionError(
                "NFFI connector is not running".into(),
            ));
        }
        Ok(())
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        ConnectorStats {
            events_processed: self.events_processed.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            last_event_timestamp: None,
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

    #[test]
    fn test_parse_nffi_single_track() {
        let xml = r#"
        <nffi>
            <track>
                <trackId>ALPHA-01</trackId>
                <name>Alpha Team</name>
                <position>
                    <latitude>51.5074</latitude>
                    <longitude>-0.1278</longitude>
                </position>
                <altitude>100.0</altitude>
                <speed>5.0</speed>
                <course>270.0</course>
                <identity>FRIENDLY</identity>
                <platformType>GROUND_VEHICLE</platformType>
                <nationality>GBR</nationality>
                <operationalStatus>OPERATIONAL</operationalStatus>
            </track>
        </nffi>
        "#;

        let tracks = parse_nffi_xml(xml).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].track_id, "ALPHA-01");
        assert_eq!(tracks[0].name, Some("Alpha Team".to_string()));
        assert!((tracks[0].latitude - 51.5074).abs() < 0.0001);
        assert!((tracks[0].longitude - (-0.1278)).abs() < 0.0001);
        assert_eq!(tracks[0].altitude, Some(100.0));
        assert_eq!(tracks[0].speed, Some(5.0));
        assert_eq!(tracks[0].course, Some(270.0));
        assert_eq!(tracks[0].affiliation, NffiAffiliation::Friendly);
        assert_eq!(tracks[0].platform_type, NffiPlatformType::GroundVehicle);
        assert_eq!(tracks[0].nationality, Some("GBR".to_string()));
        assert_eq!(tracks[0].operational_status, NffiOperationalStatus::Operational);
    }

    #[test]
    fn test_parse_nffi_multiple_tracks() {
        let xml = r#"
        <nffi>
            <track>
                <trackId>T1</trackId>
                <position>
                    <latitude>50.0</latitude>
                    <longitude>0.0</longitude>
                </position>
                <identity>FRIENDLY</identity>
                <platformType>AIRCRAFT</platformType>
            </track>
            <track>
                <trackId>T2</trackId>
                <position>
                    <latitude>51.0</latitude>
                    <longitude>1.0</longitude>
                </position>
                <identity>HOSTILE</identity>
                <platformType>SHIP</platformType>
            </track>
            <track>
                <trackId>T3</trackId>
                <position>
                    <latitude>52.0</latitude>
                    <longitude>2.0</longitude>
                </position>
                <identity>NEUTRAL</identity>
                <platformType>UAV</platformType>
            </track>
        </nffi>
        "#;

        let tracks = parse_nffi_xml(xml).unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].track_id, "T1");
        assert_eq!(tracks[1].track_id, "T2");
        assert_eq!(tracks[2].track_id, "T3");
        assert_eq!(tracks[0].affiliation, NffiAffiliation::Friendly);
        assert_eq!(tracks[1].affiliation, NffiAffiliation::Hostile);
        assert_eq!(tracks[2].affiliation, NffiAffiliation::Neutral);
    }

    #[test]
    fn test_parse_affiliation_friendly() {
        assert_eq!(parse_affiliation("FRIENDLY"), NffiAffiliation::Friendly);
        assert_eq!(parse_affiliation("friendly"), NffiAffiliation::Friendly);
        assert_eq!(parse_affiliation("FRIEND"), NffiAffiliation::Friendly);
        assert_eq!(parse_affiliation("F"), NffiAffiliation::Friendly);
    }

    #[test]
    fn test_parse_affiliation_hostile() {
        assert_eq!(parse_affiliation("HOSTILE"), NffiAffiliation::Hostile);
        assert_eq!(parse_affiliation("ENEMY"), NffiAffiliation::Hostile);
        assert_eq!(parse_affiliation("H"), NffiAffiliation::Hostile);
    }

    #[test]
    fn test_parse_affiliation_variants() {
        assert_eq!(parse_affiliation("NEUTRAL"), NffiAffiliation::Neutral);
        assert_eq!(parse_affiliation("UNKNOWN"), NffiAffiliation::Unknown);
        assert_eq!(
            parse_affiliation("ASSUMED_FRIENDLY"),
            NffiAffiliation::AssumedFriendly
        );
        assert_eq!(parse_affiliation("SUSPECT"), NffiAffiliation::Suspect);
        assert_eq!(parse_affiliation("PENDING"), NffiAffiliation::Pending);
        assert_eq!(parse_affiliation("garbage"), NffiAffiliation::Unknown);
    }

    #[test]
    fn test_parse_platform_types() {
        assert_eq!(
            parse_platform_type("GROUND_VEHICLE"),
            NffiPlatformType::GroundVehicle
        );
        assert_eq!(parse_platform_type("AIRCRAFT"), NffiPlatformType::Aircraft);
        assert_eq!(parse_platform_type("SHIP"), NffiPlatformType::Ship);
        assert_eq!(
            parse_platform_type("DISMOUNTED"),
            NffiPlatformType::Dismounted
        );
        assert_eq!(
            parse_platform_type("HELICOPTER"),
            NffiPlatformType::Helicopter
        );
        assert_eq!(parse_platform_type("UAV"), NffiPlatformType::Uav);
        assert_eq!(parse_platform_type("DRONE"), NffiPlatformType::Uav);
        assert_eq!(
            parse_platform_type("ARTILLERY"),
            NffiPlatformType::Artillery
        );
        assert_eq!(
            parse_platform_type("COMMAND_POST"),
            NffiPlatformType::CommandPost
        );
    }

    #[test]
    fn test_parse_operational_status() {
        assert_eq!(
            parse_operational_status("OPERATIONAL"),
            NffiOperationalStatus::Operational
        );
        assert_eq!(
            parse_operational_status("DAMAGED"),
            NffiOperationalStatus::Damaged
        );
        assert_eq!(
            parse_operational_status("DESTROYED"),
            NffiOperationalStatus::Destroyed
        );
        assert_eq!(
            parse_operational_status("other"),
            NffiOperationalStatus::Unknown
        );
    }

    #[test]
    fn test_nffi_track_to_source_event() {
        let track = NffiTrack {
            track_id: "BRAVO-07".to_string(),
            name: Some("Bravo Team".to_string()),
            latitude: 48.8566,
            longitude: 2.3522,
            altitude: Some(50.0),
            speed: Some(10.0),
            course: Some(180.0),
            affiliation: NffiAffiliation::Friendly,
            platform_type: NffiPlatformType::GroundVehicle,
            nationality: Some("FRA".to_string()),
            symbology: Some("SFGPUCI----".to_string()),
            operational_status: NffiOperationalStatus::Operational,
            timestamp: Some("2026-03-26T12:00:00Z".to_string()),
            remarks: Some("On patrol".to_string()),
        };

        let event = nffi_track_to_source_event(&track, "nffi-test");
        assert_eq!(event.entity_type, "force_element");
        assert_eq!(event.entity_id, "nffi:BRAVO-07");
        assert_eq!(event.latitude, Some(48.8566));
        assert_eq!(event.longitude, Some(2.3522));
        assert_eq!(event.properties["affiliation"], json!("FRIENDLY"));
        assert_eq!(event.properties["platform_type"], json!("GROUND_VEHICLE"));
        assert_eq!(event.properties["nationality"], json!("FRA"));
        assert_eq!(event.properties["symbology"], json!("SFGPUCI----"));
    }

    #[test]
    fn test_nffi_entity_type() {
        let track = NffiTrack {
            track_id: "X1".to_string(),
            name: None,
            latitude: 0.0,
            longitude: 0.0,
            altitude: None,
            speed: None,
            course: None,
            affiliation: NffiAffiliation::Unknown,
            platform_type: NffiPlatformType::Unknown("TEST".into()),
            nationality: None,
            symbology: None,
            operational_status: NffiOperationalStatus::Unknown,
            timestamp: None,
            remarks: None,
        };
        let event = nffi_track_to_source_event(&track, "test");
        assert_eq!(event.entity_type, "force_element");
    }

    #[test]
    fn test_parse_nffi_with_optional_fields() {
        let xml = r#"
        <nffi>
            <track>
                <trackId>MINIMAL</trackId>
                <position>
                    <latitude>40.0</latitude>
                    <longitude>-74.0</longitude>
                </position>
                <identity>UNKNOWN</identity>
                <platformType>UNKNOWN</platformType>
            </track>
        </nffi>
        "#;

        let tracks = parse_nffi_xml(xml).unwrap();
        assert_eq!(tracks.len(), 1);
        assert!(tracks[0].name.is_none());
        assert!(tracks[0].altitude.is_none());
        assert!(tracks[0].speed.is_none());
        assert!(tracks[0].nationality.is_none());
    }

    #[test]
    fn test_parse_nffi_empty_document() {
        let xml = "<nffi></nffi>";
        let tracks = parse_nffi_xml(xml).unwrap();
        assert!(tracks.is_empty());
    }

    #[test]
    fn test_nffi_connector_id() {
        let config = ConnectorConfig {
            connector_id: "nffi-1".to_string(),
            connector_type: "nffi".to_string(),
            url: None,
            entity_type: "force_element".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = NffiConnector::new(config);
        assert_eq!(connector.connector_id(), "nffi-1");
    }

    #[tokio::test]
    async fn test_nffi_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "nffi-h".to_string(),
            connector_type: "nffi".to_string(),
            url: None,
            entity_type: "force_element".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = NffiConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_parse_nffi_track_with_id_attribute() {
        let xml = r#"
        <nffi>
            <track id="ATTR-ID">
                <position>
                    <latitude>55.0</latitude>
                    <longitude>10.0</longitude>
                </position>
                <identity>NEUTRAL</identity>
                <platformType>SHIP</platformType>
            </track>
        </nffi>
        "#;
        let tracks = parse_nffi_xml(xml).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].track_id, "ATTR-ID");
    }
}
