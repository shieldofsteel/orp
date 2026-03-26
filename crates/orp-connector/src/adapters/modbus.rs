use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Modbus protocol model
// ---------------------------------------------------------------------------
// Modbus is the most widely deployed ICS (Industrial Control System) protocol.
// Two main variants:
//   - Modbus RTU: binary serial (RS-485/RS-232)
//   - Modbus TCP: binary over TCP (port 502), with MBAP header
//
// Data model:
//   - Coils (single-bit read/write): addresses 00001–09999
//   - Discrete Inputs (single-bit read-only): 10001–19999
//   - Holding Registers (16-bit read/write): 40001–49999
//   - Input Registers (16-bit read-only): 30001–39999
//
// Function codes:
//   FC01: Read Coils
//   FC02: Read Discrete Inputs
//   FC03: Read Holding Registers
//   FC04: Read Input Registers
//   FC05: Write Single Coil
//   FC06: Write Single Register
//   FC15: Write Multiple Coils
//   FC16: Write Multiple Registers

/// Modbus function code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionCode {
    ReadCoils,
    ReadDiscreteInputs,
    ReadHoldingRegisters,
    ReadInputRegisters,
    WriteSingleCoil,
    WriteSingleRegister,
    WriteMultipleCoils,
    WriteMultipleRegisters,
    Unknown(u8),
}

impl FunctionCode {
    pub fn from_u8(code: u8) -> Self {
        match code {
            0x01 => FunctionCode::ReadCoils,
            0x02 => FunctionCode::ReadDiscreteInputs,
            0x03 => FunctionCode::ReadHoldingRegisters,
            0x04 => FunctionCode::ReadInputRegisters,
            0x05 => FunctionCode::WriteSingleCoil,
            0x06 => FunctionCode::WriteSingleRegister,
            0x0F => FunctionCode::WriteMultipleCoils,
            0x10 => FunctionCode::WriteMultipleRegisters,
            _ => FunctionCode::Unknown(code),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            FunctionCode::ReadCoils => 0x01,
            FunctionCode::ReadDiscreteInputs => 0x02,
            FunctionCode::ReadHoldingRegisters => 0x03,
            FunctionCode::ReadInputRegisters => 0x04,
            FunctionCode::WriteSingleCoil => 0x05,
            FunctionCode::WriteSingleRegister => 0x06,
            FunctionCode::WriteMultipleCoils => 0x0F,
            FunctionCode::WriteMultipleRegisters => 0x10,
            FunctionCode::Unknown(c) => c,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FunctionCode::ReadCoils => "ReadCoils",
            FunctionCode::ReadDiscreteInputs => "ReadDiscreteInputs",
            FunctionCode::ReadHoldingRegisters => "ReadHoldingRegisters",
            FunctionCode::ReadInputRegisters => "ReadInputRegisters",
            FunctionCode::WriteSingleCoil => "WriteSingleCoil",
            FunctionCode::WriteSingleRegister => "WriteSingleRegister",
            FunctionCode::WriteMultipleCoils => "WriteMultipleCoils",
            FunctionCode::WriteMultipleRegisters => "WriteMultipleRegisters",
            FunctionCode::Unknown(_) => "Unknown",
        }
    }

    pub fn is_read(&self) -> bool {
        matches!(
            self,
            FunctionCode::ReadCoils
                | FunctionCode::ReadDiscreteInputs
                | FunctionCode::ReadHoldingRegisters
                | FunctionCode::ReadInputRegisters
        )
    }
}

/// Modbus TCP MBAP (Modbus Application Protocol) header.
#[derive(Clone, Debug, PartialEq)]
pub struct MbapHeader {
    pub transaction_id: u16,
    pub protocol_id: u16, // Always 0x0000 for Modbus
    pub length: u16,
    pub unit_id: u8,
}

/// A Modbus TCP request PDU.
#[derive(Clone, Debug)]
pub struct ModbusRequest {
    pub header: MbapHeader,
    pub function_code: FunctionCode,
    pub start_address: u16,
    pub quantity: u16,
}

/// A Modbus TCP response PDU.
#[derive(Clone, Debug)]
pub struct ModbusResponse {
    pub header: MbapHeader,
    pub function_code: FunctionCode,
    pub data: Vec<u8>,
}

