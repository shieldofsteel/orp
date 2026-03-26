use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// METAR / TAF aviation weather parser
// ---------------------------------------------------------------------------
// METAR (Meteorological Aerodrome Report) is the standard format for
// reporting aviation weather observations.
//
// Format example:
//   METAR KJFK 260830Z 22006KT 10SM FEW250 18/10 A3012 RMK AO2
//
// Fields (in order):
//   [METAR|SPECI]     — report type (SPECI = special/unscheduled)
//   CCCC              — 4-letter ICAO station identifier
//   DDHHmmZ           — day of month, hour, minute (UTC)
//   dddffGggKT        — wind direction (degrees), speed, gust, unit
//   VVVV[SM]          — visibility (statute miles or meters)
//   clouds            — FEW/SCT/BKN/OVC + altitude (hundreds of feet)
//   TT/DD             — temperature / dewpoint (°C)
//   ANNNN / QNNNN     — altimeter (inHg) or QNH (hPa)
//   RMK ...           — remarks section
//
// TAF (Terminal Aerodrome Forecast) has similar structure but covers future
// periods with BECMG, TEMPO, FM groups.

/// Cloud cover level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloudCover {
    Few,       // 1–2 oktas
    Scattered, // 3–4 oktas
    Broken,    // 5–7 oktas
    Overcast,  // 8 oktas
    VerticalVisibility, // VV (sky obscured)
}

impl CloudCover {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "FEW" => Some(CloudCover::Few),
            "SCT" => Some(CloudCover::Scattered),
            "BKN" => Some(CloudCover::Broken),
            "OVC" => Some(CloudCover::Overcast),
            "VV" => Some(CloudCover::VerticalVisibility),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CloudCover::Few => "FEW",
            CloudCover::Scattered => "SCT",
            CloudCover::Broken => "BKN",
            CloudCover::Overcast => "OVC",
            CloudCover::VerticalVisibility => "VV",
        }
    }
}

/// Parsed cloud layer.
#[derive(Clone, Debug)]
pub struct CloudLayer {
    pub cover: CloudCover,
    pub altitude_ft: u32, // in hundreds of feet AGL
    pub cloud_type: Option<String>, // CB, TCU, etc.
}

/// Parsed wind information.
#[derive(Clone, Debug)]
pub struct Wind {
    pub direction_deg: Option<u16>, // None = VRB (variable)
    pub speed_kt: u16,
    pub gust_kt: Option<u16>,
    pub variable_from: Option<u16>,
    pub variable_to: Option<u16>,
}

/// Parsed METAR report.
#[derive(Clone, Debug)]
pub struct MetarReport {
    pub raw: String,
    pub report_type: String, // "METAR" or "SPECI"
    pub station: String,     // ICAO 4-letter code
    pub observation_time: Option<DateTime<Utc>>,
    pub day: Option<u8>,
    pub hour: Option<u8>,
    pub minute: Option<u8>,
    pub wind: Option<Wind>,
    pub visibility_sm: Option<f64>,   // statute miles
    pub visibility_m: Option<u32>,    // meters
    pub clouds: Vec<CloudLayer>,
    pub temperature_c: Option<i16>,
    pub dewpoint_c: Option<i16>,
    pub altimeter_inhg: Option<f64>,
    pub altimeter_hpa: Option<f64>,
    pub weather: Vec<String>,       // present weather codes (RA, SN, FG, etc.)
    pub sky_clear: bool,
    pub cavok: bool,
    pub remarks: Option<String>,
}

/// Parsed TAF forecast.
#[derive(Clone, Debug)]
pub struct TafReport {
    pub raw: String,
    pub station: String,
    pub issue_time: Option<DateTime<Utc>>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub groups: Vec<TafGroup>,
}

