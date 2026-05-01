use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use quick_xml::events::Event as XmlEvent;
use quick_xml::Reader;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CoT type‑code → ORP entity classification
// ---------------------------------------------------------------------------

/// Determine the ORP entity type from a CoT type string.
/// CoT types follow the pattern: a-{affiliation}-{battle_dimension}-...
///   Affiliations: f=friendly, h=hostile, n=neutral, u=unknown
///   Battle dimensions: A=air, G=ground, S=surface(sea), U=subsurface, P=space
/// Atoms starting with "b-" are sensor data (e.g. b-m-p-s-m = earthquake).
pub fn cot_type_to_entity(cot_type: &str) -> CotEntityMapping {
    let parts: Vec<&str> = cot_type.split('-').collect();
    if parts.is_empty() {
        return CotEntityMapping {
            entity_type: "unknown".to_string(),
            classification: "unknown".to_string(),
            domain: "unknown".to_string(),
        };
    }

    match parts[0] {
        "a" => {
            // Atom – a real‑world object
            let classification = parts
                .get(1)
                .map(|a| match *a {
                    "f" => "friendly",
                    "h" => "hostile",
                    "n" => "neutral",
                    "u" => "unknown",
                    "s" => "suspect",
                    "j" => "joker",
                    "k" => "faker",
                    "o" => "none",
                    "p" => "pending",
                    _ => "unknown",
                })
                .unwrap_or("unknown")
                .to_string();

            let (entity_type, domain) = parts
                .get(2)
                .map(|d| match *d {
                    "A" => ("aircraft", "air"),
                    "G" => ("ground_unit", "ground"),
                    "S" => ("vessel", "surface"),
                    "U" => ("submarine", "subsurface"),
                    "P" => ("space_object", "space"),
                    "F" => ("sof_unit", "special_operations"),
                    _ => ("unit", "unknown"),
                })
                .unwrap_or(("unit", "unknown"));

            CotEntityMapping {
                entity_type: entity_type.to_string(),
                classification,
                domain: domain.to_string(),
            }
        }
        "b" => {
            // Bits – sensor data / observations
            let domain = parts
                .get(1)
                .map(|d| match *d {
                    "m" => "sensor",
                    "r" => "reference",
                    "d" => "detection",
                    "a" => "alarm",
                    _ => "sensor",
                })
                .unwrap_or("sensor");

            CotEntityMapping {
                entity_type: "sensor_point".to_string(),
                classification: "none".to_string(),
                domain: domain.to_string(),
            }
        }
        "t" => {
            // Tactical graphics
            CotEntityMapping {
                entity_type: "tactical_graphic".to_string(),
                classification: "none".to_string(),
                domain: "tactical".to_string(),
            }
        }
        _ => CotEntityMapping {
            entity_type: "unknown".to_string(),
            classification: "unknown".to_string(),
            domain: "unknown".to_string(),
        },
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CotEntityMapping {
    pub entity_type: String,
    pub classification: String,
    pub domain: String,
}

// ---------------------------------------------------------------------------
// CoT event model
// ---------------------------------------------------------------------------

/// Parsed representation of a CoT XML `<event>` element.
#[derive(Clone, Debug)]
pub struct CotEvent {
    pub uid: String,
    pub event_type: String,
    pub how: String,
    pub time: String,
    pub start: String,
    pub stale: String,
    /// latitude from <point>
    pub lat: f64,
    /// longitude from <point>
    pub lon: f64,
    /// height above ellipsoid from <point>
    pub hae: f64,
    /// circular error from <point>
    pub ce: f64,
    /// linear error from <point>
    pub le: f64,
    /// callsign from <contact> in <detail>
    pub callsign: Option<String>,
    /// remarks text
    pub remarks: Option<String>,
    /// extra key‑value pairs found in <detail>
    pub detail_fields: HashMap<String, String>,
}

impl CotEvent {
    /// Convert to ORP SourceEvent.
    pub fn to_source_event(&self, connector_id: &str) -> SourceEvent {
        let mapping = cot_type_to_entity(&self.event_type);

        let mut properties: HashMap<String, serde_json::Value> = HashMap::new();
        properties.insert("cot_type".into(), serde_json::json!(self.event_type));
        properties.insert("cot_how".into(), serde_json::json!(self.how));
        properties.insert(
            "classification".into(),
            serde_json::json!(mapping.classification),
        );
        properties.insert("domain".into(), serde_json::json!(mapping.domain));
        properties.insert("hae".into(), serde_json::json!(self.hae));
        properties.insert("ce".into(), serde_json::json!(self.ce));
        properties.insert("le".into(), serde_json::json!(self.le));
        properties.insert("stale".into(), serde_json::json!(self.stale));

        if let Some(ref cs) = self.callsign {
            properties.insert("callsign".into(), serde_json::json!(cs));
        }
        if let Some(ref rmk) = self.remarks {
            properties.insert("remarks".into(), serde_json::json!(rmk));
        }
        for (k, v) in &self.detail_fields {
            properties.insert(k.clone(), serde_json::json!(v));
        }

        let ts = chrono::DateTime::parse_from_rfc3339(&self.time)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id: format!("cot:{}", self.uid),
            entity_type: mapping.entity_type,
            properties,
            timestamp: ts,
            latitude: Some(self.lat),
            longitude: Some(self.lon),
        }
    }
}

// ---------------------------------------------------------------------------
// CoT XML parser
// ---------------------------------------------------------------------------

/// Parse a CoT XML string into a `CotEvent`.
///
/// Expects the standard `<event>` root element with `<point>` and optional `<detail>`.
pub fn parse_cot_xml(xml: &str) -> Result<CotEvent, ConnectorError> {
    let mut reader = Reader::from_str(xml);

    let mut uid = String::new();
    let mut event_type = String::new();
    let mut how = String::new();
    let mut time = String::new();
    let mut start = String::new();
    let mut stale = String::new();
    let mut lat: f64 = 0.0;
    let mut lon: f64 = 0.0;
    let mut hae: f64 = 0.0;
    let mut ce: f64 = 0.0;
    let mut le: f64 = 0.0;
    let mut callsign: Option<String> = None;
    let mut remarks: Option<String> = None;
    let mut detail_fields: HashMap<String, String> = HashMap::new();

    let mut in_detail = false;
    let mut in_remarks = false;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(ref e)) | Ok(XmlEvent::Empty(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "event" => {
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.as_ref()).to_string();
                            let val =
                                String::from_utf8_lossy(&attr.value).to_string();
                            match key.as_str() {
                                "uid" => uid = val,
                                "type" => event_type = val,
                                "how" => how = val,
                                "time" => time = val,
                                "start" => start = val,
                                "stale" => stale = val,
                                _ => {}
                            }
                        }
                    }
                    "point" => {
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.as_ref()).to_string();
                            let val =
                                String::from_utf8_lossy(&attr.value).to_string();
                            match key.as_str() {
                                "lat" => lat = val.parse().unwrap_or(0.0),
                                "lon" => lon = val.parse().unwrap_or(0.0),
                                "hae" => hae = val.parse().unwrap_or(0.0),
                                "ce" => ce = val.parse().unwrap_or(9999999.0),
                                "le" => le = val.parse().unwrap_or(9999999.0),
                                _ => {}
                            }
                        }
                    }
                    "detail" => {
                        in_detail = true;
                    }
                    "contact" if in_detail => {
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.as_ref()).to_string();
                            let val =
                                String::from_utf8_lossy(&attr.value).to_string();
                            if key == "callsign" {
                                callsign = Some(val);
                            } else {
                                detail_fields
                                    .insert(format!("contact_{}", key), val);
                            }
                        }
                    }
                    "remarks" if in_detail => {
                        in_remarks = true;
                    }
                    _ if in_detail => {
                        // Capture arbitrary detail child elements as key‑values
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.as_ref()).to_string();
                            let val =
                                String::from_utf8_lossy(&attr.value).to_string();
                            detail_fields
                                .insert(format!("{}_{}", tag, key), val);
                        }
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Text(ref t)) if in_remarks => {
                let text = t.unescape().unwrap_or_default().to_string();
                if !text.trim().is_empty() {
                    remarks = Some(text.trim().to_string());
                }
            }
            Ok(XmlEvent::End(ref e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "detail" {
                    in_detail = false;
                }
                if tag == "remarks" {
                    in_remarks = false;
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(e) => {
                return Err(ConnectorError::ParseError(format!(
                    "CoT XML parse error: {}",
                    e
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    if uid.is_empty() {
        return Err(ConnectorError::ParseError(
            "CoT event missing 'uid' attribute".to_string(),
        ));
    }

    Ok(CotEvent {
        uid,
        event_type,
        how,
        time,
        start,
        stale,
        lat,
        lon,
        hae,
        ce,
        le,
        callsign,
        remarks,
        detail_fields,
    })
}

// ---------------------------------------------------------------------------
// CoT XML emitter (bidirectional — ORP → TAK)
// ---------------------------------------------------------------------------

/// Emit a CoT XML string from a `CotEvent`.
pub fn emit_cot_xml(event: &CotEvent) -> String {
    let mut xml = String::with_capacity(512);
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<event version=\"2.0\" uid=\"{}\" type=\"{}\" how=\"{}\" time=\"{}\" start=\"{}\" stale=\"{}\">\n",
        xml_escape(&event.uid),
        xml_escape(&event.event_type),
        xml_escape(&event.how),
        xml_escape(&event.time),
        xml_escape(&event.start),
        xml_escape(&event.stale),
    ));
    xml.push_str(&format!(
        "  <point lat=\"{}\" lon=\"{}\" hae=\"{}\" ce=\"{}\" le=\"{}\" />\n",
        event.lat, event.lon, event.hae, event.ce, event.le,
    ));

    // Detail section
    let has_detail = event.callsign.is_some()
        || event.remarks.is_some()
        || !event.detail_fields.is_empty();
    if has_detail {
        xml.push_str("  <detail>\n");
        if let Some(ref cs) = event.callsign {
            xml.push_str(&format!(
                "    <contact callsign=\"{}\" />\n",
                xml_escape(cs)
            ));
        }
        if let Some(ref rmk) = event.remarks {
            xml.push_str(&format!(
                "    <remarks>{}</remarks>\n",
                xml_escape(rmk)
            ));
        }
        xml.push_str("  </detail>\n");
    }

    xml.push_str("</event>\n");
    xml
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// CoT producer (ORP → ATAK/WinTAK/iTAK)
// ---------------------------------------------------------------------------

/// Default UDP multicast group / port used by ATAK/WinTAK/iTAK clients.
pub const DEFAULT_COT_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 2, 3, 1);
pub const DEFAULT_COT_PORT: u16 = 6969;

/// Configuration for [`spawn_cot_producer`] — emits ORP `SourceEvent`s as
/// CoT XML packets via UDP for TAK-family clients on the same network.
#[derive(Clone, Debug)]
pub struct CotProducerConfig {
    /// Local socket bind address.  Default `0.0.0.0:6969`.
    pub bind_addr: SocketAddr,
    /// Multicast group; `None` means unicast-only via `ORP_COT_PEERS`.
    pub multicast_group: Option<IpAddr>,
    /// Stale interval (seconds) added to every emitted CoT event.
    pub stale_seconds: u64,
    /// CoT type when the source event does not carry one in `properties`.
    pub cot_type: String,
}

impl Default for CotProducerConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_COT_PORT),
            multicast_group: Some(IpAddr::V4(DEFAULT_COT_MULTICAST)),
            stale_seconds: 60,
            cot_type: "a-f-G".to_string(),
        }
    }
}