/// Modbus exception response.
#[derive(Clone, Debug, PartialEq)]
pub struct ModbusException {
    pub function_code: u8,
    pub exception_code: ModbusExceptionCode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModbusExceptionCode {
    IllegalFunction,
    IllegalDataAddress,
    IllegalDataValue,
    SlaveDeviceFailure,
    Acknowledge,
    SlaveDeviceBusy,
    GatewayPathUnavailable,
    GatewayTargetFailed,
    Unknown(u8),
}

impl ModbusExceptionCode {
    pub fn from_u8(code: u8) -> Self {
        match code {
            0x01 => ModbusExceptionCode::IllegalFunction,
            0x02 => ModbusExceptionCode::IllegalDataAddress,
            0x03 => ModbusExceptionCode::IllegalDataValue,
            0x04 => ModbusExceptionCode::SlaveDeviceFailure,
            0x05 => ModbusExceptionCode::Acknowledge,
            0x06 => ModbusExceptionCode::SlaveDeviceBusy,
            0x0A => ModbusExceptionCode::GatewayPathUnavailable,
            0x0B => ModbusExceptionCode::GatewayTargetFailed,
            _ => ModbusExceptionCode::Unknown(code),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ModbusExceptionCode::IllegalFunction => "IllegalFunction",
            ModbusExceptionCode::IllegalDataAddress => "IllegalDataAddress",
            ModbusExceptionCode::IllegalDataValue => "IllegalDataValue",
            ModbusExceptionCode::SlaveDeviceFailure => "SlaveDeviceFailure",
            ModbusExceptionCode::Acknowledge => "Acknowledge",
            ModbusExceptionCode::SlaveDeviceBusy => "SlaveDeviceBusy",
            ModbusExceptionCode::GatewayPathUnavailable => "GatewayPathUnavailable",
            ModbusExceptionCode::GatewayTargetFailed => "GatewayTargetFailed",
            ModbusExceptionCode::Unknown(_) => "Unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// Modbus TCP parser
// ---------------------------------------------------------------------------

/// Parse a Modbus TCP MBAP header (7 bytes).
pub fn parse_mbap_header(data: &[u8]) -> Result<(MbapHeader, usize), ConnectorError> {
    if data.len() < 7 {
        return Err(ConnectorError::ParseError(
            "Modbus TCP: MBAP header too short (< 7 bytes)".to_string(),
        ));
    }
    let transaction_id = u16::from_be_bytes([data[0], data[1]]);
    let protocol_id = u16::from_be_bytes([data[2], data[3]]);
    let length = u16::from_be_bytes([data[4], data[5]]);
    let unit_id = data[6];

    Ok((
        MbapHeader {
            transaction_id,
            protocol_id,
            length,
            unit_id,
        },
        7,
    ))
}

/// Parse a Modbus TCP response frame (MBAP + PDU).
pub fn parse_modbus_tcp_response(
    data: &[u8],
) -> Result<ModbusResponse, ConnectorError> {
    let (header, offset) = parse_mbap_header(data)?;
    if offset >= data.len() {
        return Err(ConnectorError::ParseError(
            "Modbus TCP: no PDU after MBAP header".to_string(),
        ));
    }

    let fc_raw = data[offset];

    // Check for exception response (function code has high bit set)
    if fc_raw & 0x80 != 0 {
        let exception_code = if offset + 1 < data.len() {
            data[offset + 1]
        } else {
            0
        };
        return Err(ConnectorError::ParseError(format!(
            "Modbus exception: FC={:#04X}, code={}",
            fc_raw & 0x7F,
            ModbusExceptionCode::from_u8(exception_code).as_str()
        )));
    }

    let function_code = FunctionCode::from_u8(fc_raw);

    // For read responses, next byte is byte count, then data
    let response_data = if function_code.is_read() {
        if offset + 1 >= data.len() {
            return Err(ConnectorError::ParseError(
                "Modbus TCP: truncated read response".to_string(),
            ));
        }
        let byte_count = data[offset + 1] as usize;
        let start = offset + 2;
        let end = start + byte_count;
        if end > data.len() {
            return Err(ConnectorError::ParseError(
                "Modbus TCP: response data truncated".to_string(),
            ));
        }
        data[start..end].to_vec()
    } else {
        // Write responses echo the request
        data[offset + 1..].to_vec()
    };

    Ok(ModbusResponse {
        header,
        function_code,
        data: response_data,
    })
}

/// Parse a Modbus TCP request frame.
pub fn parse_modbus_tcp_request(
    data: &[u8],
) -> Result<ModbusRequest, ConnectorError> {
    let (header, offset) = parse_mbap_header(data)?;
    if offset + 4 >= data.len() {
        return Err(ConnectorError::ParseError(
            "Modbus TCP: request PDU too short".to_string(),
        ));
    }

    let function_code = FunctionCode::from_u8(data[offset]);
    let start_address = u16::from_be_bytes([data[offset + 1], data[offset + 2]]);
    let quantity = u16::from_be_bytes([data[offset + 3], data[offset + 4]]);

    Ok(ModbusRequest {
        header,
        function_code,
        start_address,
        quantity,
    })
}

// ---------------------------------------------------------------------------
// Modbus RTU CRC
// ---------------------------------------------------------------------------

/// Calculate Modbus RTU CRC-16.
pub fn modbus_crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 0x0001 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// Verify Modbus RTU frame CRC (last 2 bytes are CRC, little-endian).
pub fn verify_rtu_crc(frame: &[u8]) -> bool {
    if frame.len() < 4 {
        return false;
    }
    let payload = &frame[..frame.len() - 2];
    let received_crc = u16::from_le_bytes([
        frame[frame.len() - 2],
        frame[frame.len() - 1],
    ]);
    modbus_crc16(payload) == received_crc
}

// ---------------------------------------------------------------------------
// Register value interpretation
// ---------------------------------------------------------------------------

/// Interpret register values as different data types.
pub struct RegisterInterpreter;

impl RegisterInterpreter {
    /// Read a single u16 register from response data.
    pub fn read_u16(data: &[u8], register_offset: usize) -> Option<u16> {
        let byte_offset = register_offset * 2;
        if byte_offset + 1 < data.len() {
            Some(u16::from_be_bytes([
                data[byte_offset],
                data[byte_offset + 1],
            ]))
        } else {
            None
        }
    }

    /// Read a signed 16-bit value.
    pub fn read_i16(data: &[u8], register_offset: usize) -> Option<i16> {
        Self::read_u16(data, register_offset).map(|v| v as i16)
    }

    /// Read a 32-bit float (IEEE 754) from two consecutive registers.
    pub fn read_f32(data: &[u8], register_offset: usize) -> Option<f32> {
        let byte_offset = register_offset * 2;
        if byte_offset + 3 < data.len() {
            let bytes = [
                data[byte_offset],
                data[byte_offset + 1],
                data[byte_offset + 2],
                data[byte_offset + 3],
            ];
            Some(f32::from_be_bytes(bytes))
        } else {
            None
        }
    }

    /// Read a 32-bit unsigned from two consecutive registers.
    pub fn read_u32(data: &[u8], register_offset: usize) -> Option<u32> {
        let byte_offset = register_offset * 2;
        if byte_offset + 3 < data.len() {
            Some(u32::from_be_bytes([
                data[byte_offset],
                data[byte_offset + 1],
                data[byte_offset + 2],
                data[byte_offset + 3],
            ]))
        } else {
            None
        }
    }

    /// Read a single coil/discrete bit value from response data.
    pub fn read_coil(data: &[u8], bit_offset: usize) -> Option<bool> {
        let byte_idx = bit_offset / 8;
        let bit_idx = bit_offset % 8;
        data.get(byte_idx).map(|b| (b >> bit_idx) & 1 == 1)
    }
}

// ---------------------------------------------------------------------------
// Register map configuration
// ---------------------------------------------------------------------------

/// Configuration for a single Modbus register to monitor.
#[derive(Clone, Debug)]
pub struct RegisterMapping {
    pub address: u16,
    pub name: String,
    pub data_type: RegisterDataType,
    pub unit: Option<String>,
    pub scale: f64,
    pub offset: f64,
    pub entity_type: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegisterDataType {
    UInt16,
    Int16,
    Float32,
    UInt32,
    Coil,
}

impl RegisterMapping {
    /// Read and interpret a value from response data.
    pub fn read_value(&self, data: &[u8], base_address: u16) -> Option<f64> {
        let reg_offset = (self.address - base_address) as usize;
        match self.data_type {
            RegisterDataType::UInt16 => {
                RegisterInterpreter::read_u16(data, reg_offset)
                    .map(|v| v as f64 * self.scale + self.offset)
            }
            RegisterDataType::Int16 => {
                RegisterInterpreter::read_i16(data, reg_offset)
                    .map(|v| v as f64 * self.scale + self.offset)
            }
            RegisterDataType::Float32 => {
                RegisterInterpreter::read_f32(data, reg_offset)
                    .map(|v| v as f64 * self.scale + self.offset)
            }
            RegisterDataType::UInt32 => {
                RegisterInterpreter::read_u32(data, reg_offset)
                    .map(|v| v as f64 * self.scale + self.offset)
            }
            RegisterDataType::Coil => {
                RegisterInterpreter::read_coil(data, reg_offset)
                    .map(|v| if v { 1.0 } else { 0.0 })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Modbus → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a Modbus response with register mappings to SourceEvents.
pub fn modbus_response_to_events(
    response: &ModbusResponse,
    mappings: &[RegisterMapping],
    base_address: u16,
    connector_id: &str,
    device_id: &str,
    latitude: Option<f64>,
    longitude: Option<f64>,
) -> Vec<SourceEvent> {
    let mut events = Vec::new();

    for mapping in mappings {
        if let Some(value) = mapping.read_value(&response.data, base_address) {
            let mut properties: HashMap<String, serde_json::Value> = HashMap::new();
            properties.insert("value".into(), serde_json::json!(value));
            properties.insert(
                "register_address".into(),
                serde_json::json!(mapping.address),
            );
            properties.insert("register_name".into(), serde_json::json!(mapping.name));
            properties.insert(
                "unit_id".into(),
                serde_json::json!(response.header.unit_id),
            );
            properties.insert(
                "function_code".into(),
                serde_json::json!(response.function_code.as_str()),
            );
            if let Some(ref unit) = mapping.unit {
                properties.insert("unit".into(), serde_json::json!(unit));
            }

            events.push(SourceEvent {
                connector_id: connector_id.to_string(),
                entity_id: format!(
                    "modbus:{}:{}:{}",
                    device_id, response.header.unit_id, mapping.address
                ),
                entity_type: mapping.entity_type.clone(),
                properties,
                timestamp: Utc::now(),
                latitude,
                longitude,
            });
        }
    }

    events
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// Modbus TCP connector — polls Modbus TCP devices for register values.
pub struct ModbusConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl ModbusConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Build a Modbus TCP read holding registers request.
    pub fn build_read_holding_registers(
        transaction_id: u16,
        unit_id: u8,
        start_address: u16,
        quantity: u16,
    ) -> Vec<u8> {
        let length: u16 = 6; // unit_id(1) + fc(1) + addr(2) + qty(2)
        let mut frame = Vec::with_capacity(12);
        frame.extend_from_slice(&transaction_id.to_be_bytes());
        frame.extend_from_slice(&0u16.to_be_bytes()); // protocol ID
        frame.extend_from_slice(&length.to_be_bytes());
        frame.push(unit_id);
        frame.push(0x03); // FC03: Read Holding Registers
        frame.extend_from_slice(&start_address.to_be_bytes());
        frame.extend_from_slice(&quantity.to_be_bytes());
        frame
    }

    /// Build a Modbus TCP read input registers request.
    pub fn build_read_input_registers(
        transaction_id: u16,
        unit_id: u8,
        start_address: u16,
        quantity: u16,
    ) -> Vec<u8> {
        let length: u16 = 6;
        let mut frame = Vec::with_capacity(12);
        frame.extend_from_slice(&transaction_id.to_be_bytes());
        frame.extend_from_slice(&0u16.to_be_bytes());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.push(unit_id);
        frame.push(0x04); // FC04: Read Input Registers
        frame.extend_from_slice(&start_address.to_be_bytes());
        frame.extend_from_slice(&quantity.to_be_bytes());
        frame
    }
}

#[async_trait]
impl Connector for ModbusConnector {
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
            "Modbus connector started"
        );

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let errors_count = self.errors_count.clone();
        let connector_id = self.config.connector_id.clone();
        let url = self.config.url.clone();
        let props = self.config.properties.clone();

        tokio::spawn(async move {
            // If a TCP address is configured, connect and poll
            if let Some(ref url_str) = url {
                if let Some(addr) = url_str
                    .strip_prefix("tcp://")
                    .or_else(|| url_str.strip_prefix("modbus://"))
                {
                    let poll_secs = props
                        .get("poll_interval_secs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(5);
                    let unit_id = props
                        .get("unit_id")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1) as u8;
                    let start_addr = props
                        .get("start_address")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u16;
                    let quantity = props
                        .get("quantity")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(10) as u16;

                    let mut interval = tokio::time::interval(
                        tokio::time::Duration::from_secs(poll_secs),
                    );
                    let mut txn_id: u16 = 0;

                    while running.load(Ordering::SeqCst) {
                        interval.tick().await;
                        match tokio::net::TcpStream::connect(addr).await {
                            Ok(stream) => {
                                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                                let request =
                                    ModbusConnector::build_read_holding_registers(
                                        txn_id, unit_id, start_addr, quantity,
                                    );
                                let (mut reader, mut writer) =
                                    stream.into_split();
                                if writer.write_all(&request).await.is_err() {
                                    errors_count
                                        .fetch_add(1, Ordering::Relaxed);
                                    continue;
                                }
                                let mut buf = vec![0u8; 256];
                                match reader.read(&mut buf).await {
                                    Ok(n) if n > 0 => {
                                        match parse_modbus_tcp_response(
                                            &buf[..n],
                                        ) {
                                            Ok(resp) => {
                                                // Simple: emit raw register values as a single event
                                                let mut properties = HashMap::new();
                                                properties.insert(
                                                    "unit_id".into(),
                                                    serde_json::json!(unit_id),
                                                );
                                                properties.insert(
                                                    "function_code".into(),
                                                    serde_json::json!(resp.function_code.as_str()),
                                                );
                                                properties.insert(
                                                    "register_count".into(),
                                                    serde_json::json!(resp.data.len() / 2),
                                                );
                                                // Store first few register values
                                                for i in 0..(resp.data.len() / 2).min(10) {
                                                    if let Some(val) = RegisterInterpreter::read_u16(&resp.data, i) {
                                                        properties.insert(
                                                            format!("reg_{}", start_addr + i as u16),
                                                            serde_json::json!(val),
                                                        );
                                                    }
                                                }
                                                let event = SourceEvent {
                                                    connector_id: connector_id.clone(),
                                                    entity_id: format!(
                                                        "modbus:{}:{}",
                                                        addr, unit_id
                                                    ),
                                                    entity_type: "plc".to_string(),
                                                    properties,
                                                    timestamp: Utc::now(),
                                                    latitude: None,
                                                    longitude: None,
                                                };
                                                if tx.send(event).await.is_err() {
                                                    return;
                                                }
                                                events_count.fetch_add(
                                                    1,
                                                    Ordering::Relaxed,
                                                );
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "Modbus parse error: {}",
                                                    e
                                                );
                                                errors_count.fetch_add(
                                                    1,
                                                    Ordering::Relaxed,
                                                );
                                            }
                                        }
                                    }
                                    _ => {
                                        errors_count
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                txn_id = txn_id.wrapping_add(1);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Modbus TCP connect error: {}",
                                    e
                                );
                                errors_count
                                    .fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    return;
                }
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
            "Modbus connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "Modbus connector not running".to_string(),
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

    // Build a valid Modbus TCP response for FC03 (Read Holding Registers)
    // Returns: MBAP header + FC + byte_count + register data
    fn build_fc03_response(
        transaction_id: u16,
        unit_id: u8,
        register_values: &[u16],
    ) -> Vec<u8> {
        let byte_count = (register_values.len() * 2) as u8;
        let pdu_len = 2 + byte_count as usize; // FC(1) + byte_count(1) + data
        let length = 1 + pdu_len as u16; // unit_id(1) + PDU

        let mut frame = Vec::new();
        frame.extend_from_slice(&transaction_id.to_be_bytes());
        frame.extend_from_slice(&0u16.to_be_bytes()); // protocol ID
        frame.extend_from_slice(&length.to_be_bytes());
        frame.push(unit_id);
        frame.push(0x03); // FC03
        frame.push(byte_count);
        for &val in register_values {
            frame.extend_from_slice(&val.to_be_bytes());
        }
        frame
    }

    #[test]
    fn test_function_code_from_u8() {
        assert_eq!(FunctionCode::from_u8(0x01), FunctionCode::ReadCoils);
        assert_eq!(
            FunctionCode::from_u8(0x03),
            FunctionCode::ReadHoldingRegisters
        );
        assert_eq!(
            FunctionCode::from_u8(0x10),
            FunctionCode::WriteMultipleRegisters
        );
        assert_eq!(FunctionCode::from_u8(0xFF), FunctionCode::Unknown(0xFF));
    }

    #[test]
    fn test_function_code_roundtrip() {
        assert_eq!(FunctionCode::ReadCoils.to_u8(), 0x01);
        assert_eq!(FunctionCode::ReadHoldingRegisters.to_u8(), 0x03);
        assert_eq!(FunctionCode::WriteMultipleRegisters.to_u8(), 0x10);
    }

    #[test]
    fn test_function_code_is_read() {
        assert!(FunctionCode::ReadCoils.is_read());
        assert!(FunctionCode::ReadInputRegisters.is_read());
        assert!(!FunctionCode::WriteSingleCoil.is_read());
        assert!(!FunctionCode::WriteMultipleRegisters.is_read());
    }

    #[test]
    fn test_parse_mbap_header() {
        let data = [0x00, 0x01, 0x00, 0x00, 0x00, 0x06, 0x01];
        let (header, consumed) = parse_mbap_header(&data).unwrap();
        assert_eq!(header.transaction_id, 1);
        assert_eq!(header.protocol_id, 0);
        assert_eq!(header.length, 6);
        assert_eq!(header.unit_id, 1);
        assert_eq!(consumed, 7);
    }

    #[test]
    fn test_parse_mbap_too_short() {
        assert!(parse_mbap_header(&[0x00, 0x01]).is_err());
    }

    #[test]
    fn test_parse_modbus_tcp_response() {
        let frame = build_fc03_response(1, 1, &[100, 200, 300]);
        let resp = parse_modbus_tcp_response(&frame).unwrap();
        assert_eq!(resp.function_code, FunctionCode::ReadHoldingRegisters);
        assert_eq!(resp.data.len(), 6); // 3 registers × 2 bytes
    }

    #[test]
    fn test_parse_modbus_tcp_request() {
        let frame = ModbusConnector::build_read_holding_registers(1, 1, 100, 10);
        let req = parse_modbus_tcp_request(&frame).unwrap();
        assert_eq!(req.function_code, FunctionCode::ReadHoldingRegisters);
        assert_eq!(req.start_address, 100);
        assert_eq!(req.quantity, 10);
        assert_eq!(req.header.unit_id, 1);
    }

    #[test]
    fn test_parse_modbus_exception() {
        // Exception response: FC with high bit set + exception code
        let mut frame = Vec::new();
        frame.extend_from_slice(&1u16.to_be_bytes()); // txn ID
        frame.extend_from_slice(&0u16.to_be_bytes()); // protocol
        frame.extend_from_slice(&3u16.to_be_bytes()); // length
        frame.push(1); // unit ID
        frame.push(0x83); // FC03 + 0x80 = exception
        frame.push(0x02); // IllegalDataAddress

        let result = parse_modbus_tcp_response(&frame);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("IllegalDataAddress"));
    }

    #[test]
    fn test_register_interpreter_u16() {
        let data = [0x00, 0x64, 0x00, 0xC8]; // 100, 200
        assert_eq!(RegisterInterpreter::read_u16(&data, 0), Some(100));
        assert_eq!(RegisterInterpreter::read_u16(&data, 1), Some(200));
        assert_eq!(RegisterInterpreter::read_u16(&data, 2), None);
    }

    #[test]
    fn test_register_interpreter_i16() {
        let data = [0xFF, 0x9C]; // -100 in i16
        assert_eq!(RegisterInterpreter::read_i16(&data, 0), Some(-100));
    }

    #[test]
    fn test_register_interpreter_f32() {
        let val: f32 = 42.5;
        let bytes = val.to_be_bytes();
        assert!(
            (RegisterInterpreter::read_f32(&bytes, 0).unwrap() - 42.5).abs()
                < 0.001
        );
    }

    #[test]
    fn test_register_interpreter_coil() {
        let data = [0x05]; // bits: 00000101 → bit0=1, bit1=0, bit2=1
        assert_eq!(RegisterInterpreter::read_coil(&data, 0), Some(true));
        assert_eq!(RegisterInterpreter::read_coil(&data, 1), Some(false));
        assert_eq!(RegisterInterpreter::read_coil(&data, 2), Some(true));
    }

    #[test]
    fn test_modbus_crc16() {
        // Known CRC for Modbus RTU: device 1, FC03, addr 0000, qty 0001
        let data = [0x01, 0x03, 0x00, 0x00, 0x00, 0x01];
        let crc = modbus_crc16(&data);
        assert_eq!(crc, 0x0A84);
    }

    #[test]
    fn test_verify_rtu_crc() {
        // Frame with valid CRC appended (little-endian)
        let frame = [0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A];
        assert!(verify_rtu_crc(&frame));
    }

    #[test]
    fn test_verify_rtu_crc_invalid() {
        let frame = [0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0xFF, 0xFF];
        assert!(!verify_rtu_crc(&frame));
    }

    #[test]
    fn test_register_mapping_read_value() {
        let mapping = RegisterMapping {
            address: 100,
            name: "Temperature".into(),
            data_type: RegisterDataType::UInt16,
            unit: Some("°C".into()),
            scale: 0.1,
            offset: 0.0,
            entity_type: "temperature_sensor".into(),
        };
        let data = [0x01, 0x2C]; // 300 → 300 × 0.1 = 30.0°C
        assert!((mapping.read_value(&data, 100).unwrap() - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_modbus_response_to_events() {
        let frame = build_fc03_response(1, 1, &[300, 500]);
        let resp = parse_modbus_tcp_response(&frame).unwrap();

        let mappings = vec![
            RegisterMapping {
                address: 0,
                name: "Temperature".into(),
                data_type: RegisterDataType::UInt16,
                unit: Some("°C".into()),
                scale: 0.1,
                offset: 0.0,
                entity_type: "temperature_sensor".into(),
            },
            RegisterMapping {
                address: 1,
                name: "Pressure".into(),
                data_type: RegisterDataType::UInt16,
                unit: Some("kPa".into()),
                scale: 1.0,
                offset: 0.0,
                entity_type: "pressure_sensor".into(),
            },
        ];

        let events = modbus_response_to_events(
            &resp, &mappings, 0, "modbus-test", "plc-1", None, None,
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].entity_type, "temperature_sensor");
        assert_eq!(events[1].entity_type, "pressure_sensor");
    }

    #[test]
    fn test_build_read_holding_registers() {
        let frame =
            ModbusConnector::build_read_holding_registers(42, 1, 100, 10);
        assert_eq!(frame.len(), 12);
        assert_eq!(frame[0..2], 42u16.to_be_bytes()); // txn ID
        assert_eq!(frame[6], 1); // unit ID
        assert_eq!(frame[7], 0x03); // FC03
    }

    #[test]
    fn test_build_read_input_registers() {
        let frame =
            ModbusConnector::build_read_input_registers(1, 2, 0, 5);
        assert_eq!(frame[7], 0x04); // FC04
        assert_eq!(frame[6], 2); // unit ID
    }

    #[test]
    fn test_exception_code_strings() {
        assert_eq!(
            ModbusExceptionCode::from_u8(0x01),
            ModbusExceptionCode::IllegalFunction
        );
        assert_eq!(
            ModbusExceptionCode::IllegalFunction.as_str(),
            "IllegalFunction"
        );
        assert_eq!(
            ModbusExceptionCode::from_u8(0x04),
            ModbusExceptionCode::SlaveDeviceFailure
        );
    }

    #[test]
    fn test_modbus_connector_id() {
        let config = ConnectorConfig {
            connector_id: "modbus-1".to_string(),
            connector_type: "modbus".to_string(),
            url: None,
            entity_type: "plc".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = ModbusConnector::new(config);
        assert_eq!(connector.connector_id(), "modbus-1");
    }

    #[tokio::test]
    async fn test_modbus_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "modbus-health".to_string(),
            connector_type: "modbus".to_string(),
            url: None,
            entity_type: "plc".to_string(),
            enabled: true,
            trust_score: 0.9,
            properties: HashMap::new(),
        };
        let connector = ModbusConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }
}