/// A TAF forecast group (FM, BECMG, TEMPO, or base).
#[derive(Clone, Debug)]
pub struct TafGroup {
    pub group_type: String, // "BASE", "FM", "BECMG", "TEMPO"
    pub wind: Option<Wind>,
    pub visibility_sm: Option<f64>,
    pub clouds: Vec<CloudLayer>,
    pub weather: Vec<String>,
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse a METAR string.
pub fn parse_metar(raw: &str) -> Result<MetarReport, ConnectorError> {
    let raw = raw.trim();
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    if tokens.is_empty() {
        return Err(ConnectorError::ParseError("METAR: empty string".into()));
    }

    let mut idx = 0;

    // Report type
    let report_type = if tokens[0] == "METAR" || tokens[0] == "SPECI" {
        idx += 1;
        tokens[0].to_string()
    } else {
        "METAR".to_string()
    };

    // Station (4-letter ICAO)
    if idx >= tokens.len() {
        return Err(ConnectorError::ParseError("METAR: missing station".into()));
    }
    let station = tokens[idx].to_string();
    idx += 1;

    // Time DDHHmmZ
    let (day, hour, minute, obs_time) = if idx < tokens.len() && tokens[idx].ends_with('Z') {
        let t = tokens[idx];
        idx += 1;
        let d = t.get(0..2).and_then(|s| s.parse::<u8>().ok());
        let h = t.get(2..4).and_then(|s| s.parse::<u8>().ok());
        let m = t.get(4..6).and_then(|s| s.parse::<u8>().ok());
        let obs = build_obs_time(d, h, m);
        (d, h, m, obs)
    } else {
        (None, None, None, None)
    };

    // AUTO or COR
    if idx < tokens.len() && (tokens[idx] == "AUTO" || tokens[idx] == "COR") {
        idx += 1;
    }

    let mut wind = None;
    let mut visibility_sm = None;
    let mut visibility_m = None;
    let mut clouds = Vec::new();
    let mut temperature_c = None;
    let mut dewpoint_c = None;
    let mut altimeter_inhg = None;
    let mut altimeter_hpa = None;
    let mut weather = Vec::new();
    let mut sky_clear = false;
    let mut cavok = false;
    let mut remarks = None;

    while idx < tokens.len() {
        let tok = tokens[idx];

        // Remarks — everything after RMK is remarks
        if tok == "RMK" {
            remarks = Some(tokens[idx + 1..].join(" "));
            break;
        }

        // Wind
        if (tok.ends_with("KT") || tok.ends_with("MPS") || tok.ends_with("KMH"))
            && wind.is_none()
        {
            wind = parse_wind(tok);
            idx += 1;
            // Check for variable wind direction (e.g., 180V240)
            if idx < tokens.len() && tokens[idx].contains('V') && tokens[idx].len() == 7 {
                if let Some(ref mut w) = wind {
                    let parts: Vec<&str> = tokens[idx].split('V').collect();
                    if parts.len() == 2 {
                        w.variable_from = parts[0].parse().ok();
                        w.variable_to = parts[1].parse().ok();
                    }
                }
                idx += 1;
            }
            continue;
        }

        // CAVOK
        if tok == "CAVOK" {
            cavok = true;
            visibility_sm = Some(6.21); // >10km ≈ 6+ SM
            sky_clear = true;
            idx += 1;
            continue;
        }

        // Visibility (SM)
        if tok.ends_with("SM") {
            visibility_sm = parse_visibility_sm(tok);
            idx += 1;
            continue;
        }

        // Visibility (meters, 4-digit)
        if tok.len() == 4 && tok.chars().all(|c| c.is_ascii_digit()) && visibility_m.is_none() {
            visibility_m = tok.parse::<u32>().ok();
            idx += 1;
            continue;
        }

        // SKC / CLR / NSC
        if tok == "SKC" || tok == "CLR" || tok == "NSC" || tok == "NCD" {
            sky_clear = true;
            idx += 1;
            continue;
        }

        // Cloud layers
        if let Some(cloud) = parse_cloud_layer(tok) {
            clouds.push(cloud);
            idx += 1;
            continue;
        }

        // Temperature/Dewpoint (TT/DD)
        if tok.contains('/') && !tok.starts_with('A') && !tok.starts_with('Q') {
            let (t, d) = parse_temp_dewpoint(tok);
            if t.is_some() || d.is_some() {
                temperature_c = t;
                dewpoint_c = d;
                idx += 1;
                continue;
            }
        }

        // Altimeter (A or Q)
        if tok.starts_with('A') && tok.len() == 5 {
            altimeter_inhg = tok[1..].parse::<f64>().ok().map(|v| v / 100.0);
            idx += 1;
            continue;
        }
        if tok.starts_with('Q') && tok.len() >= 5 {
            altimeter_hpa = tok[1..].parse::<f64>().ok();
            idx += 1;
            continue;
        }

        // Present weather (e.g., -RA, +SN, BR, FG, TS, FZRA, etc.)
        if is_weather_code(tok) {
            weather.push(tok.to_string());
            idx += 1;
            continue;
        }

        idx += 1;
    }

    Ok(MetarReport {
        raw: raw.to_string(),
        report_type,
        station,
        observation_time: obs_time,
        day,
        hour,
        minute,
        wind,
        visibility_sm,
        visibility_m,
        clouds,
        temperature_c,
        dewpoint_c,
        altimeter_inhg,
        altimeter_hpa,
        weather,
        sky_clear,
        cavok,
        remarks,
    })
}

/// Build an observation time from day/hour/minute using today's date.
fn build_obs_time(day: Option<u8>, hour: Option<u8>, minute: Option<u8>) -> Option<DateTime<Utc>> {
    let d = day? as u32;
    let h = hour? as u32;
    let m = minute? as u32;
    let now = Utc::now();
    let date = NaiveDate::from_ymd_opt(now.year(), now.month(), d)?;
    let time = chrono::NaiveTime::from_hms_opt(h, m, 0)?;
    let dt = chrono::NaiveDateTime::new(date, time);
    Some(DateTime::from_naive_utc_and_offset(dt, Utc))
}

/// Parse wind token (e.g., "22006KT", "VRB03KT", "18010G25KT").
fn parse_wind(tok: &str) -> Option<Wind> {
    let unit_pos = tok.find("KT").or_else(|| tok.find("MPS")).or_else(|| tok.find("KMH"))?;
    let wind_part = &tok[..unit_pos];

    let (dir, rest) = if let Some(stripped) = wind_part.strip_prefix("VRB") {
        (None, stripped)
    } else if wind_part.len() >= 5 {
        let dir = wind_part[..3].parse::<u16>().ok();
        (dir, &wind_part[3..])
    } else {
        return None;
    };

    let (speed, gust) = if let Some(g_pos) = rest.find('G') {
        let speed = rest[..g_pos].parse::<u16>().ok()?;
        let gust = rest[g_pos + 1..].parse::<u16>().ok();
        (speed, gust)
    } else {
        (rest.parse::<u16>().ok()?, None)
    };

    Some(Wind {
        direction_deg: dir,
        speed_kt: speed,
        gust_kt: gust,
        variable_from: None,
        variable_to: None,
    })
}

/// Parse visibility in statute miles (e.g., "10SM", "1/2SM", "1SM").
fn parse_visibility_sm(tok: &str) -> Option<f64> {
    let s = tok.trim_end_matches("SM");
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let num: f64 = parts[0].parse().ok()?;
            let den: f64 = parts[1].parse().ok()?;
            if den == 0.0 {
                return None;
            }
            return Some(num / den);
        }
    }
    s.parse().ok()
}

