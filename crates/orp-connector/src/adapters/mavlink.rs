//! MAVLink v2 adapter — drone telemetry over UDP/TCP.
//!
//! [MAVLink](https://mavlink.io) is the dominant micro-air-vehicle telemetry
//! protocol. PX4, ArduPilot, Skydio, Auterion and ModalAI ground stations all
//! speak it. A drone broadcasts UDP packets to a ground-control station every
//! few hundred milliseconds; each packet carries one MAVLink frame with a
//! `system_id` / `component_id` pair identifying the originating vehicle.
//!
//! # Supported message types (common dialect)
//!
//! | Message              | Effect on entity                                       |
//! |----------------------|--------------------------------------------------------|
//! | `HEARTBEAT`          | Register / refresh drone (`autopilot`, `vehicle_type`) |
//! | `GLOBAL_POSITION_INT`| Position update (lat, lon, alt MSL/relative, vx/vy/vz) |
//! | `VFR_HUD`            | Airspeed, ground speed, heading, throttle, climb       |
//! | `ATTITUDE`           | Roll / pitch / yaw + angular rates                     |
//! | `GPS_RAW_INT`        | GPS fix type, satellites visible, HDOP, VDOP           |
//! | `SYS_STATUS`         | Battery voltage / remaining, comm errors               |
//!
//! # URL scheme
//!
//! * `mavlink://0.0.0.0:14550` — bind a UDP socket (default port 14550)
//! * `mavlinktcp://host:port`  — connect to a TCP MAVLink stream (e.g. PX4 SITL)
//!
//! # Trust & safety
//!
//! Drone packets arrive from untrusted radios. Every parser path uses the
//! `mavlink` crate's safe API. There are **no** `.unwrap()` / `.expect()`
//! calls outside `#[cfg(test)]` code on data derived from the wire.

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mavlink::common::MavMessage;
use mavlink::peek_reader::PeekReader;
use mavlink::{read_v2_msg, MavHeader, MAV_STX_V2};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;

/// Default GCS port used by PX4, ArduPilot, QGroundControl and MissionPlanner.
const DEFAULT_MAVLINK_PORT: u16 = 14550;
/// Maximum size of a MAVLink v2 frame (per the spec).
const MAX_MAVLINK_FRAME: usize = 280;

/// Convenience macro — `props.insert("name".into(), json!(value))`.
macro_rules! prop {
    ($props:ident, $key:literal => $value:expr) => {
        $props.insert($key.into(), json!($value));
    };
}

/// Bind / connection target parsed from the configured URL.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Endpoint {
    /// Bind a UDP socket on this address.
    Udp(SocketAddr),
    /// Connect to this TCP endpoint (e.g. PX4 SITL on `127.0.0.1:5760`).
    Tcp(SocketAddr),
}

