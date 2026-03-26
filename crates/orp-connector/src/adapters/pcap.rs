use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// PCAP / PCAPNG packet capture file parser
// ---------------------------------------------------------------------------
// PCAP is the standard binary format produced by tcpdump, Wireshark, Zeek, etc.
//
// File layout:
//   Global Header (24 bytes)
//   Packet Record 1  (16-byte header + captured data)
//   Packet Record 2  …
//
// Global Header:
//   magic_number  : u32  (0xa1b2c3d4 = LE, 0xd4c3b2a1 = BE, 0xa1b23c4d = ns-LE)
//   version_major : u16
//   version_minor : u16
//   thiszone      : i32  (GMT-to-local correction — usually 0)
//   sigfigs       : u32  (accuracy of timestamps — usually 0)
//   snaplen       : u32  (max captured bytes per packet)
//   network       : u32  (link-layer type, 1 = Ethernet)
//
// Packet Record Header (16 bytes):
//   ts_sec   : u32  (UNIX epoch seconds)
//   ts_usec  : u32  (microseconds or nanoseconds depending on magic)
//   incl_len : u32  (captured bytes)
//   orig_len : u32  (original packet length on wire)
//
// Link-layer types:
//   1 = Ethernet (most common)
//   228 = Raw IPv4
//   229 = Raw IPv6
//   113 = Linux cooked (SLL)

/// PCAP byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcapEndian {
    Little,
    Big,
}

/// Timestamp resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TsResolution {
    Microseconds,
    Nanoseconds,
}

/// Parsed PCAP global header.
#[derive(Clone, Debug)]
pub struct PcapGlobalHeader {
    pub endian: PcapEndian,
    pub ts_resolution: TsResolution,
    pub version_major: u16,
    pub version_minor: u16,
    pub thiszone: i32,
    pub sigfigs: u32,
    pub snaplen: u32,
    pub network: u32,
}

/// Parsed PCAP packet record header.
#[derive(Clone, Debug)]
pub struct PcapPacketHeader {
    pub ts_sec: u32,
    pub ts_usec: u32,
    pub incl_len: u32,
    pub orig_len: u32,
}

/// Ethernet frame header (14 bytes).
#[derive(Clone, Debug)]
pub struct EthernetHeader {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub ethertype: u16,
}

/// IPv4 header (20+ bytes).
#[derive(Clone, Debug)]
pub struct Ipv4Header {
    pub version: u8,
    pub ihl: u8,
    pub dscp: u8,
    pub total_length: u16,
    pub identification: u16,
    pub flags: u8,
    pub fragment_offset: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub header_checksum: u16,
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
}

impl Ipv4Header {
    pub fn src_ip_str(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.src_ip[0], self.src_ip[1], self.src_ip[2], self.src_ip[3]
        )
    }

    pub fn dst_ip_str(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.dst_ip[0], self.dst_ip[1], self.dst_ip[2], self.dst_ip[3]
        )
    }

    /// IP protocol name.
    pub fn protocol_name(&self) -> &'static str {
        match self.protocol {
            1 => "ICMP",
            6 => "TCP",
            17 => "UDP",
            47 => "GRE",
            50 => "ESP",
            51 => "AH",
            58 => "ICMPv6",
            89 => "OSPF",
            132 => "SCTP",
            _ => "OTHER",
        }
    }

    /// Header length in bytes.
    pub fn header_len(&self) -> usize {
        (self.ihl as usize) * 4
    }
}

/// TCP header (20+ bytes).
#[derive(Clone, Debug)]
pub struct TcpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq_number: u32,
    pub ack_number: u32,
    pub data_offset: u8,
    pub flags: u8,
    pub window_size: u16,
    pub checksum: u16,
    pub urgent_pointer: u16,
}

impl TcpHeader {
    /// TCP flag mnemonics.
    pub fn flags_str(&self) -> String {
        let mut s = String::new();
        if self.flags & 0x01 != 0 { s.push_str("FIN "); }
        if self.flags & 0x02 != 0 { s.push_str("SYN "); }
        if self.flags & 0x04 != 0 { s.push_str("RST "); }
        if self.flags & 0x08 != 0 { s.push_str("PSH "); }
        if self.flags & 0x10 != 0 { s.push_str("ACK "); }
        if self.flags & 0x20 != 0 { s.push_str("URG "); }
        s.trim().to_string()
    }