/// Parse a cloud layer token (e.g., "FEW250", "BKN040CB", "OVC100").
fn parse_cloud_layer(tok: &str) -> Option<CloudLayer> {
    if tok.len() < 6 {
        return None;
    }
    let cover_str = &tok[..3];
    let cover = if cover_str == "VV/" || tok.starts_with("VV") {
        CloudCover::VerticalVisibility
    } else {
        CloudCover::parse(cover_str)?
    };

    let rest = &tok[3..];
    let alt_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let altitude_ft = alt_str.parse::<u32>().ok()? * 100;

    let cloud_type = {
        let remaining = &rest[alt_str.len()..];
        if remaining.is_empty() {
            None
        } else {
            Some(remaining.to_string())
        }
    };

    Some(CloudLayer {
        cover,
        altitude_ft,
        cloud_type,
    })
}

/// Parse temperature/dewpoint (e.g., "18/10", "M02/M05").
fn parse_temp_dewpoint(tok: &str) -> (Option<i16>, Option<i16>) {
    let parts: Vec<&str> = tok.split('/').collect();
    if parts.len() != 2 {
        return (None, None);
    }
    let t = parse_temp_value(parts[0]);
    let d = parse_temp_value(parts[1]);
    (t, d)
}

fn parse_temp_value(s: &str) -> Option<i16> {
    if s == "M" || s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('M') {
        rest.parse::<i16>().ok().map(|v| -v)
    } else {
        s.parse::<i16>().ok()
    }
}