/// Parse a producer URL into a [`CotProducerConfig`].
///
/// Schemes:
///   * `cot-out://<group>:<port>` — UDP multicast/unicast
///   * `cot-out-tcp://<host>:<port>` — TCP target (recorded as the dial
///     destination; runtime falls back to UDP unicast via `ORP_COT_PEERS`).
pub fn cot_producer_url_to_config(url: &str) -> Result<CotProducerConfig, ConnectorError> {
    let mut cfg = CotProducerConfig::default();
    let (scheme, rest) = url.split_once("://").ok_or_else(|| {
        ConnectorError::ConfigError(format!("CoT producer URL missing scheme: {url}"))
    })?;
    let (host, port_str) = rest.rsplit_once(':').ok_or_else(|| {
        ConnectorError::ConfigError(format!("CoT producer URL missing port: {url}"))
    })?;
    let port: u16 = port_str.parse().map_err(|_| {
        ConnectorError::ConfigError(format!("CoT producer invalid port in {url}"))
    })?;
    cfg.bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);

    match scheme {
        "cot-out" => {
            let ip: IpAddr = host.parse().map_err(|_| {
                ConnectorError::ConfigError(format!("CoT producer invalid IP in {url}"))
            })?;
            cfg.multicast_group = if is_multicast(&ip) { Some(ip) } else { None };
            Ok(cfg)
        }
        "cot-out-tcp" => {
            // No DNS lookup here — only literal IPs are recorded.
            cfg.multicast_group = host.parse::<IpAddr>().ok();
            Ok(cfg)
        }
        _ => Err(ConnectorError::ConfigError(format!(
            "CoT producer unsupported scheme: {scheme}"
        ))),
    }
}