    /// Header length in bytes.
    pub fn header_len(&self) -> usize {
        (self.data_offset as usize) * 4
    }
}

/// UDP header (8 bytes).
#[derive(Clone, Debug)]
pub struct UdpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub length: u16,
    pub checksum: u16,
}

/// Parsed network packet summary.
#[derive(Clone, Debug)]
pub struct PacketSummary {
    pub ts_sec: u32,
    pub ts_usec: u32,
    pub src_ip: String,
    pub dst_ip: String,
    pub src_port: Option<u16>,
    pub dst_port: Option<u16>,
    pub protocol: String,
    pub payload_size: u32,
    pub orig_len: u32,
    pub tcp_flags: Option<String>,
    pub ttl: u8,
}

// ---------------------------------------------------------------------------
// Helper: read u16/u32/i32 with endianness
// ---------------------------------------------------------------------------

fn read_u16(data: &[u8], offset: usize, endian: PcapEndian) -> Option<u16> {
    if offset + 2 > data.len() {
        return None;
    }
    let bytes: [u8; 2] = [data[offset], data[offset + 1]];
    Some(match endian {
        PcapEndian::Little => u16::from_le_bytes(bytes),
        PcapEndian::Big => u16::from_be_bytes(bytes),
    })
}

fn read_u32(data: &[u8], offset: usize, endian: PcapEndian) -> Option<u32> {
    if offset + 4 > data.len() {
        return None;
    }
    let bytes: [u8; 4] = [data[offset], data[offset + 1], data[offset + 2], data[offset + 3]];
    Some(match endian {
        PcapEndian::Little => u32::from_le_bytes(bytes),
        PcapEndian::Big => u32::from_be_bytes(bytes),
    })
}