/// Check if a token is a present weather code.
fn is_weather_code(tok: &str) -> bool {
    // Present weather has intensity prefix (+/-/VC), descriptor, and phenomena
    let phenomena = [
        "DZ", "RA", "SN", "SG", "IC", "PL", "GR", "GS", "UP", // precipitation
        "BR", "FG", "FU", "VA", "DU", "SA", "HZ", "PY",        // obscuration
        "PO", "SQ", "FC", "SS", "DS",                           // other
        "TS", "SH", "FZ", "MI", "PR", "BC", "BL", "DR",        // descriptors
    ];
    let clean = tok.trim_start_matches(['+', '-'].as_ref()).trim_start_matches("VC");
    if clean.is_empty() {
        return false;
    }
    // Check if made of 2-char weather codes
    let bytes = clean.as_bytes();
    if bytes.len() % 2 != 0 || bytes.len() > 8 {
        return false;
    }
    for chunk in bytes.chunks(2) {
        let code = std::str::from_utf8(chunk).unwrap_or("");
        if !phenomena.contains(&code) {
            return false;
        }
    }
    true
}

/// Parse a simple TAF.
pub fn parse_taf(raw: &str) -> Result<TafReport, ConnectorError> {
    let raw = raw.trim();
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    if tokens.is_empty() || tokens[0] != "TAF" {
        return Err(ConnectorError::ParseError("TAF: expected 'TAF' prefix".into()));
    }

    let mut idx = 1;
    // Skip AMD / COR
    if idx < tokens.len() && (tokens[idx] == "AMD" || tokens[idx] == "COR") {
        idx += 1;
    }

    let station = if idx < tokens.len() {
        let s = tokens[idx].to_string();
        idx += 1;
        s
    } else {
        return Err(ConnectorError::ParseError("TAF: missing station".into()));
    };

    // Issue time
    let issue_time = if idx < tokens.len() && tokens[idx].ends_with('Z') {
        let t = tokens[idx];
        let d = t.get(0..2).and_then(|s| s.parse::<u8>().ok());
        let h = t.get(2..4).and_then(|s| s.parse::<u8>().ok());
        let m = t.get(4..6).and_then(|s| s.parse::<u8>().ok());
        build_obs_time(d, h, m)
    } else {
        None
    };

    Ok(TafReport {
        raw: raw.to_string(),
        station,
        issue_time,
        valid_from: None,
        valid_to: None,
        groups: Vec::new(), // Simplified — full TAF group parsing is complex
    })
}