fn is_multicast(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_multicast(),
        IpAddr::V6(v6) => v6.is_multicast(),
    }
}

/// Build a `CotEvent` from a `SourceEvent` using the producer config.
/// Honours `properties.cot_type`; otherwise specialises by entity_type.
pub fn source_event_to_cot(ev: &SourceEvent, cfg: &CotProducerConfig) -> CotEvent {
    let stale = (Utc::now() + chrono::Duration::seconds(cfg.stale_seconds as i64))
        .to_rfc3339();
    let event_type = ev
        .properties
        .get("cot_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| match ev.entity_type.as_str() {
            "aircraft" | "sar_aircraft" => "a-f-A".to_string(),
            "ship" | "vessel" => "a-f-S".to_string(),
            "submarine" => "a-f-U".to_string(),
            _ => cfg.cot_type.clone(),
        });
    let callsign = ev
        .properties
        .get("callsign")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| Some(ev.entity_id.clone()));
    CotEvent {
        uid: ev.entity_id.clone(),
        event_type,
        how: "m-g".to_string(),
        time: ev.timestamp.to_rfc3339(),
        start: ev.timestamp.to_rfc3339(),
        stale,
        lat: ev.latitude.unwrap_or(0.0),
        lon: ev.longitude.unwrap_or(0.0),
        hae: 0.0,
        ce: 9999999.0,
        le: 9999999.0,
        callsign,
        remarks: None,
        detail_fields: HashMap::new(),
    }
}

