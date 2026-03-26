use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// NetFlow v5 / v9 / IPFIX parser
// ---------------------------------------------------------------------------
// NetFlow is a Cisco-originated protocol for IP traffic flow analysis.
//
// Versions:
//   v5  — fixed-format, 48-byte flow records, most widely deployed
//   v9  — template-based, variable-format, superset of v5
//   IPFIX (v10) — IETF standard based on v9 (RFC 7011)
//
// Transport: UDP datagrams (commonly port 2055, 9995, or 9996)
//
// NetFlow v5 header (24 bytes):
//   version    : u16  (5)
//   count      : u16  (number of flow records, 1–30)
//   sys_uptime : u32  (milliseconds since device boot)
//   unix_secs  : u32  (epoch seconds)
//   unix_nsecs : u32  (residual nanoseconds)
//   flow_seq   : u32  (sequence counter)
//   engine_type: u8
//   engine_id  : u8
//   sampling   : u16
//
// NetFlow v5 flow record (48 bytes):
//   src_addr   : [u8;4]  (source IP)
//   dst_addr   : [u8;4]  (destination IP)
//   next_hop   : [u8;4]
//   snmp_input : u16
//   snmp_output: u16
//   packets    : u32
//   octets     : u32
//   first      : u32  (sys_uptime at flow start)
//   last       : u32  (sys_uptime at flow end)
//   src_port   : u16
//   dst_port   : u16
//   pad1       : u8
//   tcp_flags  : u8
//   protocol   : u8
//   tos        : u8
//   src_as     : u16
//   dst_as     : u16
//   src_mask   : u8
//   dst_mask   : u8
//   pad2       : u16
//
// NetFlow v9 header (20 bytes):
//   version    : u16  (9)
//   count      : u16  (number of FlowSets)
//   sys_uptime : u32
//   unix_secs  : u32
//   seq_number : u32
//   source_id  : u32

/// NetFlow packet version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetFlowVersion {
    V5,
    V9,
    Ipfix,
}

/// Parsed NetFlow v5 header.
#[derive(Clone, Debug)]
pub struct NetFlowV5Header {
    pub version: u16,
    pub count: u16,
    pub sys_uptime: u32,
    pub unix_secs: u32,
    pub unix_nsecs: u32,
    pub flow_sequence: u32,
    pub engine_type: u8,
    pub engine_id: u8,
    pub sampling_interval: u16,
}

/// Parsed NetFlow v5 flow record.
#[derive(Clone, Debug)]
pub struct NetFlowV5Record {
    pub src_addr: [u8; 4],
    pub dst_addr: [u8; 4],
    pub next_hop: [u8; 4],
    pub snmp_input: u16,
    pub snmp_output: u16,
    pub packets: u32,
    pub octets: u32,
    pub first: u32,
    pub last: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub tcp_flags: u8,
    pub protocol: u8,
    pub tos: u8,
    pub src_as: u16,
    pub dst_as: u16,
    pub src_mask: u8,
    pub dst_mask: u8,
}

impl NetFlowV5Record {
    pub fn src_addr_str(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.src_addr[0], self.src_addr[1], self.src_addr[2], self.src_addr[3]
        )
    }

    pub fn dst_addr_str(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.dst_addr[0], self.dst_addr[1], self.dst_addr[2], self.dst_addr[3]
        )
    }

    pub fn next_hop_str(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.next_hop[0], self.next_hop[1], self.next_hop[2], self.next_hop[3]
        )
    }

    pub fn protocol_name(&self) -> &'static str {
        match self.protocol {
            1 => "ICMP",
            6 => "TCP",
            17 => "UDP",
            47 => "GRE",
            50 => "ESP",
            89 => "OSPF",
            132 => "SCTP",
            _ => "OTHER",
        }
    }

    pub fn tcp_flags_str(&self) -> String {
        let mut s = String::new();
        if self.tcp_flags & 0x01 != 0 { s.push_str("FIN "); }
        if self.tcp_flags & 0x02 != 0 { s.push_str("SYN "); }
        if self.tcp_flags & 0x04 != 0 { s.push_str("RST "); }
        if self.tcp_flags & 0x08 != 0 { s.push_str("PSH "); }
        if self.tcp_flags & 0x10 != 0 { s.push_str("ACK "); }
        if self.tcp_flags & 0x20 != 0 { s.push_str("URG "); }
        s.trim().to_string()
    }

    /// Duration in milliseconds (from sys_uptime first → last).
    pub fn duration_ms(&self) -> u32 {
        self.last.saturating_sub(self.first)
    }
}