fn read_i32(data: &[u8], offset: usize, endian: PcapEndian) -> Option<i32> {
    if offset + 4 > data.len() {
        return None;
    }
    let bytes: [u8; 4] = [data[offset], data[offset + 1], data[offset + 2], data[offset + 3]];
    Some(match endian {
        PcapEndian::Little => i32::from_le_bytes(bytes),
        PcapEndian::Big => i32::from_be_bytes(bytes),
    })
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse PCAP global header from raw bytes.
pub fn parse_pcap_global_header(data: &[u8]) -> Result<PcapGlobalHeader, ConnectorError> {
    if data.len() < 24 {
        return Err(ConnectorError::ParseError(
            "PCAP: global header too short (need 24 bytes)".into(),
        ));
    }

    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let (endian, ts_resolution) = match magic {
        0xa1b2c3d4 => (PcapEndian::Little, TsResolution::Microseconds),
        0xd4c3b2a1 => (PcapEndian::Big, TsResolution::Microseconds),
        0xa1b23c4d => (PcapEndian::Little, TsResolution::Nanoseconds),
        0x4d3cb2a1 => (PcapEndian::Big, TsResolution::Nanoseconds),
        0x0a0d0d0a => {
            return Err(ConnectorError::ParseError(
                "PCAP: PCAPNG format detected — not yet supported, use standard PCAP".into(),
            ));
        }
        _ => {
            return Err(ConnectorError::ParseError(format!(
                "PCAP: invalid magic number 0x{:08x}",
                magic
            )));
        }
    };

    Ok(PcapGlobalHeader {
        endian,
        ts_resolution,
        version_major: read_u16(data, 4, endian)
            .ok_or_else(|| ConnectorError::ParseError("PCAP: global header truncated at version_major".into()))?,
        version_minor: read_u16(data, 6, endian)
            .ok_or_else(|| ConnectorError::ParseError("PCAP: global header truncated at version_minor".into()))?,
        thiszone: read_i32(data, 8, endian)
            .ok_or_else(|| ConnectorError::ParseError("PCAP: global header truncated at thiszone".into()))?,
        sigfigs: read_u32(data, 12, endian)
            .ok_or_else(|| ConnectorError::ParseError("PCAP: global header truncated at sigfigs".into()))?,
        snaplen: read_u32(data, 16, endian)
            .ok_or_else(|| ConnectorError::ParseError("PCAP: global header truncated at snaplen".into()))?,
        network: read_u32(data, 20, endian)
            .ok_or_else(|| ConnectorError::ParseError("PCAP: global header truncated at network".into()))?,
    })
}

/// Parse a PCAP packet record header at a given offset.
pub fn parse_pcap_packet_header(
    data: &[u8],
    offset: usize,
    endian: PcapEndian,
) -> Result<(PcapPacketHeader, usize), ConnectorError> {
    if offset + 16 > data.len() {
        return Err(ConnectorError::ParseError(
            "PCAP: packet header truncated".into(),
        ));
    }
    let ts_sec = read_u32(data, offset, endian)
        .ok_or_else(|| ConnectorError::ParseError("PCAP: packet header truncated at ts_sec".into()))?;
    let ts_usec = read_u32(data, offset + 4, endian)
        .ok_or_else(|| ConnectorError::ParseError("PCAP: packet header truncated at ts_usec".into()))?;
    let incl_len = read_u32(data, offset + 8, endian)
        .ok_or_else(|| ConnectorError::ParseError("PCAP: packet header truncated at incl_len".into()))?;
    let orig_len = read_u32(data, offset + 12, endian)
        .ok_or_else(|| ConnectorError::ParseError("PCAP: packet header truncated at orig_len".into()))?;
    let next = offset + 16 + incl_len as usize;
    if next > data.len() {
        return Err(ConnectorError::ParseError(
            "PCAP: packet data truncated".into(),
        ));
    }
    Ok((
        PcapPacketHeader {
            ts_sec,
            ts_usec,
            incl_len,
            orig_len,
        },
        next,
    ))
}

/// Parse Ethernet frame header.
pub fn parse_ethernet_header(data: &[u8]) -> Option<EthernetHeader> {
    if data.len() < 14 {
        return None;
    }
    let mut dst_mac = [0u8; 6];
    let mut src_mac = [0u8; 6];
    dst_mac.copy_from_slice(&data[0..6]);
    src_mac.copy_from_slice(&data[6..12]);
    let ethertype = u16::from_be_bytes([data[12], data[13]]);
    Some(EthernetHeader {
        dst_mac,
        src_mac,
        ethertype,
    })
}

/// Parse IPv4 header.
pub fn parse_ipv4_header(data: &[u8]) -> Option<Ipv4Header> {
    if data.len() < 20 {
        return None;
    }
    let version = (data[0] >> 4) & 0x0F;
    let ihl = data[0] & 0x0F;
    if version != 4 || ihl < 5 {
        return None;
    }
    let total_length = u16::from_be_bytes([data[2], data[3]]);
    let identification = u16::from_be_bytes([data[4], data[5]]);
    let flags = (data[6] >> 5) & 0x07;
    let fragment_offset = u16::from_be_bytes([data[6] & 0x1F, data[7]]);
    let header_checksum = u16::from_be_bytes([data[10], data[11]]);
    let mut src_ip = [0u8; 4];
    let mut dst_ip = [0u8; 4];
    src_ip.copy_from_slice(&data[12..16]);
    dst_ip.copy_from_slice(&data[16..20]);

    Some(Ipv4Header {
        version,
        ihl,
        dscp: data[1],
        total_length,
        identification,
        flags,
        fragment_offset,
        ttl: data[8],
        protocol: data[9],
        header_checksum,
        src_ip,
        dst_ip,
    })
}

/// Parse TCP header.
pub fn parse_tcp_header(data: &[u8]) -> Option<TcpHeader> {
    if data.len() < 20 {
        return None;
    }
    let src_port = u16::from_be_bytes([data[0], data[1]]);
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    let seq_number = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ack_number = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let data_offset = (data[12] >> 4) & 0x0F;
    let flags = data[13] & 0x3F;
    let window_size = u16::from_be_bytes([data[14], data[15]]);
    let checksum = u16::from_be_bytes([data[16], data[17]]);
    let urgent_pointer = u16::from_be_bytes([data[18], data[19]]);

    Some(TcpHeader {
        src_port,
        dst_port,
        seq_number,
        ack_number,
        data_offset,
        flags,
        window_size,
        checksum,
        urgent_pointer,
    })
}

/// Parse UDP header.
pub fn parse_udp_header(data: &[u8]) -> Option<UdpHeader> {
    if data.len() < 8 {
        return None;
    }
    Some(UdpHeader {
        src_port: u16::from_be_bytes([data[0], data[1]]),
        dst_port: u16::from_be_bytes([data[2], data[3]]),
        length: u16::from_be_bytes([data[4], data[5]]),
        checksum: u16::from_be_bytes([data[6], data[7]]),
    })
}

/// Parse a single Ethernet packet into a `PacketSummary`.
/// Handles Ethernet → IPv4 → TCP/UDP dissection.
pub fn parse_packet(
    pkt_header: &PcapPacketHeader,
    pkt_data: &[u8],
    link_type: u32,
) -> Option<PacketSummary> {
    let ip_data = match link_type {
        1 => {
            // Ethernet
            let eth = parse_ethernet_header(pkt_data)?;
            // Handle 802.1Q VLAN tagging
            let (ethertype, offset) = if eth.ethertype == 0x8100 && pkt_data.len() >= 18 {
                let real_ethertype = u16::from_be_bytes([pkt_data[16], pkt_data[17]]);
                (real_ethertype, 18)
            } else {
                (eth.ethertype, 14)
            };
            if ethertype != 0x0800 {
                // Not IPv4 — skip (could be ARP 0x0806, IPv6 0x86DD, etc.)
                return None;
            }
            &pkt_data[offset..]
        }
        228 => {
            // Raw IPv4
            pkt_data
        }
        _ => return None,
    };

    let ipv4 = parse_ipv4_header(ip_data)?;
    let transport_offset = ipv4.header_len();
    let transport_data = ip_data.get(transport_offset..)?;
    let ip_payload_len = (ipv4.total_length as usize).saturating_sub(transport_offset);

    match ipv4.protocol {
        6 => {
            // TCP
            let tcp = parse_tcp_header(transport_data)?;
            let tcp_hdr_len = tcp.header_len();
            let payload_size = ip_payload_len.saturating_sub(tcp_hdr_len);
            Some(PacketSummary {
                ts_sec: pkt_header.ts_sec,
                ts_usec: pkt_header.ts_usec,
                src_ip: ipv4.src_ip_str(),
                dst_ip: ipv4.dst_ip_str(),
                src_port: Some(tcp.src_port),
                dst_port: Some(tcp.dst_port),
                protocol: "TCP".into(),
                payload_size: payload_size as u32,
                orig_len: pkt_header.orig_len,
                tcp_flags: Some(tcp.flags_str()),
                ttl: ipv4.ttl,
            })
        }
        17 => {
            // UDP
            let udp = parse_udp_header(transport_data)?;
            let payload_size = (udp.length as usize).saturating_sub(8);
            Some(PacketSummary {
                ts_sec: pkt_header.ts_sec,
                ts_usec: pkt_header.ts_usec,
                src_ip: ipv4.src_ip_str(),
                dst_ip: ipv4.dst_ip_str(),
                src_port: Some(udp.src_port),
                dst_port: Some(udp.dst_port),
                protocol: "UDP".into(),
                payload_size: payload_size as u32,
                orig_len: pkt_header.orig_len,
                tcp_flags: None,
                ttl: ipv4.ttl,
            })
        }
        1 => {
            // ICMP
            Some(PacketSummary {
                ts_sec: pkt_header.ts_sec,
                ts_usec: pkt_header.ts_usec,
                src_ip: ipv4.src_ip_str(),
                dst_ip: ipv4.dst_ip_str(),
                src_port: None,
                dst_port: None,
                protocol: "ICMP".into(),
                payload_size: ip_payload_len as u32,
                orig_len: pkt_header.orig_len,
                tcp_flags: None,
                ttl: ipv4.ttl,
            })
        }
        _ => {
            // Other IP protocol
            Some(PacketSummary {
                ts_sec: pkt_header.ts_sec,
                ts_usec: pkt_header.ts_usec,
                src_ip: ipv4.src_ip_str(),
                dst_ip: ipv4.dst_ip_str(),
                src_port: None,
                dst_port: None,
                protocol: ipv4.protocol_name().to_string(),
                payload_size: ip_payload_len as u32,
                orig_len: pkt_header.orig_len,
                tcp_flags: None,
                ttl: ipv4.ttl,
            })
        }
    }
}

/// Convert a `PacketSummary` into a `SourceEvent`.
pub fn packet_to_source_event(
    summary: &PacketSummary,
    connector_id: &str,
) -> SourceEvent {
    let entity_id = match (&summary.src_port, &summary.dst_port) {
        (Some(sp), Some(dp)) => {
            format!("pcap:{}:{}-{}:{}", summary.src_ip, sp, summary.dst_ip, dp)
        }
        _ => format!("pcap:{}-{}", summary.src_ip, summary.dst_ip),
    };

    let ts = DateTime::from_timestamp(summary.ts_sec as i64, summary.ts_usec * 1000)
        .unwrap_or_else(Utc::now);

    let mut properties = HashMap::new();
    properties.insert("src_ip".into(), json!(summary.src_ip));
    properties.insert("dst_ip".into(), json!(summary.dst_ip));
    properties.insert("protocol".into(), json!(summary.protocol));
    properties.insert("payload_size".into(), json!(summary.payload_size));
    properties.insert("orig_len".into(), json!(summary.orig_len));
    properties.insert("ttl".into(), json!(summary.ttl));

    if let Some(sp) = summary.src_port {
        properties.insert("src_port".into(), json!(sp));
    }
    if let Some(dp) = summary.dst_port {
        properties.insert("dst_port".into(), json!(dp));
    }
    if let Some(ref flags) = summary.tcp_flags {
        properties.insert("tcp_flags".into(), json!(flags));
    }

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "network_event".into(),
        properties,
        timestamp: ts,
        latitude: None,
        longitude: None,
    }
}