/// Read the `ORP_COT_PEERS` env var into a list of unicast peers.
fn read_unicast_peers() -> Vec<SocketAddr> {
    std::env::var("ORP_COT_PEERS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<SocketAddr>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Spawn a background task that consumes `SourceEvent`s and emits CoT
/// XML packets via UDP to the configured multicast group and any unicast
/// peers from `ORP_COT_PEERS`.  Send failures are logged but never panic.
pub fn spawn_cot_producer(
    config: CotProducerConfig,
    mut rx: tokio::sync::mpsc::Receiver<SourceEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let socket = match tokio::net::UdpSocket::bind(config.bind_addr).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(bind = %config.bind_addr, error = %e,
                    "CoT producer cannot bind — task exiting");
                return;
            }
        };
        if let Some(IpAddr::V4(g)) = config.multicast_group {
            if g.is_multicast() {
                if let Err(e) = socket.join_multicast_v4(g, Ipv4Addr::UNSPECIFIED) {
                    tracing::warn!(group = %g, error = %e, "CoT producer mcast join failed");
                }
            }
        }
        let _ = socket.set_multicast_loop_v4(true);
        let peers = read_unicast_peers();
        let mut fails: u64 = 0;
        let local_port = socket.local_addr().map(|a| a.port()).unwrap_or(config.bind_addr.port());
        tracing::info!(bind = %config.bind_addr, multicast = ?config.multicast_group,
            peers = ?peers, "CoT producer started");
        while let Some(ev) = rx.recv().await {
            let xml = emit_cot_xml(&source_event_to_cot(&ev, &config));
            let bytes = xml.as_bytes();
            if let Some(g) = config.multicast_group {
                let dst = SocketAddr::new(g, local_port);
                if let Err(e) = socket.send_to(bytes, dst).await {
                    fails = fails.saturating_add(1);
                    tracing::warn!(target = %dst, error = %e, fails, "CoT mcast send failed");
                }
            }
            for peer in &peers {
                if let Err(e) = socket.send_to(bytes, peer).await {
                    fails = fails.saturating_add(1);
                    tracing::warn!(target = %peer, error = %e, fails, "CoT unicast send failed");
                }
            }
        }
        tracing::info!(fails, "CoT producer channel closed");
    })
}