/// Connector that listens for MAVLink v2 packets and emits [`SourceEvent`]s.
#[derive(Debug)]
pub struct MavlinkConnector {
    config: ConnectorConfig,
    endpoint: Endpoint,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl MavlinkConnector {
    /// Build a connector by reading the bind / connect target out of
    /// `config.url`. The URL must use the `mavlink://` scheme for UDP or
    /// `mavlinktcp://` for TCP.
    pub fn from_connector_config(config: ConnectorConfig) -> Result<Self, ConnectorError> {
        let endpoint = parse_endpoint(config.url.as_deref())?;
        Ok(Self::with_endpoint(config, endpoint))
    }

    /// Convenience constructor that defaults to `0.0.0.0:14550` UDP when the
    /// caller doesn't care about the URL parsing dance.
    pub fn new(config: ConnectorConfig) -> Self {
        let endpoint = config
            .url
            .as_deref()
            .and_then(|u| parse_endpoint(Some(u)).ok())
            .unwrap_or_else(|| Endpoint::Udp(default_bind_addr()));
        Self::with_endpoint(config, endpoint)
    }

    fn with_endpoint(config: ConnectorConfig, endpoint: Endpoint) -> Self {
        Self {
            config,
            endpoint,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Decode a single UDP datagram. Walks the buffer to skip any leading
    /// non-`MAV_STX_V2` bytes (some bridges prepend padding), then hands the
    /// rest off to [`read_v2_msg`]. Returns `None` for malformed packets so
    /// the caller can keep going.
    pub fn decode_v2_datagram(buf: &[u8]) -> Option<(MavHeader, MavMessage)> {
        let stx = buf.iter().position(|b| *b == MAV_STX_V2)?;
        let mut reader = PeekReader::<_, MAX_MAVLINK_FRAME>::new(Cursor::new(&buf[stx..]));
        read_v2_msg::<MavMessage, _>(&mut reader).ok()
    }

    /// Translate a freshly decoded MAVLink frame into a [`SourceEvent`]. Returns
    /// `None` for messages we don't surface (everything outside the supported
    /// set in the module docs).
    pub fn frame_to_event(
        connector_id: &str,
        header: MavHeader,
        msg: &MavMessage,
        timestamp: DateTime<Utc>,
    ) -> Option<SourceEvent> {
        let entity_id = vehicle_entity_id(header.system_id, header.component_id);
        let mut p: HashMap<String, JsonValue> = HashMap::new();
        prop!(p, "system_id" => header.system_id);
        prop!(p, "component_id" => header.component_id);

        let mut latitude: Option<f64> = None;
        let mut longitude: Option<f64> = None;

        match msg {
            MavMessage::HEARTBEAT(d) => {
                prop!(p, "message" => "HEARTBEAT");
                prop!(p, "autopilot" => format!("{:?}", d.autopilot));
                prop!(p, "vehicle_type" => format!("{:?}", d.mavtype));
                prop!(p, "base_mode" => d.base_mode.bits());
                prop!(p, "system_status" => format!("{:?}", d.system_status));
                prop!(p, "custom_mode" => d.custom_mode);
                prop!(p, "mavlink_version" => d.mavlink_version);
            }
            MavMessage::GLOBAL_POSITION_INT(d) => {
                // MAVLink encodes lat/lon as int32 deg * 1e7.
                let lat = (d.lat as f64) * 1e-7;
                let lon = (d.lon as f64) * 1e-7;
                if !valid_lat_lon(lat, lon) {
                    return None;
                }
                latitude = Some(lat);
                longitude = Some(lon);
                prop!(p, "message" => "GLOBAL_POSITION_INT");
                prop!(p, "alt_msl_m" => (d.alt as f64) / 1000.0);
                prop!(p, "alt_relative_m" => (d.relative_alt as f64) / 1000.0);
                // Velocity components are cm/s.
                prop!(p, "vx_mps" => (d.vx as f64) / 100.0);
                prop!(p, "vy_mps" => (d.vy as f64) / 100.0);
                prop!(p, "vz_mps" => (d.vz as f64) / 100.0);
                if d.hdg != u16::MAX {
                    prop!(p, "heading_deg" => (d.hdg as f64) / 100.0);
                }
                prop!(p, "time_boot_ms" => d.time_boot_ms);
            }
            MavMessage::VFR_HUD(d) => {
                prop!(p, "message" => "VFR_HUD");
                prop!(p, "airspeed_mps" => d.airspeed);
                prop!(p, "groundspeed_mps" => d.groundspeed);
                prop!(p, "alt_m" => d.alt);
                prop!(p, "climb_mps" => d.climb);
                prop!(p, "heading_deg" => d.heading);
                prop!(p, "throttle_pct" => d.throttle);
            }
            MavMessage::ATTITUDE(d) => {
                prop!(p, "message" => "ATTITUDE");
                prop!(p, "roll_rad" => d.roll);
                prop!(p, "pitch_rad" => d.pitch);
                prop!(p, "yaw_rad" => d.yaw);
                prop!(p, "rollspeed_radps" => d.rollspeed);
                prop!(p, "pitchspeed_radps" => d.pitchspeed);
                prop!(p, "yawspeed_radps" => d.yawspeed);
                prop!(p, "time_boot_ms" => d.time_boot_ms);
            }
            MavMessage::GPS_RAW_INT(d) => {
                let lat = (d.lat as f64) * 1e-7;
                let lon = (d.lon as f64) * 1e-7;
                // Skip the (0,0) / no-fix sentinel, but keep the message so
                // downstream still sees the fix-type / satellite count.
                if valid_lat_lon(lat, lon) && !(d.lat == 0 && d.lon == 0) {
                    latitude = Some(lat);
                    longitude = Some(lon);
                }
                prop!(p, "message" => "GPS_RAW_INT");
                prop!(p, "fix_type" => format!("{:?}", d.fix_type));
                prop!(p, "satellites_visible" => d.satellites_visible);
                if d.eph != u16::MAX {
                    prop!(p, "hdop" => (d.eph as f64) / 100.0);
                }
                if d.epv != u16::MAX {
                    prop!(p, "vdop" => (d.epv as f64) / 100.0);
                }
                prop!(p, "alt_msl_m" => (d.alt as f64) / 1000.0);
                if d.cog != u16::MAX {
                    prop!(p, "course_over_ground" => (d.cog as f64) / 100.0);
                }
                if d.vel != u16::MAX {
                    prop!(p, "ground_speed_mps" => (d.vel as f64) / 100.0);
                }
            }
            MavMessage::SYS_STATUS(d) => {
                prop!(p, "message" => "SYS_STATUS");
                if d.voltage_battery != u16::MAX {
                    prop!(p, "battery_voltage_v" => (d.voltage_battery as f64) / 1000.0);
                }
                // battery_remaining is i8 percent; -1 = autopilot can't estimate.
                if d.battery_remaining >= 0 {
                    prop!(p, "battery_remaining_pct" => d.battery_remaining);
                }
                // current_battery is cA; -1 = unknown.
                if d.current_battery >= 0 {
                    prop!(p, "battery_current_a" => (d.current_battery as f64) / 100.0);
                }
                prop!(p, "errors_comm" => d.errors_comm);
                prop!(p, "errors_count1" => d.errors_count1);
                prop!(p, "errors_count2" => d.errors_count2);
                prop!(p, "errors_count3" => d.errors_count3);
                prop!(p, "errors_count4" => d.errors_count4);
                prop!(p, "drop_rate_comm" => d.drop_rate_comm);
                // load is /1000 of CPU.
                prop!(p, "load_pct" => (d.load as f64) / 10.0);
            }
            // Anything else: skip silently.
            _ => return None,
        }

        Some(SourceEvent {
            connector_id: connector_id.to_string(),
            entity_id,
            entity_type: "drone".to_string(),
            properties: p,
            timestamp,
            latitude,
            longitude,
        })
    }

    #[cfg(test)]
    fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

/// Build the per-vehicle entity id used for dedup. Two packets with the same
/// `(system_id, component_id)` always map to the same entity.
fn vehicle_entity_id(system_id: u8, component_id: u8) -> String {
    format!("mav-{}-{}", system_id, component_id)
}

fn default_bind_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], DEFAULT_MAVLINK_PORT))
}