/// Parse a complete PCAP file and return all packet summaries.
pub fn parse_pcap_file(data: &[u8]) -> Result<Vec<PacketSummary>, ConnectorError> {
    let header = parse_pcap_global_header(data)?;
    let mut offset = 24usize;
    let mut summaries = Vec::new();

    while offset < data.len() {
        let (pkt_hdr, next_offset) = match parse_pcap_packet_header(data, offset, header.endian) {
            Ok(v) => v,
            Err(_) => break,
        };
        let pkt_data = &data[offset + 16..offset + 16 + pkt_hdr.incl_len as usize];
        if let Some(summary) = parse_packet(&pkt_hdr, pkt_data, header.network) {
            summaries.push(summary);
        }
        offset = next_offset;
    }

    Ok(summaries)
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct PcapConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl PcapConnector {
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
impl Connector for PcapConnector {
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
            .ok_or_else(|| ConnectorError::ConfigError("PCAP: url (file path) required".into()))?;

        let data = tokio::fs::read(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        let summaries = parse_pcap_file(&data).inspect_err(|_e| {
            errors.fetch_add(1, Ordering::Relaxed);
        })?;

        for summary in &summaries {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            let event = packet_to_source_event(summary, &connector_id);
            if tx.send(event).await.is_err() {
                break;
            }
            events_processed.fetch_add(1, Ordering::Relaxed);
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
                "PCAP connector is not running".into(),
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

    /// Build a minimal PCAP file in little-endian with the given packets.
    fn build_pcap_le(link_type: u32, packets: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        // Global header
        buf.extend_from_slice(&0xa1b2c3d4u32.to_le_bytes()); // magic
        buf.extend_from_slice(&2u16.to_le_bytes()); // version major
        buf.extend_from_slice(&4u16.to_le_bytes()); // version minor
        buf.extend_from_slice(&0i32.to_le_bytes()); // thiszone
        buf.extend_from_slice(&0u32.to_le_bytes()); // sigfigs
        buf.extend_from_slice(&65535u32.to_le_bytes()); // snaplen
        buf.extend_from_slice(&link_type.to_le_bytes()); // network

        for pkt in packets {
            let ts_sec: u32 = 1700000000;
            let ts_usec: u32 = 123456;
            buf.extend_from_slice(&ts_sec.to_le_bytes());
            buf.extend_from_slice(&ts_usec.to_le_bytes());
            buf.extend_from_slice(&(pkt.len() as u32).to_le_bytes()); // incl_len
            buf.extend_from_slice(&(pkt.len() as u32).to_le_bytes()); // orig_len
            buf.extend_from_slice(pkt);
        }
        buf
    }

    /// Build a minimal Ethernet + IPv4 + TCP packet.
    fn build_eth_tcp_packet(
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        src_port: u16,
        dst_port: u16,
        payload_len: usize,
    ) -> Vec<u8> {
        let mut pkt = Vec::new();
        // Ethernet header (14 bytes)
        pkt.extend_from_slice(&[0xaa; 6]); // dst mac
        pkt.extend_from_slice(&[0xbb; 6]); // src mac
        pkt.extend_from_slice(&[0x08, 0x00]); // ethertype IPv4

        let ip_total_len = 20 + 20 + payload_len;
        // IPv4 header (20 bytes)
        pkt.push(0x45); // version=4, ihl=5
        pkt.push(0x00); // DSCP
        pkt.extend_from_slice(&(ip_total_len as u16).to_be_bytes()); // total length
        pkt.extend_from_slice(&[0x00, 0x01]); // identification
        pkt.extend_from_slice(&[0x00, 0x00]); // flags + frag offset
        pkt.push(64); // TTL
        pkt.push(6); // protocol = TCP
        pkt.extend_from_slice(&[0x00, 0x00]); // checksum (fake)
        pkt.extend_from_slice(&src_ip);
        pkt.extend_from_slice(&dst_ip);

        // TCP header (20 bytes)
        pkt.extend_from_slice(&src_port.to_be_bytes());
        pkt.extend_from_slice(&dst_port.to_be_bytes());
        pkt.extend_from_slice(&0u32.to_be_bytes()); // seq
        pkt.extend_from_slice(&0u32.to_be_bytes()); // ack
        pkt.push(0x50); // data_offset=5 (20 bytes)
        pkt.push(0x02); // flags = SYN
        pkt.extend_from_slice(&8192u16.to_be_bytes()); // window
        pkt.extend_from_slice(&[0x00, 0x00]); // checksum
        pkt.extend_from_slice(&[0x00, 0x00]); // urgent

        // Payload
        pkt.extend_from_slice(&vec![0x41; payload_len]);
        pkt
    }

    /// Build a minimal Ethernet + IPv4 + UDP packet.
    fn build_eth_udp_packet(
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        src_port: u16,
        dst_port: u16,
        payload_len: usize,
    ) -> Vec<u8> {
        let mut pkt = Vec::new();
        // Ethernet header
        pkt.extend_from_slice(&[0xaa; 6]);
        pkt.extend_from_slice(&[0xbb; 6]);
        pkt.extend_from_slice(&[0x08, 0x00]);

        let udp_len = 8 + payload_len;
        let ip_total_len = 20 + udp_len;
        // IPv4 header
        pkt.push(0x45);
        pkt.push(0x00);
        pkt.extend_from_slice(&(ip_total_len as u16).to_be_bytes());
        pkt.extend_from_slice(&[0x00, 0x02]);
        pkt.extend_from_slice(&[0x00, 0x00]);
        pkt.push(128); // TTL
        pkt.push(17); // protocol = UDP
        pkt.extend_from_slice(&[0x00, 0x00]);
        pkt.extend_from_slice(&src_ip);
        pkt.extend_from_slice(&dst_ip);

        // UDP header
        pkt.extend_from_slice(&src_port.to_be_bytes());
        pkt.extend_from_slice(&dst_port.to_be_bytes());
        pkt.extend_from_slice(&(udp_len as u16).to_be_bytes());
        pkt.extend_from_slice(&[0x00, 0x00]); // checksum

        pkt.extend_from_slice(&vec![0x42; payload_len]);
        pkt
    }

    #[test]
    fn test_parse_global_header_le() {
        let data = build_pcap_le(1, &[]);
        let hdr = parse_pcap_global_header(&data).unwrap();
        assert_eq!(hdr.endian, PcapEndian::Little);
        assert_eq!(hdr.ts_resolution, TsResolution::Microseconds);
        assert_eq!(hdr.version_major, 2);
        assert_eq!(hdr.version_minor, 4);
        assert_eq!(hdr.network, 1);
    }

    #[test]
    fn test_parse_global_header_be() {
        let mut data = Vec::new();
        data.extend_from_slice(&0xd4c3b2a1u32.to_le_bytes()); // BE magic stored as raw bytes
        data.extend_from_slice(&2u16.to_be_bytes());
        data.extend_from_slice(&4u16.to_be_bytes());
        data.extend_from_slice(&0i32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&65535u32.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());

        let hdr = parse_pcap_global_header(&data).unwrap();
        assert_eq!(hdr.endian, PcapEndian::Big);
        assert_eq!(hdr.version_major, 2);
        assert_eq!(hdr.network, 1);
    }

    #[test]
    fn test_parse_global_header_too_short() {
        let data = vec![0u8; 10];
        assert!(parse_pcap_global_header(&data).is_err());
    }

    #[test]
    fn test_pcapng_detected() {
        let mut data = vec![0u8; 24];
        data[0] = 0x0a;
        data[1] = 0x0d;
        data[2] = 0x0d;
        data[3] = 0x0a;
        let err = parse_pcap_global_header(&data).unwrap_err();
        assert!(err.to_string().contains("PCAPNG"));
    }

    #[test]
    fn test_parse_ethernet_header() {
        let data = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, // dst mac
            0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, // src mac
            0x08, 0x00, // ethertype IPv4
        ];
        let eth = parse_ethernet_header(&data).unwrap();
        assert_eq!(eth.ethertype, 0x0800);
        assert_eq!(eth.dst_mac, [0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
    }

    #[test]
    fn test_parse_ipv4_header() {
        let mut data = vec![0u8; 20];
        data[0] = 0x45; // version=4, ihl=5
        data[2..4].copy_from_slice(&60u16.to_be_bytes()); // total_length
        data[8] = 64; // TTL
        data[9] = 6; // TCP
        data[12..16].copy_from_slice(&[192, 168, 1, 100]);
        data[16..20].copy_from_slice(&[10, 0, 0, 1]);

        let ipv4 = parse_ipv4_header(&data).unwrap();
        assert_eq!(ipv4.src_ip_str(), "192.168.1.100");
        assert_eq!(ipv4.dst_ip_str(), "10.0.0.1");
        assert_eq!(ipv4.protocol, 6);
        assert_eq!(ipv4.ttl, 64);
        assert_eq!(ipv4.header_len(), 20);
    }

    #[test]
    fn test_parse_tcp_header() {
        let mut data = vec![0u8; 20];
        data[0..2].copy_from_slice(&8080u16.to_be_bytes());
        data[2..4].copy_from_slice(&443u16.to_be_bytes());
        data[12] = 0x50; // data_offset = 5
        data[13] = 0x12; // SYN + ACK

        let tcp = parse_tcp_header(&data).unwrap();
        assert_eq!(tcp.src_port, 8080);
        assert_eq!(tcp.dst_port, 443);
        assert!(tcp.flags_str().contains("SYN"));
        assert!(tcp.flags_str().contains("ACK"));
    }

    #[test]
    fn test_parse_udp_header() {
        let mut data = vec![0u8; 8];
        data[0..2].copy_from_slice(&53u16.to_be_bytes());
        data[2..4].copy_from_slice(&12345u16.to_be_bytes());
        data[4..6].copy_from_slice(&100u16.to_be_bytes());

        let udp = parse_udp_header(&data).unwrap();
        assert_eq!(udp.src_port, 53);
        assert_eq!(udp.dst_port, 12345);
        assert_eq!(udp.length, 100);
    }

    #[test]
    fn test_full_tcp_packet_parse() {
        let pkt = build_eth_tcp_packet([192, 168, 1, 1], [10, 0, 0, 1], 12345, 80, 100);
        let pcap_data = build_pcap_le(1, &[&pkt]);
        let summaries = parse_pcap_file(&pcap_data).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].src_ip, "192.168.1.1");
        assert_eq!(summaries[0].dst_ip, "10.0.0.1");
        assert_eq!(summaries[0].src_port, Some(12345));
        assert_eq!(summaries[0].dst_port, Some(80));
        assert_eq!(summaries[0].protocol, "TCP");
        assert_eq!(summaries[0].payload_size, 100);
    }

    #[test]
    fn test_full_udp_packet_parse() {
        let pkt = build_eth_udp_packet([10, 1, 2, 3], [8, 8, 8, 8], 55555, 53, 64);
        let pcap_data = build_pcap_le(1, &[&pkt]);
        let summaries = parse_pcap_file(&pcap_data).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].src_ip, "10.1.2.3");
        assert_eq!(summaries[0].protocol, "UDP");
        assert_eq!(summaries[0].payload_size, 64);
    }

    #[test]
    fn test_multiple_packets() {
        let pkt1 = build_eth_tcp_packet([1, 2, 3, 4], [5, 6, 7, 8], 100, 200, 50);
        let pkt2 = build_eth_udp_packet([9, 10, 11, 12], [13, 14, 15, 16], 300, 400, 30);
        let pcap_data = build_pcap_le(1, &[&pkt1, &pkt2]);
        let summaries = parse_pcap_file(&pcap_data).unwrap();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].protocol, "TCP");
        assert_eq!(summaries[1].protocol, "UDP");
    }

    #[test]
    fn test_non_ip_packet_skipped() {
        // ARP packet (ethertype 0x0806)
        let mut pkt = vec![0u8; 42];
        pkt[12] = 0x08;
        pkt[13] = 0x06; // ARP
        let pcap_data = build_pcap_le(1, &[&pkt]);
        let summaries = parse_pcap_file(&pcap_data).unwrap();
        assert_eq!(summaries.len(), 0);
    }

    #[test]
    fn test_packet_to_source_event() {
        let summary = PacketSummary {
            ts_sec: 1700000000,
            ts_usec: 0,
            src_ip: "192.168.1.1".into(),
            dst_ip: "10.0.0.1".into(),
            src_port: Some(12345),
            dst_port: Some(80),
            protocol: "TCP".into(),
            payload_size: 1500,
            orig_len: 1560,
            tcp_flags: Some("SYN".into()),
            ttl: 64,
        };
        let event = packet_to_source_event(&summary, "pcap-test");
        assert_eq!(event.entity_type, "network_event");
        assert_eq!(event.entity_id, "pcap:192.168.1.1:12345-10.0.0.1:80");
        assert_eq!(event.properties["src_ip"], json!("192.168.1.1"));
        assert_eq!(event.properties["protocol"], json!("TCP"));
        assert_eq!(event.properties["payload_size"], json!(1500));
        assert_eq!(event.properties["tcp_flags"], json!("SYN"));
    }

    #[test]
    fn test_pcap_connector_id() {
        let config = ConnectorConfig {
            connector_id: "pcap-1".to_string(),
            connector_type: "pcap".to_string(),
            url: None,
            entity_type: "network_event".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = PcapConnector::new(config);
        assert_eq!(connector.connector_id(), "pcap-1");
    }

    #[tokio::test]
    async fn test_pcap_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "pcap-health".to_string(),
            connector_type: "pcap".to_string(),
            url: None,
            entity_type: "network_event".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = PcapConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_nanosecond_magic() {
        let mut data = Vec::new();
        data.extend_from_slice(&0xa1b23c4du32.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&65535u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());

        let hdr = parse_pcap_global_header(&data).unwrap();
        assert_eq!(hdr.ts_resolution, TsResolution::Nanoseconds);
    }

    #[test]
    fn test_raw_ipv4_link_type() {
        // Build a raw IPv4 + TCP packet (no Ethernet header)
        let mut pkt = Vec::new();
        let ip_total_len = 20 + 20 + 10;
        pkt.push(0x45); // version=4, ihl=5
        pkt.push(0x00);
        pkt.extend_from_slice(&(ip_total_len as u16).to_be_bytes());
        pkt.extend_from_slice(&[0x00, 0x01]);
        pkt.extend_from_slice(&[0x00, 0x00]);
        pkt.push(64);
        pkt.push(6); // TCP
        pkt.extend_from_slice(&[0x00, 0x00]);
        pkt.extend_from_slice(&[172, 16, 0, 1]);
        pkt.extend_from_slice(&[172, 16, 0, 2]);
        // TCP header
        pkt.extend_from_slice(&1000u16.to_be_bytes());
        pkt.extend_from_slice(&2000u16.to_be_bytes());
        pkt.extend_from_slice(&0u32.to_be_bytes());
        pkt.extend_from_slice(&0u32.to_be_bytes());
        pkt.push(0x50);
        pkt.push(0x10); // ACK
        pkt.extend_from_slice(&8192u16.to_be_bytes());
        pkt.extend_from_slice(&[0x00, 0x00]);
        pkt.extend_from_slice(&[0x00, 0x00]);
        pkt.extend_from_slice(&[0x00; 10]);

        let pcap_data = build_pcap_le(228, &[&pkt]); // link type 228 = Raw IPv4
        let summaries = parse_pcap_file(&pcap_data).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].src_ip, "172.16.0.1");
        assert_eq!(summaries[0].protocol, "TCP");
    }
}