/// Parsed NetFlow v9 header.
#[derive(Clone, Debug)]
pub struct NetFlowV9Header {
    pub version: u16,
    pub count: u16,
    pub sys_uptime: u32,
    pub unix_secs: u32,
    pub sequence_number: u32,
    pub source_id: u32,
}

/// NetFlow v9 FlowSet header.
#[derive(Clone, Debug)]
pub struct FlowSetHeader {
    pub flowset_id: u16,
    pub length: u16,
}

/// NetFlow v9 template record — defines field layout for data FlowSets.
#[derive(Clone, Debug)]
pub struct V9Template {
    pub template_id: u16,
    pub field_count: u16,
    pub fields: Vec<V9FieldSpec>,
}

/// NetFlow v9 field specifier.
#[derive(Clone, Debug)]
pub struct V9FieldSpec {
    pub field_type: u16,
    pub field_length: u16,
}

/// A generic flow record parsed from NetFlow v9 using a template.
#[derive(Clone, Debug)]
pub struct GenericFlowRecord {
    pub fields: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Well-known NetFlow v9 / IPFIX field type IDs
// ---------------------------------------------------------------------------

fn v9_field_name(field_type: u16) -> &'static str {
    match field_type {
        1 => "in_bytes",
        2 => "in_pkts",
        3 => "flows",
        4 => "protocol",
        5 => "src_tos",
        6 => "tcp_flags",
        7 => "l4_src_port",
        8 => "ipv4_src_addr",
        10 => "input_snmp",
        11 => "l4_dst_port",
        12 => "ipv4_dst_addr",
        14 => "output_snmp",
        15 => "ipv4_next_hop",
        16 => "src_as",
        17 => "dst_as",
        21 => "last_switched",
        22 => "first_switched",
        23 => "out_bytes",
        24 => "out_pkts",
        32 => "icmp_type",
        56 => "in_src_mac",
        80 => "in_dst_mac",
        136 => "flow_end_reason",
        152 => "flow_start_milliseconds",
        153 => "flow_end_milliseconds",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Detect NetFlow version from the first 2 bytes.
pub fn detect_version(data: &[u8]) -> Result<NetFlowVersion, ConnectorError> {
    if data.len() < 2 {
        return Err(ConnectorError::ParseError(
            "NetFlow: packet too short to detect version".into(),
        ));
    }
    let version = u16::from_be_bytes([data[0], data[1]]);
    match version {
        5 => Ok(NetFlowVersion::V5),
        9 => Ok(NetFlowVersion::V9),
        10 => Ok(NetFlowVersion::Ipfix),
        _ => Err(ConnectorError::ParseError(format!(
            "NetFlow: unsupported version {}",
            version
        ))),
    }
}

/// Parse NetFlow v5 header.
pub fn parse_v5_header(data: &[u8]) -> Result<NetFlowV5Header, ConnectorError> {
    if data.len() < 24 {
        return Err(ConnectorError::ParseError(
            "NetFlow v5: header too short (need 24 bytes)".into(),
        ));
    }
    Ok(NetFlowV5Header {
        version: u16::from_be_bytes([data[0], data[1]]),
        count: u16::from_be_bytes([data[2], data[3]]),
        sys_uptime: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
        unix_secs: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        unix_nsecs: u32::from_be_bytes([data[12], data[13], data[14], data[15]]),
        flow_sequence: u32::from_be_bytes([data[16], data[17], data[18], data[19]]),
        engine_type: data[20],
        engine_id: data[21],
        sampling_interval: u16::from_be_bytes([data[22], data[23]]),
    })
}

/// Parse a single NetFlow v5 flow record at offset.
pub fn parse_v5_record(data: &[u8], offset: usize) -> Result<NetFlowV5Record, ConnectorError> {
    if offset + 48 > data.len() {
        return Err(ConnectorError::ParseError(
            "NetFlow v5: record truncated".into(),
        ));
    }
    let d = &data[offset..];
    let mut src_addr = [0u8; 4];
    let mut dst_addr = [0u8; 4];
    let mut next_hop = [0u8; 4];
    src_addr.copy_from_slice(&d[0..4]);
    dst_addr.copy_from_slice(&d[4..8]);
    next_hop.copy_from_slice(&d[8..12]);

    Ok(NetFlowV5Record {
        src_addr,
        dst_addr,
        next_hop,
        snmp_input: u16::from_be_bytes([d[12], d[13]]),
        snmp_output: u16::from_be_bytes([d[14], d[15]]),
        packets: u32::from_be_bytes([d[16], d[17], d[18], d[19]]),
        octets: u32::from_be_bytes([d[20], d[21], d[22], d[23]]),
        first: u32::from_be_bytes([d[24], d[25], d[26], d[27]]),
        last: u32::from_be_bytes([d[28], d[29], d[30], d[31]]),
        src_port: u16::from_be_bytes([d[32], d[33]]),
        dst_port: u16::from_be_bytes([d[34], d[35]]),
        tcp_flags: d[37],
        protocol: d[38],
        tos: d[39],
        src_as: u16::from_be_bytes([d[40], d[41]]),
        dst_as: u16::from_be_bytes([d[42], d[43]]),
        src_mask: d[44],
        dst_mask: d[45],
    })
}

/// Parse a complete NetFlow v5 packet.
pub fn parse_v5_packet(
    data: &[u8],
) -> Result<(NetFlowV5Header, Vec<NetFlowV5Record>), ConnectorError> {
    let header = parse_v5_header(data)?;
    let mut records = Vec::new();
    let count = header.count as usize;

    for i in 0..count {
        let offset = 24 + i * 48;
        match parse_v5_record(data, offset) {
            Ok(r) => records.push(r),
            Err(_) => break,
        }
    }

    Ok((header, records))
}

/// Parse NetFlow v9 header.
pub fn parse_v9_header(data: &[u8]) -> Result<NetFlowV9Header, ConnectorError> {
    if data.len() < 20 {
        return Err(ConnectorError::ParseError(
            "NetFlow v9: header too short (need 20 bytes)".into(),
        ));
    }
    Ok(NetFlowV9Header {
        version: u16::from_be_bytes([data[0], data[1]]),
        count: u16::from_be_bytes([data[2], data[3]]),
        sys_uptime: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
        unix_secs: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        sequence_number: u32::from_be_bytes([data[12], data[13], data[14], data[15]]),
        source_id: u32::from_be_bytes([data[16], data[17], data[18], data[19]]),
    })
}

/// Parse a FlowSet header.
pub fn parse_flowset_header(data: &[u8], offset: usize) -> Result<FlowSetHeader, ConnectorError> {
    if offset + 4 > data.len() {
        return Err(ConnectorError::ParseError(
            "NetFlow v9: FlowSet header truncated".into(),
        ));
    }
    Ok(FlowSetHeader {
        flowset_id: u16::from_be_bytes([data[offset], data[offset + 1]]),
        length: u16::from_be_bytes([data[offset + 2], data[offset + 3]]),
    })
}

/// Parse a template FlowSet (flowset_id = 0).
pub fn parse_v9_template_flowset(
    data: &[u8],
    offset: usize,
    length: usize,
) -> Result<Vec<V9Template>, ConnectorError> {
    let mut templates = Vec::new();
    let end = offset + length;
    let mut pos = offset;

    while pos + 4 <= end {
        let template_id = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let field_count = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
        pos += 4;

        let mut fields = Vec::new();
        for _ in 0..field_count {
            if pos + 4 > end {
                break;
            }
            let field_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
            let field_length = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
            fields.push(V9FieldSpec {
                field_type,
                field_length,
            });
            pos += 4;
        }
        templates.push(V9Template {
            template_id,
            field_count,
            fields,
        });
    }

    Ok(templates)
}

/// Parse a data FlowSet using a known template.
pub fn parse_v9_data_flowset(
    data: &[u8],
    offset: usize,
    length: usize,
    template: &V9Template,
) -> Vec<GenericFlowRecord> {
    let end = offset + length;
    let mut pos = offset;
    let mut records = Vec::new();

    let record_len: usize = template.fields.iter().map(|f| f.field_length as usize).sum();
    if record_len == 0 {
        return records;
    }

    while pos + record_len <= end {
        let mut fields = HashMap::new();
        for spec in &template.fields {
            let len = spec.field_length as usize;
            if pos + len > end {
                break;
            }
            let field_data = &data[pos..pos + len];
            let name = v9_field_name(spec.field_type).to_string();
            let value = match len {
                1 => json!(field_data[0]),
                2 => json!(u16::from_be_bytes([field_data[0], field_data[1]])),
                4 => {
                    if spec.field_type == 8 || spec.field_type == 12 || spec.field_type == 15 {
                        // IPv4 address
                        json!(format!(
                            "{}.{}.{}.{}",
                            field_data[0], field_data[1], field_data[2], field_data[3]
                        ))
                    } else {
                        json!(u32::from_be_bytes([
                            field_data[0],
                            field_data[1],
                            field_data[2],
                            field_data[3]
                        ]))
                    }
                }
                8 => json!(u64::from_be_bytes([
                    field_data[0],
                    field_data[1],
                    field_data[2],
                    field_data[3],
                    field_data[4],
                    field_data[5],
                    field_data[6],
                    field_data[7],
                ])),
                _ => {
                    let hex: String =
                        field_data.iter().map(|b| format!("{:02x}", b)).collect();
                    json!(hex)
                }
            };
            fields.insert(name, value);
            pos += len;
        }
        records.push(GenericFlowRecord { fields });
    }

    records
}

// ---------------------------------------------------------------------------
// Flow → SourceEvent conversion
// ---------------------------------------------------------------------------

/// Convert a NetFlow v5 record into a SourceEvent.
pub fn v5_record_to_source_event(
    record: &NetFlowV5Record,
    header: &NetFlowV5Header,
    connector_id: &str,
) -> SourceEvent {
    let entity_id = format!(
        "netflow:{}:{}-{}:{}",
        record.src_addr_str(),
        record.src_port,
        record.dst_addr_str(),
        record.dst_port
    );

    let ts = DateTime::from_timestamp(header.unix_secs as i64, header.unix_nsecs)
        .unwrap_or_else(Utc::now);

    let mut properties = HashMap::new();
    properties.insert("src_ip".into(), json!(record.src_addr_str()));
    properties.insert("dst_ip".into(), json!(record.dst_addr_str()));
    properties.insert("src_port".into(), json!(record.src_port));
    properties.insert("dst_port".into(), json!(record.dst_port));
    properties.insert("protocol".into(), json!(record.protocol_name()));
    properties.insert("protocol_num".into(), json!(record.protocol));
    properties.insert("packets".into(), json!(record.packets));
    properties.insert("bytes".into(), json!(record.octets));
    properties.insert("duration_ms".into(), json!(record.duration_ms()));
    properties.insert("tcp_flags".into(), json!(record.tcp_flags_str()));
    properties.insert("tos".into(), json!(record.tos));
    properties.insert("src_as".into(), json!(record.src_as));
    properties.insert("dst_as".into(), json!(record.dst_as));
    properties.insert("next_hop".into(), json!(record.next_hop_str()));
    properties.insert("netflow_version".into(), json!(5));

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "network_flow".into(),
        properties,
        timestamp: ts,
        latitude: None,
        longitude: None,
    }
}

/// Convert a generic v9/IPFIX flow record into a SourceEvent.
pub fn generic_flow_to_source_event(
    record: &GenericFlowRecord,
    unix_secs: u32,
    connector_id: &str,
    version: u16,
) -> SourceEvent {
    let src_ip = record
        .fields
        .get("ipv4_src_addr")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();
    let dst_ip = record
        .fields
        .get("ipv4_dst_addr")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();
    let src_port = record
        .fields
        .get("l4_src_port")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let dst_port = record
        .fields
        .get("l4_dst_port")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let entity_id = format!("netflow:{}:{}-{}:{}", src_ip, src_port, dst_ip, dst_port);

    let ts = DateTime::from_timestamp(unix_secs as i64, 0).unwrap_or_else(Utc::now);

    let mut properties = record.fields.clone();
    properties.insert("netflow_version".into(), json!(version));

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "network_flow".into(),
        properties,
        timestamp: ts,
        latitude: None,
        longitude: None,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

pub struct NetFlowConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl NetFlowConnector {
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
impl Connector for NetFlowConnector {
    fn connector_id(&self) -> &str {
        &self.config.connector_id
    }

    async fn start(
        &self,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        let bind_addr = self
            .config
            .url
            .as_deref()
            .unwrap_or("0.0.0.0:2055");

        let socket = tokio::net::UdpSocket::bind(bind_addr)
            .await
            .map_err(|e| ConnectorError::ConnectionError(format!("NetFlow: bind failed: {}", e)))?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        let mut buf = vec![0u8; 65535];
        // Template cache for v9
        let mut templates: HashMap<u16, V9Template> = HashMap::new();

        while running.load(Ordering::Relaxed) {
            let recv = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                socket.recv_from(&mut buf),
            )
            .await;

            let (len, _addr) = match recv {
                Ok(Ok((len, addr))) => (len, addr),
                Ok(Err(_)) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                Err(_) => continue, // timeout, check running flag
            };

            let data = &buf[..len];
            let version = match detect_version(data) {
                Ok(v) => v,
                Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };

            match version {
                NetFlowVersion::V5 => {
                    if let Ok((header, records)) = parse_v5_packet(data) {
                        for record in &records {
                            let event =
                                v5_record_to_source_event(record, &header, &connector_id);
                            if tx.send(event).await.is_err() {
                                running.store(false, Ordering::SeqCst);
                                break;
                            }
                            events_processed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                NetFlowVersion::V9 | NetFlowVersion::Ipfix => {
                    let hdr_len = if version == NetFlowVersion::V9 { 20 } else { 16 };
                    if let Ok(v9_header) = parse_v9_header(data) {
                        let mut offset = hdr_len;
                        while offset + 4 <= len {
                            if let Ok(fs_hdr) = parse_flowset_header(data, offset) {
                                let fs_len = fs_hdr.length as usize;
                                if fs_len < 4 || offset + fs_len > len {
                                    break;
                                }
                                if fs_hdr.flowset_id == 0 {
                                    // Template FlowSet
                                    if let Ok(tmpls) = parse_v9_template_flowset(
                                        data,
                                        offset + 4,
                                        fs_len - 4,
                                    ) {
                                        for t in tmpls {
                                            templates.insert(t.template_id, t);
                                        }
                                    }
                                } else if fs_hdr.flowset_id >= 256 {
                                    // Data FlowSet
                                    if let Some(tmpl) = templates.get(&fs_hdr.flowset_id) {
                                        let records = parse_v9_data_flowset(
                                            data,
                                            offset + 4,
                                            fs_len - 4,
                                            tmpl,
                                        );
                                        for record in &records {
                                            let event = generic_flow_to_source_event(
                                                record,
                                                v9_header.unix_secs,
                                                &connector_id,
                                                v9_header.version,
                                            );
                                            if tx.send(event).await.is_err() {
                                                running.store(false, Ordering::SeqCst);
                                                break;
                                            }
                                            events_processed.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                }
                                offset += fs_len;
                            } else {
                                break;
                            }
                        }
                    }
                }
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
                "NetFlow connector is not running".into(),
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

    /// Build a NetFlow v5 packet with the given flow records.
    fn build_v5_packet(records: &[NetFlowV5Record]) -> Vec<u8> {
        let mut buf = Vec::new();
        // Header (24 bytes)
        buf.extend_from_slice(&5u16.to_be_bytes()); // version
        buf.extend_from_slice(&(records.len() as u16).to_be_bytes()); // count
        buf.extend_from_slice(&1000000u32.to_be_bytes()); // sys_uptime
        buf.extend_from_slice(&1700000000u32.to_be_bytes()); // unix_secs
        buf.extend_from_slice(&0u32.to_be_bytes()); // unix_nsecs
        buf.extend_from_slice(&1u32.to_be_bytes()); // flow_sequence
        buf.push(0); // engine_type
        buf.push(0); // engine_id
        buf.extend_from_slice(&0u16.to_be_bytes()); // sampling

        for r in records {
            buf.extend_from_slice(&r.src_addr);
            buf.extend_from_slice(&r.dst_addr);
            buf.extend_from_slice(&r.next_hop);
            buf.extend_from_slice(&r.snmp_input.to_be_bytes());
            buf.extend_from_slice(&r.snmp_output.to_be_bytes());
            buf.extend_from_slice(&r.packets.to_be_bytes());
            buf.extend_from_slice(&r.octets.to_be_bytes());
            buf.extend_from_slice(&r.first.to_be_bytes());
            buf.extend_from_slice(&r.last.to_be_bytes());
            buf.extend_from_slice(&r.src_port.to_be_bytes());
            buf.extend_from_slice(&r.dst_port.to_be_bytes());
            buf.push(0); // pad1
            buf.push(r.tcp_flags);
            buf.push(r.protocol);
            buf.push(r.tos);
            buf.extend_from_slice(&r.src_as.to_be_bytes());
            buf.extend_from_slice(&r.dst_as.to_be_bytes());
            buf.push(r.src_mask);
            buf.push(r.dst_mask);
            buf.extend_from_slice(&0u16.to_be_bytes()); // pad2
        }
        buf
    }

    fn sample_v5_record() -> NetFlowV5Record {
        NetFlowV5Record {
            src_addr: [192, 168, 1, 100],
            dst_addr: [10, 0, 0, 1],
            next_hop: [192, 168, 1, 1],
            snmp_input: 1,
            snmp_output: 2,
            packets: 42,
            octets: 12345,
            first: 900000,
            last: 1000000,
            src_port: 54321,
            dst_port: 80,
            tcp_flags: 0x12, // SYN + ACK
            protocol: 6,     // TCP
            tos: 0,
            src_as: 65000,
            dst_as: 65001,
            src_mask: 24,
            dst_mask: 16,
        }
    }

    #[test]
    fn test_detect_version_v5() {
        let mut data = vec![0u8; 24];
        data[0..2].copy_from_slice(&5u16.to_be_bytes());
        assert_eq!(detect_version(&data).unwrap(), NetFlowVersion::V5);
    }

    #[test]
    fn test_detect_version_v9() {
        let mut data = vec![0u8; 20];
        data[0..2].copy_from_slice(&9u16.to_be_bytes());
        assert_eq!(detect_version(&data).unwrap(), NetFlowVersion::V9);
    }

    #[test]
    fn test_detect_version_ipfix() {
        let mut data = vec![0u8; 20];
        data[0..2].copy_from_slice(&10u16.to_be_bytes());
        assert_eq!(detect_version(&data).unwrap(), NetFlowVersion::Ipfix);
    }

    #[test]
    fn test_detect_version_unknown() {
        let data = vec![0u8, 42u8];
        assert!(detect_version(&data).is_err());
    }

    #[test]
    fn test_parse_v5_header() {
        let record = sample_v5_record();
        let packet = build_v5_packet(&[record]);
        let header = parse_v5_header(&packet).unwrap();
        assert_eq!(header.version, 5);
        assert_eq!(header.count, 1);
        assert_eq!(header.unix_secs, 1700000000);
    }

    #[test]
    fn test_parse_v5_record() {
        let record = sample_v5_record();
        let packet = build_v5_packet(&[record]);
        let (header, records) = parse_v5_packet(&packet).unwrap();
        assert_eq!(header.count, 1);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].src_addr_str(), "192.168.1.100");
        assert_eq!(records[0].dst_addr_str(), "10.0.0.1");
        assert_eq!(records[0].src_port, 54321);
        assert_eq!(records[0].dst_port, 80);
        assert_eq!(records[0].packets, 42);
        assert_eq!(records[0].octets, 12345);
        assert_eq!(records[0].protocol, 6);
        assert_eq!(records[0].protocol_name(), "TCP");
    }

    #[test]
    fn test_v5_tcp_flags() {
        let record = sample_v5_record();
        let flags = record.tcp_flags_str();
        assert!(flags.contains("SYN"));
        assert!(flags.contains("ACK"));
    }

    #[test]
    fn test_v5_duration() {
        let record = sample_v5_record();
        assert_eq!(record.duration_ms(), 100000);
    }

    #[test]
    fn test_v5_record_to_source_event() {
        let record = sample_v5_record();
        let packet = build_v5_packet(std::slice::from_ref(&record));
        let (header, _) = parse_v5_packet(&packet).unwrap();
        let event = v5_record_to_source_event(&record, &header, "netflow-test");
        assert_eq!(event.entity_type, "network_flow");
        assert_eq!(
            event.entity_id,
            "netflow:192.168.1.100:54321-10.0.0.1:80"
        );
        assert_eq!(event.properties["packets"], json!(42));
        assert_eq!(event.properties["bytes"], json!(12345));
        assert_eq!(event.properties["protocol"], json!("TCP"));
        assert_eq!(event.properties["netflow_version"], json!(5));
    }

    #[test]
    fn test_multiple_v5_records() {
        let r1 = sample_v5_record();
        let mut r2 = sample_v5_record();
        r2.src_addr = [10, 1, 2, 3];
        r2.protocol = 17; // UDP
        let packet = build_v5_packet(&[r1, r2]);
        let (header, records) = parse_v5_packet(&packet).unwrap();
        assert_eq!(header.count, 2);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].protocol_name(), "TCP");
        assert_eq!(records[1].protocol_name(), "UDP");
    }

    #[test]
    fn test_parse_v9_header() {
        let mut data = vec![0u8; 20];
        data[0..2].copy_from_slice(&9u16.to_be_bytes());
        data[2..4].copy_from_slice(&2u16.to_be_bytes());
        data[8..12].copy_from_slice(&1700000000u32.to_be_bytes());
        data[16..20].copy_from_slice(&100u32.to_be_bytes());

        let header = parse_v9_header(&data).unwrap();
        assert_eq!(header.version, 9);
        assert_eq!(header.count, 2);
        assert_eq!(header.unix_secs, 1700000000);
        assert_eq!(header.source_id, 100);
    }

    #[test]
    fn test_parse_v9_template() {
        // Build a template FlowSet: template_id=256, fields: src_addr(4), dst_addr(4), src_port(2), dst_port(2), protocol(1)
        let mut data = Vec::new();
        data.extend_from_slice(&256u16.to_be_bytes()); // template ID
        data.extend_from_slice(&5u16.to_be_bytes()); // field count
        // src addr: type=8, len=4
        data.extend_from_slice(&8u16.to_be_bytes());
        data.extend_from_slice(&4u16.to_be_bytes());
        // dst addr: type=12, len=4
        data.extend_from_slice(&12u16.to_be_bytes());
        data.extend_from_slice(&4u16.to_be_bytes());
        // src port: type=7, len=2
        data.extend_from_slice(&7u16.to_be_bytes());
        data.extend_from_slice(&2u16.to_be_bytes());
        // dst port: type=11, len=2
        data.extend_from_slice(&11u16.to_be_bytes());
        data.extend_from_slice(&2u16.to_be_bytes());
        // protocol: type=4, len=1
        data.extend_from_slice(&4u16.to_be_bytes());
        data.extend_from_slice(&1u16.to_be_bytes());

        let templates = parse_v9_template_flowset(&data, 0, data.len()).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].template_id, 256);
        assert_eq!(templates[0].field_count, 5);
        assert_eq!(templates[0].fields.len(), 5);
    }

    #[test]
    fn test_parse_v9_data_flowset() {
        let template = V9Template {
            template_id: 256,
            field_count: 3,
            fields: vec![
                V9FieldSpec { field_type: 8, field_length: 4 },  // src_addr
                V9FieldSpec { field_type: 12, field_length: 4 }, // dst_addr
                V9FieldSpec { field_type: 7, field_length: 2 },  // src_port
            ],
        };

        let mut data = Vec::new();
        data.extend_from_slice(&[192, 168, 1, 100]); // src
        data.extend_from_slice(&[10, 0, 0, 1]); // dst
        data.extend_from_slice(&8080u16.to_be_bytes()); // src port

        let records = parse_v9_data_flowset(&data, 0, data.len(), &template);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].fields["ipv4_src_addr"], json!("192.168.1.100"));
        assert_eq!(records[0].fields["ipv4_dst_addr"], json!("10.0.0.1"));
        assert_eq!(records[0].fields["l4_src_port"], json!(8080));
    }

    #[test]
    fn test_generic_flow_to_source_event() {
        let mut fields = HashMap::new();
        fields.insert("ipv4_src_addr".into(), json!("10.1.2.3"));
        fields.insert("ipv4_dst_addr".into(), json!("10.4.5.6"));
        fields.insert("l4_src_port".into(), json!(12345));
        fields.insert("l4_dst_port".into(), json!(443));
        let record = GenericFlowRecord { fields };

        let event = generic_flow_to_source_event(&record, 1700000000, "nf-test", 9);
        assert_eq!(event.entity_type, "network_flow");
        assert_eq!(event.entity_id, "netflow:10.1.2.3:12345-10.4.5.6:443");
        assert_eq!(event.properties["netflow_version"], json!(9));
    }

    #[test]
    fn test_netflow_connector_id() {
        let config = ConnectorConfig {
            connector_id: "nf-1".to_string(),
            connector_type: "netflow".to_string(),
            url: None,
            entity_type: "network_flow".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = NetFlowConnector::new(config);
        assert_eq!(connector.connector_id(), "nf-1");
    }

    #[tokio::test]
    async fn test_netflow_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "nf-health".to_string(),
            connector_type: "netflow".to_string(),
            url: None,
            entity_type: "network_flow".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = NetFlowConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_v5_header_too_short() {
        let data = vec![0u8; 10];
        assert!(parse_v5_header(&data).is_err());
    }
}