// ---------------------------------------------------------------------------
// Connector implementation
// ---------------------------------------------------------------------------

/// CoT (Cursor on Target) connector for TAK / ATAK integration.
///
/// Receives CoT XML events over UDP multicast (239.2.3.1:6969 default) or TCP.
pub struct CotConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl CotConnector {
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
impl Connector for CotConnector {
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
            "CoT connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let connector_id = self.config.connector_id.clone();
        let url = self.config.url.clone();

        tokio::spawn(async move {
            // If a UDP url is configured, try real connection
            if let Some(ref url_str) = url {
                if let Some(addr) = url_str.strip_prefix("udp://") {
                    match tokio::net::UdpSocket::bind(addr).await {
                        Ok(socket) => {
                            tracing::info!("CoT listening on UDP {}", addr);
                            let mut buf = vec![0u8; 65535];
                            while running.load(Ordering::SeqCst) {
                                match socket.recv_from(&mut buf).await {
                                    Ok((n, _src)) => {
                                        if let Ok(xml) = std::str::from_utf8(&buf[..n]) {
                                            match parse_cot_xml(xml) {
                                                Ok(cot) => {
                                                    let event = cot.to_source_event(
                                                        &connector_id,
                                                    );
                                                    if tx.send(event).await.is_err() {
                                                        return;
                                                    }
                                                    events_count.fetch_add(
                                                        1,
                                                        Ordering::Relaxed,
                                                    );
                                                }
                                                Err(e) => {
                                                    tracing::warn!("CoT parse error: {}", e);
                                                    errors_count.fetch_add(
                                                        1,
                                                        Ordering::Relaxed,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("CoT UDP recv error: {}", e);
                                        errors_count
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }
                            return;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Cannot bind CoT UDP {}: {}, using demo data",
                                addr,
                                e
                            );
                        }
                    }
                }
            }

            // Demo mode: emit synthetic CoT events
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(3));
            let demo_events = vec![
                ("ALPHA-1", "a-f-G-U-C", 38.8977, -77.0365, "Alpha Squad"),
                ("BRAVO-2", "a-f-G-E-V", 34.0522, -118.2437, "Bravo Team"),
                ("HOSTILE-1", "a-h-A", 36.12, 37.16, "Unknown Aircraft"),
                ("NEUTRAL-3", "a-n-S", 51.5074, -0.1278, "Commercial Vessel"),
                ("SENSOR-5", "b-m-p-s-m", 35.6895, 139.6917, "Seismic Sensor"),
            ];

            let mut counter = 0u64;
            while running.load(Ordering::SeqCst) {
                interval.tick().await;
                for (uid, ctype, lat, lon, cs) in &demo_events {
                    let jitter = (counter as f64 % 30.0) * 0.0001;
                    let now = Utc::now().to_rfc3339();
                    let cot = CotEvent {
                        uid: uid.to_string(),
                        event_type: ctype.to_string(),
                        how: "m-g".to_string(),
                        time: now.clone(),
                        start: now.clone(),
                        stale: now.clone(),
                        lat: lat + jitter,
                        lon: lon + jitter * 0.5,
                        hae: 0.0,
                        ce: 10.0,
                        le: 10.0,
                        callsign: Some(cs.to_string()),
                        remarks: None,
                        detail_fields: HashMap::new(),
                    };
                    let event = cot.to_source_event(&connector_id);
                    if tx.send(event).await.is_err() {
                        return;
                    }
                    events_count.fetch_add(1, Ordering::Relaxed);
                }
                counter += 1;
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "CoT connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "CoT connector not running".to_string(),
            ))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        // `Some(Utc::now())` would lie to ops dashboards. We don't yet
        // track the per-event timestamp, so report None until we do.
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
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

    fn sample_cot_xml() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<event version="2.0" uid="ALPHA-1" type="a-f-G-U-C" how="m-g"
       time="2026-03-26T12:00:00Z" start="2026-03-26T12:00:00Z"
       stale="2026-03-26T12:05:00Z">
  <point lat="38.8977" lon="-77.0365" hae="50.0" ce="10.0" le="5.0" />
  <detail>
    <contact callsign="Alpha Squad" />
    <remarks>On patrol near objective</remarks>
  </detail>
</event>"#
    }

    #[test]
    fn test_parse_basic_cot_event() {
        let cot = parse_cot_xml(sample_cot_xml()).unwrap();
        assert_eq!(cot.uid, "ALPHA-1");
        assert_eq!(cot.event_type, "a-f-G-U-C");
        assert_eq!(cot.how, "m-g");
        assert!((cot.lat - 38.8977).abs() < 0.0001);
        assert!((cot.lon - (-77.0365)).abs() < 0.0001);
        assert!((cot.hae - 50.0).abs() < 0.01);
        assert!((cot.ce - 10.0).abs() < 0.01);
        assert!((cot.le - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_cot_callsign() {
        let cot = parse_cot_xml(sample_cot_xml()).unwrap();
        assert_eq!(cot.callsign, Some("Alpha Squad".to_string()));
    }

    #[test]
    fn test_parse_cot_remarks() {
        let cot = parse_cot_xml(sample_cot_xml()).unwrap();
        assert_eq!(
            cot.remarks,
            Some("On patrol near objective".to_string())
        );
    }

    #[test]
    fn test_parse_cot_timestamps() {
        let cot = parse_cot_xml(sample_cot_xml()).unwrap();
        assert_eq!(cot.time, "2026-03-26T12:00:00Z");
        assert_eq!(cot.stale, "2026-03-26T12:05:00Z");
    }

    #[test]
    fn test_cot_type_friendly_ground() {
        let m = cot_type_to_entity("a-f-G-U-C");
        assert_eq!(m.entity_type, "ground_unit");
        assert_eq!(m.classification, "friendly");
        assert_eq!(m.domain, "ground");
    }

    #[test]
    fn test_cot_type_hostile_aircraft() {
        let m = cot_type_to_entity("a-h-A");
        assert_eq!(m.entity_type, "aircraft");
        assert_eq!(m.classification, "hostile");
        assert_eq!(m.domain, "air");
    }

    #[test]
    fn test_cot_type_neutral_surface() {
        let m = cot_type_to_entity("a-n-S");
        assert_eq!(m.entity_type, "vessel");
        assert_eq!(m.classification, "neutral");
        assert_eq!(m.domain, "surface");
    }

    #[test]
    fn test_cot_type_unknown() {
        let m = cot_type_to_entity("a-u-P");
        assert_eq!(m.entity_type, "space_object");
        assert_eq!(m.classification, "unknown");
    }

    #[test]
    fn test_cot_type_sensor() {
        let m = cot_type_to_entity("b-m-p-s-m");
        assert_eq!(m.entity_type, "sensor_point");
        assert_eq!(m.domain, "sensor");
    }

    #[test]
    fn test_cot_type_tactical() {
        let m = cot_type_to_entity("t-x-y");
        assert_eq!(m.entity_type, "tactical_graphic");
    }

    #[test]
    fn test_cot_to_source_event() {
        let cot = parse_cot_xml(sample_cot_xml()).unwrap();
        let event = cot.to_source_event("cot-test");
        assert_eq!(event.connector_id, "cot-test");
        assert_eq!(event.entity_id, "cot:ALPHA-1");
        assert_eq!(event.entity_type, "ground_unit");
        assert_eq!(
            event.properties.get("classification").unwrap(),
            &serde_json::json!("friendly")
        );
        assert_eq!(
            event.properties.get("callsign").unwrap(),
            &serde_json::json!("Alpha Squad")
        );
    }

    #[test]
    fn test_emit_cot_xml_roundtrip() {
        let original = parse_cot_xml(sample_cot_xml()).unwrap();
        let xml_out = emit_cot_xml(&original);
        let reparsed = parse_cot_xml(&xml_out).unwrap();
        assert_eq!(original.uid, reparsed.uid);
        assert_eq!(original.event_type, reparsed.event_type);
        assert!((original.lat - reparsed.lat).abs() < 0.0001);
        assert!((original.lon - reparsed.lon).abs() < 0.0001);
        assert_eq!(original.callsign, reparsed.callsign);
    }

    #[test]
    fn test_emit_cot_xml_contains_expected_tags() {
        let cot = parse_cot_xml(sample_cot_xml()).unwrap();
        let xml = emit_cot_xml(&cot);
        assert!(xml.contains("<event "));
        assert!(xml.contains("<point "));
        assert!(xml.contains("<detail>"));
        assert!(xml.contains("<contact "));
        assert!(xml.contains("</event>"));
    }

    #[test]
    fn test_parse_cot_missing_uid() {
        let xml = r#"<event type="a-f-G" how="m-g" time="2026-03-26T12:00:00Z" start="2026-03-26T12:00:00Z" stale="2026-03-26T12:05:00Z">
  <point lat="0" lon="0" hae="0" ce="0" le="0"/>
</event>"#;
        assert!(parse_cot_xml(xml).is_err());
    }

    #[test]
    fn test_parse_cot_minimal() {
        let xml = r#"<event uid="MIN-1" type="a-u-G" how="h-e" time="2026-03-26T00:00:00Z" start="2026-03-26T00:00:00Z" stale="2026-03-26T00:10:00Z">
  <point lat="0.0" lon="0.0" hae="0" ce="9999999" le="9999999"/>
</event>"#;
        let cot = parse_cot_xml(xml).unwrap();
        assert_eq!(cot.uid, "MIN-1");
        assert!(cot.callsign.is_none());
        assert!(cot.remarks.is_none());
    }

    #[test]
    fn test_parse_cot_with_extra_detail_elements() {
        let xml = r#"<event uid="EXT-1" type="a-f-G" how="m-g" time="2026-03-26T00:00:00Z" start="2026-03-26T00:00:00Z" stale="2026-03-26T00:10:00Z">
  <point lat="40.0" lon="-74.0" hae="100" ce="5" le="5"/>
  <detail>
    <contact callsign="Bravo" />
    <status readiness="true" />
    <track course="90" speed="5.0" />
  </detail>
</event>"#;
        let cot = parse_cot_xml(xml).unwrap();
        assert_eq!(cot.callsign, Some("Bravo".to_string()));
        assert_eq!(
            cot.detail_fields.get("status_readiness"),
            Some(&"true".to_string())
        );
        assert_eq!(
            cot.detail_fields.get("track_course"),
            Some(&"90".to_string())
        );
    }

    #[test]
    fn test_cot_connector_id() {
        let config = ConnectorConfig {
            connector_id: "cot-test-id".to_string(),
            connector_type: "cot".to_string(),
            url: None,
            entity_type: "unit".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = CotConnector::new(config);
        assert_eq!(connector.connector_id(), "cot-test-id");
    }

    #[tokio::test]
    async fn test_cot_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "cot-health".to_string(),
            connector_type: "cot".to_string(),
            url: None,
            entity_type: "unit".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = CotConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("a<b>c&d\"e'f"), "a&lt;b&gt;c&amp;d&quot;e&apos;f");
    }

    #[test]
    fn test_cot_type_suspect() {
        let m = cot_type_to_entity("a-s-G");
        assert_eq!(m.classification, "suspect");
    }

    #[test]
    fn test_cot_type_subsurface() {
        let m = cot_type_to_entity("a-h-U");
        assert_eq!(m.entity_type, "submarine");
        assert_eq!(m.domain, "subsurface");
    }

    #[test]
    fn test_cot_type_empty() {
        let m = cot_type_to_entity("");
        assert_eq!(m.entity_type, "unknown");
    }

    #[test]
    fn test_parse_invalid_xml() {
        assert!(parse_cot_xml("<not valid xml>>>").is_err());
    }

    // ── Producer (ORP → TAK) ─────────────────────────────────────────────

    fn make_event(et: &str, lat: f64, lon: f64) -> SourceEvent {
        SourceEvent {
            connector_id: "t".into(),
            entity_id: format!("{}:1", et),
            entity_type: et.into(),
            properties: HashMap::new(),
            timestamp: Utc::now(),
            latitude: Some(lat),
            longitude: Some(lon),
        }
    }

    #[test]
    fn test_cot_producer_config_defaults() {
        let cfg = CotProducerConfig::default();
        assert_eq!(cfg.bind_addr.port(), 6969);
        assert_eq!(cfg.bind_addr.ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(cfg.multicast_group, Some(IpAddr::V4(Ipv4Addr::new(239, 2, 3, 1))));
        assert_eq!(cfg.stale_seconds, 60);
        assert_eq!(cfg.cot_type, "a-f-G");
    }

    #[test]
    fn test_cot_producer_url_parsing() {
        let mc = cot_producer_url_to_config("cot-out://239.2.3.1:6969").unwrap();
        assert_eq!(mc.bind_addr.port(), 6969);
        assert_eq!(mc.multicast_group, Some(IpAddr::V4(Ipv4Addr::new(239, 2, 3, 1))));

        let alt = cot_producer_url_to_config("cot-out://224.0.0.1:18999").unwrap();
        assert_eq!(alt.bind_addr.port(), 18999);

        // Non-multicast IP clears the group
        let uc = cot_producer_url_to_config("cot-out://10.0.0.1:6969").unwrap();
        assert!(uc.multicast_group.is_none());

        let tcp = cot_producer_url_to_config("cot-out-tcp://127.0.0.1:8089").unwrap();
        assert_eq!(tcp.bind_addr.port(), 8089);
        assert_eq!(tcp.multicast_group, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));

        assert!(cot_producer_url_to_config("foo://bar").is_err());
        assert!(cot_producer_url_to_config("no-scheme").is_err());
        assert!(cot_producer_url_to_config("cot-out://no-port").is_err());
    }

    #[test]
    fn test_source_event_to_cot_ship_and_aircraft() {
        let cfg = CotProducerConfig::default();

        // Ship with explicit callsign
        let mut ship_props: HashMap<String, serde_json::Value> = HashMap::new();
        ship_props.insert("callsign".into(), serde_json::json!("Black Pearl"));
        let mut ship = make_event("ship", 36.123, -121.456);
        ship.properties = ship_props;
        let cot_s = source_event_to_cot(&ship, &cfg);
        assert_eq!(cot_s.event_type, "a-f-S");
        assert_eq!(cot_s.callsign.as_deref(), Some("Black Pearl"));
        let xml_s = emit_cot_xml(&cot_s);
        assert!(xml_s.contains("type=\"a-f-S\""));
        assert!(xml_s.contains("lat=\"36.123\""));
        assert!(xml_s.contains("lon=\"-121.456\""));

        // Aircraft falls back to entity_id as callsign and "a-f-A" type
        let air = make_event("aircraft", 51.5, -0.1);
        let cot_a = source_event_to_cot(&air, &cfg);
        assert_eq!(cot_a.event_type, "a-f-A");
        assert!(emit_cot_xml(&cot_a).contains("type=\"a-f-A\""));

        // Property override beats entity_type defaults
        let mut over_props: HashMap<String, serde_json::Value> = HashMap::new();
        over_props.insert("cot_type".into(), serde_json::json!("a-h-A"));
        let mut over = make_event("ship", 0.0, 0.0);
        over.properties = over_props;
        assert_eq!(source_event_to_cot(&over, &cfg).event_type, "a-h-A");
    }

    #[tokio::test]
    async fn test_spawn_cot_producer_unicast_round_trip() {
        // Bind a receiver on an ephemeral port; instruct the producer to
        // send unicast there via ORP_COT_PEERS.  Multicast is disabled in
        // the config to keep the test deterministic.
        let recv = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let recv_addr = recv.local_addr().unwrap();
        // SAFETY: env mutation is global; this test runs with default
        // cargo test threading and clears the var on exit.
        unsafe { std::env::set_var("ORP_COT_PEERS", recv_addr.to_string()); }

        let cfg = CotProducerConfig {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            multicast_group: None,
            ..CotProducerConfig::default()
        };

        let (tx, rx) = tokio::sync::mpsc::channel::<SourceEvent>(4);
        let handle = spawn_cot_producer(cfg, rx);
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        tx.send(make_event("sar_aircraft", 45.0, -90.0)).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let (n, _src) = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            recv.recv_from(&mut buf),
        )
        .await
        .expect("CoT packet timeout")
        .expect("recv_from failed");
        let xml = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(xml.contains("<event "));
        assert!(xml.contains("type=\"a-f-A\""));
        assert!(xml.contains("lat=\"45\""));
        assert!(xml.contains("lon=\"-90\""));

        drop(tx);
        let _ = handle.await;
        unsafe { std::env::remove_var("ORP_COT_PEERS"); }
    }
}