fn valid_lat_lon(lat: f64, lon: f64) -> bool {
    lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0
}

/// Parse `mavlink://host:port` or `mavlinktcp://host:port`. A missing port
/// defaults to 14550 (UDP only — TCP requires explicit port).
fn parse_endpoint(url: Option<&str>) -> Result<Endpoint, ConnectorError> {
    let url = url
        .ok_or_else(|| ConnectorError::ConfigError("mavlink connector requires a URL".into()))?
        .trim();

    let (is_tcp, rest) = if let Some(r) = url.strip_prefix("mavlink://") {
        (false, r)
    } else if let Some(r) = url.strip_prefix("mavlinktcp://") {
        (true, r)
    } else {
        return Err(ConnectorError::ConfigError(format!(
            "mavlink connector URL must use mavlink:// or mavlinktcp:// scheme, got: {url}"
        )));
    };

    // Strip optional trailing path / query.
    let rest = rest.split('/').next().unwrap_or(rest);
    if rest.is_empty() {
        return Err(ConnectorError::ConfigError(
            "mavlink connector URL is missing host:port".into(),
        ));
    }

    let with_port = if rest.contains(':') {
        rest.to_string()
    } else if is_tcp {
        return Err(ConnectorError::ConfigError(
            "mavlinktcp:// URL must include a port".into(),
        ));
    } else {
        format!("{rest}:{DEFAULT_MAVLINK_PORT}")
    };

    let addr: SocketAddr = with_port.parse().map_err(|e| {
        ConnectorError::ConfigError(format!("invalid mavlink address {with_port}: {e}"))
    })?;

    Ok(if is_tcp {
        Endpoint::Tcp(addr)
    } else {
        Endpoint::Udp(addr)
    })
}