// ---------------------------------------------------------------------------
// METAR → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a MetarReport to a SourceEvent.
pub fn metar_to_source_event(
    metar: &MetarReport,
    connector_id: &str,
) -> SourceEvent {
    let entity_id = format!("metar:{}", metar.station);

    let mut properties = HashMap::new();
    properties.insert("station".into(), json!(metar.station));
    properties.insert("report_type".into(), json!(metar.report_type));
    properties.insert("raw".into(), json!(metar.raw));

    if let Some(ref w) = metar.wind {
        if let Some(dir) = w.direction_deg {
            properties.insert("wind_direction_deg".into(), json!(dir));
        } else {
            properties.insert("wind_direction_deg".into(), json!("VRB"));
        }
        properties.insert("wind_speed_kt".into(), json!(w.speed_kt));
        if let Some(g) = w.gust_kt {
            properties.insert("wind_gust_kt".into(), json!(g));
        }
    }

    if let Some(v) = metar.visibility_sm {
        properties.insert("visibility_sm".into(), json!(v));
    }
    if let Some(v) = metar.visibility_m {
        properties.insert("visibility_m".into(), json!(v));
    }

    if !metar.clouds.is_empty() {
        let cloud_info: Vec<serde_json::Value> = metar
            .clouds
            .iter()
            .map(|c| {
                json!({
                    "cover": c.cover.as_str(),
                    "altitude_ft": c.altitude_ft,
                    "cloud_type": c.cloud_type,
                })
            })
            .collect();
        properties.insert("clouds".into(), json!(cloud_info));
    }

    if let Some(t) = metar.temperature_c {
        properties.insert("temperature_c".into(), json!(t));
    }
    if let Some(d) = metar.dewpoint_c {
        properties.insert("dewpoint_c".into(), json!(d));
    }
    if let Some(a) = metar.altimeter_inhg {
        properties.insert("altimeter_inhg".into(), json!(a));
    }
    if let Some(a) = metar.altimeter_hpa {
        properties.insert("altimeter_hpa".into(), json!(a));
    }

    if !metar.weather.is_empty() {
        properties.insert("weather".into(), json!(metar.weather));
    }

    properties.insert("sky_clear".into(), json!(metar.sky_clear));
    properties.insert("cavok".into(), json!(metar.cavok));

    if let Some(ref r) = metar.remarks {
        properties.insert("remarks".into(), json!(r));
    }

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "weather_station".into(),
        properties,
        timestamp: metar.observation_time.unwrap_or_else(Utc::now),
        latitude: None,
        longitude: None,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct MetarConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl MetarConnector {
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
impl Connector for MetarConnector {
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
                ConnectorError::ConfigError("METAR: url (file path) required".into())
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let running = Arc::clone(&self.running);

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if !running.load(Ordering::Relaxed) {
                break;
            }

            if let Ok(metar) = parse_metar(line) {
                let event = metar_to_source_event(&metar, &connector_id);
                if tx.send(event).await.is_err() {
                    break;
                }
                events_processed.fetch_add(1, Ordering::Relaxed);
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
                "METAR connector is not running".into(),
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
    fn test_parse_basic_metar() {
        let metar = parse_metar("METAR KJFK 260830Z 22006KT 10SM FEW250 18/10 A3012 RMK AO2").unwrap();
        assert_eq!(metar.station, "KJFK");
        assert_eq!(metar.report_type, "METAR");
        assert_eq!(metar.day, Some(26));
        assert_eq!(metar.hour, Some(8));
        assert_eq!(metar.minute, Some(30));
    }

    #[test]
    fn test_parse_wind() {
        let metar = parse_metar("METAR EGLL 260900Z 22006KT 9999 SCT040 18/10 Q1013").unwrap();
        let wind = metar.wind.unwrap();
        assert_eq!(wind.direction_deg, Some(220));
        assert_eq!(wind.speed_kt, 6);
        assert!(wind.gust_kt.is_none());
    }

    #[test]
    fn test_parse_wind_with_gust() {
        let metar = parse_metar("METAR KSFO 260900Z 27015G25KT 10SM OVC020 15/12 A2990").unwrap();
        let wind = metar.wind.unwrap();
        assert_eq!(wind.direction_deg, Some(270));
        assert_eq!(wind.speed_kt, 15);
        assert_eq!(wind.gust_kt, Some(25));
    }

    #[test]
    fn test_parse_variable_wind() {
        let metar = parse_metar("METAR EDDF 260900Z VRB03KT 9999 FEW040 20/12 Q1020").unwrap();
        let wind = metar.wind.unwrap();
        assert!(wind.direction_deg.is_none()); // VRB
        assert_eq!(wind.speed_kt, 3);
    }

    #[test]
    fn test_parse_visibility_sm() {
        let metar = parse_metar("METAR KJFK 260830Z 22006KT 10SM FEW250 18/10 A3012").unwrap();
        assert!((metar.visibility_sm.unwrap() - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_fractional_visibility() {
        let vis = parse_visibility_sm("1/2SM");
        assert!((vis.unwrap() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_parse_cloud_layers() {
        let metar = parse_metar("METAR KJFK 260830Z 22006KT 10SM FEW020 SCT080 BKN120 18/10 A3012").unwrap();
        assert_eq!(metar.clouds.len(), 3);
        assert_eq!(metar.clouds[0].cover, CloudCover::Few);
        assert_eq!(metar.clouds[0].altitude_ft, 2000);
        assert_eq!(metar.clouds[1].cover, CloudCover::Scattered);
        assert_eq!(metar.clouds[1].altitude_ft, 8000);
        assert_eq!(metar.clouds[2].cover, CloudCover::Broken);
        assert_eq!(metar.clouds[2].altitude_ft, 12000);
    }

    #[test]
    fn test_parse_cloud_with_type() {
        let cloud = parse_cloud_layer("BKN040CB").unwrap();
        assert_eq!(cloud.cover, CloudCover::Broken);
        assert_eq!(cloud.altitude_ft, 4000);
        assert_eq!(cloud.cloud_type, Some("CB".into()));
    }

    #[test]
    fn test_parse_temperature() {
        let metar = parse_metar("METAR KJFK 260830Z 22006KT 10SM FEW250 18/10 A3012").unwrap();
        assert_eq!(metar.temperature_c, Some(18));
        assert_eq!(metar.dewpoint_c, Some(10));
    }

    #[test]
    fn test_parse_negative_temperature() {
        let metar = parse_metar("METAR CYUL 260830Z 36010KT 15SM OVC030 M02/M05 A2985").unwrap();
        assert_eq!(metar.temperature_c, Some(-2));
        assert_eq!(metar.dewpoint_c, Some(-5));
    }

    #[test]
    fn test_parse_altimeter() {
        let metar = parse_metar("METAR KJFK 260830Z 22006KT 10SM FEW250 18/10 A3012").unwrap();
        assert!((metar.altimeter_inhg.unwrap() - 30.12).abs() < 0.01);
    }

    #[test]
    fn test_parse_altimeter_qnh() {
        let metar = parse_metar("METAR EGLL 260900Z 22006KT 9999 SCT040 18/10 Q1013").unwrap();
        assert!((metar.altimeter_hpa.unwrap() - 1013.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_weather_codes() {
        let metar = parse_metar("METAR KJFK 260830Z 22006KT 3SM -RA BR OVC010 12/10 A2990").unwrap();
        assert!(metar.weather.contains(&"-RA".to_string()));
        assert!(metar.weather.contains(&"BR".to_string()));
    }

    #[test]
    fn test_parse_cavok() {
        let metar = parse_metar("METAR LFPG 260900Z 18008KT CAVOK 22/14 Q1018").unwrap();
        assert!(metar.cavok);
        assert!(metar.sky_clear);
    }

    #[test]
    fn test_parse_sky_clear() {
        let metar = parse_metar("METAR KDEN 260900Z 18005KT 10SM CLR 25/08 A3025").unwrap();
        assert!(metar.sky_clear);
    }

    #[test]
    fn test_parse_remarks() {
        let metar = parse_metar("METAR KJFK 260830Z 22006KT 10SM FEW250 18/10 A3012 RMK AO2 SLP198").unwrap();
        assert!(metar.remarks.is_some());
        assert!(metar.remarks.unwrap().contains("AO2"));
    }

    #[test]
    fn test_metar_to_source_event() {
        let metar = parse_metar("METAR KJFK 260830Z 22006KT 10SM FEW250 18/10 A3012").unwrap();
        let event = metar_to_source_event(&metar, "metar-test");
        assert_eq!(event.entity_type, "weather_station");
        assert_eq!(event.entity_id, "metar:KJFK");
        assert_eq!(event.properties["station"], json!("KJFK"));
        assert_eq!(event.properties["wind_direction_deg"], json!(220));
        assert_eq!(event.properties["wind_speed_kt"], json!(6));
        assert_eq!(event.properties["temperature_c"], json!(18));
    }

    #[test]
    fn test_parse_taf() {
        let taf = parse_taf("TAF KJFK 260500Z 2606/2712 22010KT P6SM FEW250").unwrap();
        assert_eq!(taf.station, "KJFK");
    }

    #[test]
    fn test_speci_report() {
        let metar = parse_metar("SPECI KORD 260945Z 18015G30KT 2SM +TSRA BKN015CB OVC030 20/18 A2975").unwrap();
        assert_eq!(metar.report_type, "SPECI");
        assert_eq!(metar.station, "KORD");
        assert!(metar.weather.contains(&"+TSRA".to_string()));
    }

    #[test]
    fn test_parse_empty_metar() {
        assert!(parse_metar("").is_err());
    }

    #[test]
    fn test_metar_connector_id() {
        let config = ConnectorConfig {
            connector_id: "metar-1".to_string(),
            connector_type: "metar".to_string(),
            url: None,
            entity_type: "weather_station".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = MetarConnector::new(config);
        assert_eq!(connector.connector_id(), "metar-1");
    }

    #[tokio::test]
    async fn test_metar_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "metar-h".to_string(),
            connector_type: "metar".to_string(),
            url: None,
            entity_type: "weather_station".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = MetarConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_is_weather_code() {
        assert!(is_weather_code("-RA"));
        assert!(is_weather_code("+SN"));
        assert!(is_weather_code("BR"));
        assert!(is_weather_code("FG"));
        assert!(is_weather_code("+TSRA"));
        assert!(is_weather_code("FZRA"));
        assert!(!is_weather_code("METAR"));
        assert!(!is_weather_code("RMK"));
    }
}