#[async_trait]
impl Connector for MavlinkConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let connector_id = self.config.connector_id.clone();
        let endpoint = self.endpoint.clone();

        tracing::info!(connector_id = %connector_id, ?endpoint, "MAVLink connector starting");

        match endpoint {
            Endpoint::Udp(addr) => {
                let socket = UdpSocket::bind(addr).await.map_err(|e| {
                    ConnectorError::ConnectionError(format!(
                        "MAVLink UDP bind to {addr} failed: {e}"
                    ))
                })?;
                tokio::spawn(drive_udp_socket(
                    socket,
                    running,
                    events_count,
                    errors_count,
                    connector_id,
                    tx,
                ));
            }
            Endpoint::Tcp(addr) => {
                tokio::spawn(drive_tcp_loop(
                    addr,
                    running,
                    events_count,
                    errors_count,
                    connector_id,
                    tx,
                ));
            }
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(connector_id = %self.config.connector_id, "MAVLink connector stopped");
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "MAVLink connector not running".into(),
            ))
        }
    }

    fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    fn stats(&self) -> ConnectorStats {
        // Returning `Some(Utc::now())` would lie: ops dashboards keying off
        // "last event timestamp" would see a fresh value even when no
        // events have flowed. We don't currently track the per-event
        // timestamp in this adapter, so report `None` until we do.
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
            last_event_timestamp: None,
            uptime_seconds: 0,
        }
    }
}

async fn drive_udp_socket(
    socket: UdpSocket,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    connector_id: String,
    tx: tokio::sync::mpsc::Sender<SourceEvent>,
) {
    let mut buf = vec![0u8; MAX_MAVLINK_FRAME * 4];
    while running.load(Ordering::SeqCst) {
        let recv = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            socket.recv_from(&mut buf),
        )
        .await;
        let n = match recv {
            Ok(Ok((n, _))) => n,
            Ok(Err(e)) => {
                tracing::debug!("MAVLink UDP recv error: {e}");
                errors_count.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            Err(_) => continue, // timeout — re-check running flag
        };
        if n == 0 {
            continue;
        }
        if let Some((header, msg)) = MavlinkConnector::decode_v2_datagram(&buf[..n]) {
            if let Some(event) =
                MavlinkConnector::frame_to_event(&connector_id, header, &msg, Utc::now())
            {
                if tx.send(event).await.is_err() {
                    running.store(false, Ordering::SeqCst);
                    break;
                }
                events_count.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            errors_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

async fn drive_tcp_loop(
    addr: SocketAddr,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
    connector_id: String,
    tx: tokio::sync::mpsc::Sender<SourceEvent>,
) {
    let mut backoff_ms: u64 = 500;
    while running.load(Ordering::SeqCst) {
        let stream = match tokio::net::TcpStream::connect(addr).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("MAVLink TCP connect to {addr} failed: {e}");
                errors_count.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(30_000);
                continue;
            }
        };
        backoff_ms = 500;
        if let Err(e) = drive_tcp_stream(
            stream,
            &running,
            &events_count,
            &errors_count,
            &connector_id,
            &tx,
        )
        .await
        {
            tracing::warn!("MAVLink TCP stream ended: {e}");
            errors_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// TCP read loop. Buffers bytes, finds the next STX, attempts a parse, and on
/// failure drops one byte and retries — that lets us recover from a corrupted
/// prefix without losing the rest of the stream.
async fn drive_tcp_stream(
    mut stream: tokio::net::TcpStream,
    running: &Arc<AtomicBool>,
    events_count: &Arc<AtomicU64>,
    errors_count: &Arc<AtomicU64>,
    connector_id: &str,
    tx: &tokio::sync::mpsc::Sender<SourceEvent>,
) -> std::io::Result<()> {
    use tokio::io::AsyncReadExt;
    let mut buf: Vec<u8> = Vec::with_capacity(MAX_MAVLINK_FRAME * 4);
    let mut tmp = [0u8; 1024];
    while running.load(Ordering::SeqCst) {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);

        loop {
            let stx = match buf.iter().position(|b| *b == MAV_STX_V2) {
                Some(p) => p,
                None => {
                    buf.clear();
                    break;
                }
            };
            // V2 header is 10 bytes (incl STX) + payload + 2-byte CRC, min 12 total.
            if buf.len() - stx < 12 {
                if stx > 0 {
                    buf.drain(..stx);
                }
                break;
            }
            let frame_len = 12 + buf[stx + 1] as usize;
            if buf.len() - stx < frame_len {
                if stx > 0 {
                    buf.drain(..stx);
                }
                break;
            }
            match MavlinkConnector::decode_v2_datagram(&buf[stx..stx + frame_len]) {
                Some((header, msg)) => {
                    if let Some(event) =
                        MavlinkConnector::frame_to_event(connector_id, header, &msg, Utc::now())
                    {
                        if tx.send(event).await.is_err() {
                            running.store(false, Ordering::SeqCst);
                            return Ok(());
                        }
                        events_count.fetch_add(1, Ordering::Relaxed);
                    }
                    buf.drain(..stx + frame_len);
                }
                None => {
                    errors_count.fetch_add(1, Ordering::Relaxed);
                    buf.drain(..stx + 1);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavlink::common::{
        GpsFixType, MavAutopilot, MavModeFlag, MavState, MavType, ATTITUDE_DATA,
        GLOBAL_POSITION_INT_DATA, GPS_RAW_INT_DATA, HEARTBEAT_DATA, SYS_STATUS_DATA, VFR_HUD_DATA,
    };
    use mavlink::write_v2_msg;

    fn build_config(url: Option<&str>) -> ConnectorConfig {
        ConnectorConfig {
            connector_id: "mavlink-test".into(),
            connector_type: "mavlink".into(),
            url: url.map(str::to_string),
            entity_type: "drone".into(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        }
    }

    fn header(system_id: u8) -> MavHeader {
        MavHeader {
            system_id,
            component_id: 1,
            sequence: 0,
        }
    }

    fn encode_v2(hdr: MavHeader, msg: &MavMessage) -> Vec<u8> {
        let mut buf = Vec::with_capacity(MAX_MAVLINK_FRAME);
        write_v2_msg(&mut buf, hdr, msg).expect("encode v2 in test");
        buf
    }

    fn heartbeat_msg() -> MavMessage {
        MavMessage::HEARTBEAT(HEARTBEAT_DATA {
            custom_mode: 4,
            mavtype: MavType::MAV_TYPE_QUADROTOR,
            autopilot: MavAutopilot::MAV_AUTOPILOT_PX4,
            base_mode: MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED
                | MavModeFlag::MAV_MODE_FLAG_AUTO_ENABLED,
            system_status: MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        })
    }

    #[test]
    fn from_config_parses_udp_url() {
        let conn =
            MavlinkConnector::from_connector_config(build_config(Some("mavlink://0.0.0.0:14550")))
                .expect("valid url");
        match conn.endpoint() {
            Endpoint::Udp(addr) => {
                assert_eq!(addr.port(), 14550);
                assert_eq!(addr.ip().to_string(), "0.0.0.0");
            }
            other => panic!("expected udp endpoint, got {other:?}"),
        }
        assert_eq!(conn.connector_id(), "mavlink-test");
    }

    #[test]
    fn from_config_defaults_port_when_missing() {
        let conn = MavlinkConnector::from_connector_config(build_config(Some("mavlink://0.0.0.0")))
            .expect("valid url");
        assert_eq!(conn.endpoint(), &Endpoint::Udp(default_bind_addr()));
    }

    #[test]
    fn from_config_parses_tcp_url() {
        let conn = MavlinkConnector::from_connector_config(build_config(Some(
            "mavlinktcp://127.0.0.1:5760",
        )))
        .expect("valid url");
        assert_eq!(
            conn.endpoint(),
            &Endpoint::Tcp("127.0.0.1:5760".parse().unwrap())
        );
    }

    #[test]
    fn from_config_rejects_bad_scheme() {
        match MavlinkConnector::from_connector_config(build_config(Some("udp://0.0.0.0:14550"))) {
            Err(ConnectorError::ConfigError(_)) => {}
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn from_config_rejects_missing_url() {
        match MavlinkConnector::from_connector_config(build_config(None)) {
            Err(ConnectorError::ConfigError(_)) => {}
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn heartbeat_round_trips_into_source_event() {
        let bytes = encode_v2(header(7), &heartbeat_msg());
        let (h, m) = MavlinkConnector::decode_v2_datagram(&bytes).expect("decode");
        assert_eq!(h.system_id, 7);
        assert_eq!(h.component_id, 1);
        let event = MavlinkConnector::frame_to_event("mav", h, &m, Utc::now()).expect("event");
        assert_eq!(event.entity_id, "mav-7-1");
        assert_eq!(event.entity_type, "drone");
        assert_eq!(event.properties["message"], json!("HEARTBEAT"));
        assert_eq!(event.properties["system_id"], json!(7));
        assert!(event.properties["autopilot"]
            .as_str()
            .unwrap()
            .contains("PX4"));
        assert!(event.properties["vehicle_type"]
            .as_str()
            .unwrap()
            .contains("QUADROTOR"));
        assert_eq!(event.properties["custom_mode"], json!(4));
    }

    #[test]
    fn global_position_int_lat_lon_scale() {
        // 47.3977419°, 8.5455934°, alt 488.123m, hdg 92.5°
        let data = GLOBAL_POSITION_INT_DATA {
            time_boot_ms: 123_456,
            lat: 473_977_419,
            lon: 85_455_934,
            alt: 488_123,
            relative_alt: 12_345,
            vx: 250,
            vy: -100,
            vz: 50,
            hdg: 9250,
        };
        let bytes = encode_v2(header(1), &MavMessage::GLOBAL_POSITION_INT(data));
        let (h, m) = MavlinkConnector::decode_v2_datagram(&bytes).unwrap();
        let event = MavlinkConnector::frame_to_event("mav", h, &m, Utc::now()).unwrap();
        let lat = event.latitude.unwrap();
        let lon = event.longitude.unwrap();
        assert!((lat - 47.3977419).abs() < 1e-6, "lat = {lat}");
        assert!((lon - 8.5455934).abs() < 1e-6, "lon = {lon}");
        assert!((event.properties["alt_msl_m"].as_f64().unwrap() - 488.123).abs() < 1e-3);
        assert!((event.properties["alt_relative_m"].as_f64().unwrap() - 12.345).abs() < 1e-3);
        assert!((event.properties["vx_mps"].as_f64().unwrap() - 2.5).abs() < 1e-9);
        assert!((event.properties["heading_deg"].as_f64().unwrap() - 92.5).abs() < 1e-6);
        assert_eq!(event.entity_id, "mav-1-1");
    }

    #[test]
    fn vfr_hud_maps_speed_and_heading() {
        let data = VFR_HUD_DATA {
            airspeed: 12.5,
            groundspeed: 11.0,
            alt: 100.0,
            climb: 0.5,
            heading: 273,
            throttle: 65,
        };
        let bytes = encode_v2(header(2), &MavMessage::VFR_HUD(data));
        let (h, m) = MavlinkConnector::decode_v2_datagram(&bytes).unwrap();
        let event = MavlinkConnector::frame_to_event("mav", h, &m, Utc::now()).unwrap();
        assert_eq!(event.properties["airspeed_mps"], json!(12.5));
        assert_eq!(event.properties["groundspeed_mps"], json!(11.0));
        assert_eq!(event.properties["heading_deg"], json!(273));
        assert_eq!(event.properties["throttle_pct"], json!(65));
        assert!(event.latitude.is_none());
    }

    #[test]
    fn dedup_two_global_position_packets_share_entity_id() {
        let mk = |lat: i32, lon: i32, ms: u32| {
            MavMessage::GLOBAL_POSITION_INT(GLOBAL_POSITION_INT_DATA {
                time_boot_ms: ms,
                lat,
                lon,
                alt: 0,
                relative_alt: 0,
                vx: 0,
                vy: 0,
                vz: 0,
                hdg: 0,
            })
        };
        let b1 = encode_v2(header(1), &mk(473_977_419, 85_455_934, 1));
        let b2 = encode_v2(header(1), &mk(473_980_000, 85_460_000, 2));
        let (h1, m1) = MavlinkConnector::decode_v2_datagram(&b1).unwrap();
        let (h2, m2) = MavlinkConnector::decode_v2_datagram(&b2).unwrap();
        let e1 = MavlinkConnector::frame_to_event("mav", h1, &m1, Utc::now()).unwrap();
        let e2 = MavlinkConnector::frame_to_event("mav", h2, &m2, Utc::now()).unwrap();
        assert_eq!(e1.entity_id, e2.entity_id);
        assert_eq!(e1.entity_id, "mav-1-1");
    }

    #[test]
    fn different_system_ids_get_distinct_entity_ids() {
        let make = |sys| {
            let bytes = encode_v2(header(sys), &heartbeat_msg());
            let (h, m) = MavlinkConnector::decode_v2_datagram(&bytes).unwrap();
            MavlinkConnector::frame_to_event("mav", h, &m, Utc::now()).unwrap()
        };
        let a = make(1);
        let b = make(42);
        assert_ne!(a.entity_id, b.entity_id);
        assert_eq!(a.entity_id, "mav-1-1");
        assert_eq!(b.entity_id, "mav-42-1");
    }

    #[test]
    fn attitude_message_decodes_into_event() {
        let data = ATTITUDE_DATA {
            time_boot_ms: 5,
            roll: 0.1,
            pitch: -0.05,
            yaw: 1.57,
            rollspeed: 0.01,
            pitchspeed: -0.02,
            yawspeed: 0.03,
        };
        let bytes = encode_v2(header(1), &MavMessage::ATTITUDE(data));
        let (h, m) = MavlinkConnector::decode_v2_datagram(&bytes).unwrap();
        let event = MavlinkConnector::frame_to_event("mav", h, &m, Utc::now()).unwrap();
        assert_eq!(event.properties["message"], json!("ATTITUDE"));
        assert!((event.properties["roll_rad"].as_f64().unwrap() - 0.1).abs() < 1e-6);
        assert!((event.properties["yaw_rad"].as_f64().unwrap() - 1.57).abs() < 1e-6);
    }

    #[test]
    fn gps_raw_int_extracts_fix_and_satellites() {
        let data = GPS_RAW_INT_DATA {
            time_usec: 0,
            lat: 473_977_419,
            lon: 85_455_934,
            alt: 488_123,
            eph: 80,
            epv: 120,
            vel: 250,
            cog: 9250,
            fix_type: GpsFixType::GPS_FIX_TYPE_3D_FIX,
            satellites_visible: 14,
        };
        let bytes = encode_v2(header(3), &MavMessage::GPS_RAW_INT(data));
        let (h, m) = MavlinkConnector::decode_v2_datagram(&bytes).unwrap();
        let event = MavlinkConnector::frame_to_event("mav", h, &m, Utc::now()).unwrap();
        assert_eq!(event.properties["satellites_visible"], json!(14));
        assert!(event.properties["fix_type"]
            .as_str()
            .unwrap()
            .contains("3D_FIX"));
        assert!((event.properties["hdop"].as_f64().unwrap() - 0.80).abs() < 1e-6);
        assert!((event.properties["vdop"].as_f64().unwrap() - 1.20).abs() < 1e-6);
    }

    #[test]
    fn sys_status_extracts_battery_and_errors() {
        let data = SYS_STATUS_DATA {
            onboard_control_sensors_present: Default::default(),
            onboard_control_sensors_enabled: Default::default(),
            onboard_control_sensors_health: Default::default(),
            load: 250,
            voltage_battery: 12_450,
            current_battery: 1_500,
            drop_rate_comm: 5,
            errors_comm: 7,
            errors_count1: 0,
            errors_count2: 1,
            errors_count3: 2,
            errors_count4: 3,
            battery_remaining: 73,
        };
        let bytes = encode_v2(header(1), &MavMessage::SYS_STATUS(data));
        let (h, m) = MavlinkConnector::decode_v2_datagram(&bytes).unwrap();
        let event = MavlinkConnector::frame_to_event("mav", h, &m, Utc::now()).unwrap();
        assert!((event.properties["battery_voltage_v"].as_f64().unwrap() - 12.45).abs() < 1e-6);
        assert_eq!(event.properties["battery_remaining_pct"], json!(73));
        assert_eq!(event.properties["errors_comm"], json!(7));
        assert!((event.properties["battery_current_a"].as_f64().unwrap() - 15.00).abs() < 1e-6);
    }

    /// End-to-end UDP test: bind the connector on an ephemeral loopback port
    /// (so multiple test runs don't fight over 14550), shoot a HEARTBEAT at
    /// it, and verify both the channel and `stats()` reflect the event.
    #[tokio::test]
    async fn stats_increment_via_real_udp_socket() {
        // First grab a free port by binding-and-dropping an ephemeral socket.
        let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let bind_addr = probe.local_addr().unwrap();
        drop(probe);

        let url = format!("mavlink://{bind_addr}");
        let connector = MavlinkConnector::from_connector_config(build_config(Some(&url))).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<SourceEvent>(16);
        connector.start(tx).await.expect("start");

        // Tiny pause so the listener has actually called .bind().
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let bytes = encode_v2(header(9), &heartbeat_msg());
        client.send_to(&bytes, bind_addr).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("did not receive event in time")
            .expect("channel closed");
        assert_eq!(event.entity_id, "mav-9-1");
        assert!(connector.stats().events_processed >= 1);

        connector.stop().await.unwrap();
    }

    #[test]
    fn rejects_garbage_and_parses_trailing_slash() {
        assert!(MavlinkConnector::decode_v2_datagram(&[0, 0, 0]).is_none());
        assert!(MavlinkConnector::decode_v2_datagram(b"hello world").is_none());
        let ep = parse_endpoint(Some("mavlink://127.0.0.1:9000/")).unwrap();
        assert_eq!(ep, Endpoint::Udp("127.0.0.1:9000".parse().unwrap()));
    }
}
