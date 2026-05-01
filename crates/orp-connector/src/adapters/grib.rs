use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// GRIB2 / BUFR meteorological data parser
// ---------------------------------------------------------------------------
// GRIB (GRIdded Binary, WMO FM 92) is the standard format for distributing
// numerical weather prediction (NWP) model output: temperature, wind,
// pressure, precipitation, etc. on regular or irregular grids.
//
// GRIB2 (Edition 2) structure:
//   Section 0 — Indicator: "GRIB" magic, discipline, edition, total_length
//   Section 1 — Identification: originating center, reference time, data type
//   Section 2 — Local Use (optional)
//   Section 3 — Grid Definition: projection, grid size, lat/lon bounds
//   Section 4 — Product Definition: parameter, forecast time, surface
//   Section 5 — Data Representation: packing method, reference value, scale factors
//   Section 6 — Bitmap (optional)
//   Section 7 — Data: packed field values
//   Section 8 — End: "7777"
//
// All multi-byte integers are big-endian (network byte order).

/// GRIB2 Section 0 — Indicator.
#[derive(Clone, Debug)]
pub struct GribIndicator {
    pub discipline: u8,
    pub edition: u8,
    pub total_length: u64,
}

impl GribIndicator {
    pub fn discipline_name(&self) -> &'static str {
        match self.discipline {
            0 => "Meteorological",
            1 => "Hydrological",
            2 => "Land Surface",
            3 => "Satellite Remote Sensing (Space)",
            4 => "Space Weather",
            10 => "Oceanographic",
            _ => "Unknown",
        }
    }
}

/// GRIB2 Section 1 — Identification.
#[derive(Clone, Debug)]
pub struct GribIdentification {
    pub center_id: u16,
    pub subcenter_id: u16,
    pub master_tables_version: u8,
    pub local_tables_version: u8,
    pub significance_of_ref_time: u8,
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub production_status: u8,
    pub data_type: u8,
}

impl GribIdentification {
    pub fn center_name(&self) -> &'static str {
        match self.center_id {
            7 => "NCEP (US National Weather Service)",
            8 => "NWS Telecommunications Gateway",
            9 => "NWS Other",
            34 => "JMA (Japan Meteorological Agency)",
            46 => "CMC (Canadian Meteorological Centre)",
            78 | 79 => "DWD (Deutscher Wetterdienst)",
            85 => "ECMWF",
            98 => "ECMWF",
            _ => "Unknown Center",
        }
    }

    pub fn reference_time_string(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// GRIB2 Section 3 — Grid Definition.
#[derive(Clone, Debug)]
pub struct GribGridDefinition {
    pub source_of_grid_def: u8,
    pub num_data_points: u32,
    pub grid_template_number: u16,
    pub ni: u32,         // number of points along parallel
    pub nj: u32,         // number of points along meridian
    pub lat_first: f64,  // latitude of first grid point (degrees)
    pub lon_first: f64,  // longitude of first grid point (degrees)
    pub lat_last: f64,   // latitude of last grid point (degrees)
    pub lon_last: f64,   // longitude of last grid point (degrees)
    pub di: f64,         // i-direction increment (degrees)
    pub dj: f64,         // j-direction increment (degrees)
}

/// GRIB2 Section 4 — Product Definition.
#[derive(Clone, Debug)]
pub struct GribProductDefinition {
    pub product_template_number: u16,
    pub parameter_category: u8,
    pub parameter_number: u8,
    pub generating_process_type: u8,
    pub forecast_time: u32,
    pub surface_type: u8,
    pub surface_value: i32,
}

/// GRIB2 Section 5 — Data Representation.
#[derive(Clone, Debug)]
pub struct GribDataRepresentation {
    pub num_data_points: u32,
    pub template_number: u16,
    pub reference_value: f32,
    pub binary_scale_factor: i16,
    pub decimal_scale_factor: i16,
    pub bits_per_value: u8,
}

/// Parameter information lookup.
#[derive(Clone, Debug)]
pub struct GribParameterInfo {
    pub name: &'static str,
    pub unit: &'static str,
    pub discipline: u8,
    pub category: u8,
    pub number: u8,
}

/// Full parsed GRIB2 message.
#[derive(Clone, Debug)]
pub struct GribMessage {
    pub indicator: GribIndicator,
    pub identification: GribIdentification,
    pub grid_definition: Option<GribGridDefinition>,
    pub product_definition: Option<GribProductDefinition>,
    pub data_representation: Option<GribDataRepresentation>,
    /// Section 5 template number (0 = simple packing). 0xFFFF when no S5 parsed.
    pub data_template: u16,
    /// Unpacked Section 7 values. `None` ⇒ S7 absent or template unsupported
    /// (a `tracing::warn!` is emitted in that case).
    pub data_values: Option<Vec<f64>>,
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parse Section 0 — Indicator (16 bytes).
pub fn parse_grib_indicator(data: &[u8]) -> Result<GribIndicator, ConnectorError> {
    if data.len() < 16 {
        return Err(ConnectorError::ParseError(
            "GRIB: indicator section too short (need 16 bytes)".into(),
        ));
    }
    // Check magic bytes "GRIB"
    if &data[0..4] != b"GRIB" {
        return Err(ConnectorError::ParseError(format!(
            "GRIB: invalid magic bytes (expected 'GRIB', got {:?})",
            &data[0..4]
        )));
    }

    let discipline = data[6];
    let edition = data[7];

    if edition != 2 {
        return Err(ConnectorError::ParseError(format!(
            "GRIB: unsupported edition {} (only edition 2 supported)",
            edition
        )));
    }

    let total_length = u64::from_be_bytes([
        data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
    ]);

    Ok(GribIndicator {
        discipline,
        edition,
        total_length,
    })
}

/// Parse Section 1 — Identification.
/// Input: section data starting at the section length field.
pub fn parse_grib_identification(data: &[u8]) -> Result<GribIdentification, ConnectorError> {
    if data.len() < 21 {
        return Err(ConnectorError::ParseError(
            "GRIB: identification section too short (need 21 bytes)".into(),
        ));
    }

    // Bytes 0-3: section length
    // Byte 4: section number (should be 1)
    let section_number = data[4];
    if section_number != 1 {
        return Err(ConnectorError::ParseError(format!(
            "GRIB: expected section 1, got {}",
            section_number
        )));
    }

    let center_id = u16::from_be_bytes([data[5], data[6]]);
    let subcenter_id = u16::from_be_bytes([data[7], data[8]]);
    let master_tables_version = data[9];
    let local_tables_version = data[10];
    let significance_of_ref_time = data[11];
    let year = u16::from_be_bytes([data[12], data[13]]);
    let month = data[14];
    let day = data[15];
    let hour = data[16];
    let minute = data[17];
    let second = data[18];
    let production_status = data[19];
    let data_type = data[20];

    Ok(GribIdentification {
        center_id,
        subcenter_id,
        master_tables_version,
        local_tables_version,
        significance_of_ref_time,
        year,
        month,
        day,
        hour,
        minute,
        second,
        production_status,
        data_type,
    })
}

/// Parse Section 3 — Grid Definition (lat/lon grid, template 0).
/// Input: section data starting at the section length field.
pub fn parse_grib_grid_definition(data: &[u8]) -> Result<GribGridDefinition, ConnectorError> {
    if data.len() < 72 {
        return Err(ConnectorError::ParseError(
            "GRIB: grid definition section too short".into(),
        ));
    }

    let section_number = data[4];
    if section_number != 3 {
        return Err(ConnectorError::ParseError(format!(
            "GRIB: expected section 3, got {}",
            section_number
        )));
    }

    let source_of_grid_def = data[5];
    let num_data_points = u32::from_be_bytes([data[6], data[7], data[8], data[9]]);
    let grid_template_number = u16::from_be_bytes([data[12], data[13]]);

    // Template 0 (Latitude/Longitude) data starts at byte 14
    // Skip shape_of_earth(1) + scale factors(5+5) = 14+11 = byte 25
    let template_offset = 14;

    // Shape of earth at template_offset (byte 14)
    // Scale factor numerator at 15-18, denominator at 19-22
    // Ni at 30-33, Nj at 34-37 (relative offsets within template data)
    let ni_offset = template_offset + 16; // byte 30
    let nj_offset = template_offset + 20; // byte 34

    let ni = if data.len() > ni_offset + 3 {
        u32::from_be_bytes([
            data[ni_offset],
            data[ni_offset + 1],
            data[ni_offset + 2],
            data[ni_offset + 3],
        ])
    } else {
        0
    };

    let nj = if data.len() > nj_offset + 3 {
        u32::from_be_bytes([
            data[nj_offset],
            data[nj_offset + 1],
            data[nj_offset + 2],
            data[nj_offset + 3],
        ])
    } else {
        0
    };

    // Lat/lon of first point at bytes 46-49 and 50-53 (signed, 1e-6 degrees)
    let lat_first_offset = template_offset + 24; // byte 38
    let lon_first_offset = template_offset + 28; // byte 42

    let lat_first = if data.len() > lat_first_offset + 3 {
        let raw = i32::from_be_bytes([
            data[lat_first_offset],
            data[lat_first_offset + 1],
            data[lat_first_offset + 2],
            data[lat_first_offset + 3],
        ]);
        raw as f64 / 1_000_000.0
    } else {
        0.0
    };

    let lon_first = if data.len() > lon_first_offset + 3 {
        let raw = i32::from_be_bytes([
            data[lon_first_offset],
            data[lon_first_offset + 1],
            data[lon_first_offset + 2],
            data[lon_first_offset + 3],
        ]);
        raw as f64 / 1_000_000.0
    } else {
        0.0
    };

    // Resolution byte, then lat/lon last, then di/dj
    let lat_last_offset = template_offset + 33; // byte 47
    let lon_last_offset = template_offset + 37; // byte 51

    let lat_last = if data.len() > lat_last_offset + 3 {
        let raw = i32::from_be_bytes([
            data[lat_last_offset],
            data[lat_last_offset + 1],
            data[lat_last_offset + 2],
            data[lat_last_offset + 3],
        ]);
        raw as f64 / 1_000_000.0
    } else {
        0.0
    };

    let lon_last = if data.len() > lon_last_offset + 3 {
        let raw = i32::from_be_bytes([
            data[lon_last_offset],
            data[lon_last_offset + 1],
            data[lon_last_offset + 2],
            data[lon_last_offset + 3],
        ]);
        raw as f64 / 1_000_000.0
    } else {
        0.0
    };

    let di_offset = template_offset + 41; // byte 55
    let dj_offset = template_offset + 45; // byte 59

    let di = if data.len() > di_offset + 3 {
        let raw = u32::from_be_bytes([
            data[di_offset],
            data[di_offset + 1],
            data[di_offset + 2],
            data[di_offset + 3],
        ]);
        raw as f64 / 1_000_000.0
    } else {
        0.0
    };

    let dj = if data.len() > dj_offset + 3 {
        let raw = u32::from_be_bytes([
            data[dj_offset],
            data[dj_offset + 1],
            data[dj_offset + 2],
            data[dj_offset + 3],
        ]);
        raw as f64 / 1_000_000.0
    } else {
        0.0
    };

    Ok(GribGridDefinition {
        source_of_grid_def,
        num_data_points,
        grid_template_number,
        ni,
        nj,
        lat_first,
        lon_first,
        lat_last,
        lon_last,
        di,
        dj,
    })
}

/// Parse Section 4 — Product Definition (template 0).
pub fn parse_grib_product_definition(data: &[u8]) -> Result<GribProductDefinition, ConnectorError> {
    if data.len() < 34 {
        return Err(ConnectorError::ParseError(
            "GRIB: product definition section too short".into(),
        ));
    }

    let section_number = data[4];
    if section_number != 4 {
        return Err(ConnectorError::ParseError(format!(
            "GRIB: expected section 4, got {}",
            section_number
        )));
    }

    let product_template_number = u16::from_be_bytes([data[7], data[8]]);
    let parameter_category = data[9];
    let parameter_number = data[10];
    let generating_process_type = data[11];

    // Forecast time at bytes 18-21 (for template 0)
    let forecast_time = if data.len() > 21 {
        u32::from_be_bytes([data[18], data[19], data[20], data[21]])
    } else {
        0
    };

    // Surface type at byte 22, surface value at bytes 23-26
    let surface_type = if data.len() > 22 { data[22] } else { 0 };
    let surface_value = if data.len() > 26 {
        i32::from_be_bytes([data[23], data[24], data[25], data[26]])
    } else {
        0
    };

    Ok(GribProductDefinition {
        product_template_number,
        parameter_category,
        parameter_number,
        generating_process_type,
        forecast_time,
        surface_type,
        surface_value,
    })
}

/// Parse Section 5 — Data Representation (template 0, simple packing).
pub fn parse_grib_data_representation(data: &[u8]) -> Result<GribDataRepresentation, ConnectorError> {
    if data.len() < 21 {
        return Err(ConnectorError::ParseError(
            "GRIB: data representation section too short".into(),
        ));
    }

    let section_number = data[4];
    if section_number != 5 {
        return Err(ConnectorError::ParseError(format!(
            "GRIB: expected section 5, got {}",
            section_number
        )));
    }

    let num_data_points = u32::from_be_bytes([data[5], data[6], data[7], data[8]]);
    let template_number = u16::from_be_bytes([data[9], data[10]]);

    let reference_value = f32::from_be_bytes([data[11], data[12], data[13], data[14]]);
    let binary_scale_factor = i16::from_be_bytes([data[15], data[16]]);
    let decimal_scale_factor = i16::from_be_bytes([data[17], data[18]]);
    let bits_per_value = data[19];

    Ok(GribDataRepresentation {
        num_data_points,
        template_number,
        reference_value,
        binary_scale_factor,
        decimal_scale_factor,
        bits_per_value,
    })
}

// Section 7 unpacking — DRT 5.0: Y = (R + X * 2^E) / 10^D, where R/E/D come
// from Section 5 and X is the i-th nbits-bit big-endian unsigned integer.
// nbits == 0 is a legal "constant field": every grid point = R / 10^D.

/// Read `nbits` bits from `buf` at `bit_offset` (MSB-first stream).
/// Returns `None` on out-of-range or `nbits > 64`. `nbits == 0` ⇒ `Some(0)`.
pub fn read_bits(buf: &[u8], bit_offset: usize, nbits: usize) -> Option<u64> {
    if nbits == 0 { return Some(0); }
    if nbits > 64 { return None; }
    let end_bit = bit_offset.checked_add(nbits)?;
    if (end_bit - 1) / 8 >= buf.len() { return None; }
    let mut value: u64 = 0;
    let mut bits_remaining = nbits;
    let mut bit_pos = bit_offset;
    while bits_remaining > 0 {
        let bit_in_byte = bit_pos % 8;
        let avail = 8 - bit_in_byte;
        let take = avail.min(bits_remaining);
        let byte = *buf.get(bit_pos / 8)?;
        let chunk = ((byte as u64) >> (avail - take)) & ((1u64 << take) - 1);
        value = (value << take) | chunk;
        bit_pos += take;
        bits_remaining -= take;
    }
    Some(value)
}

/// Unpack Section 7 using DRT 5.0 (simple packing). Bounds-checks the bit
/// stream; returns `ParseError` on truncation, never panics.
pub fn unpack_template_5_0(
    section5: &GribDataRepresentation,
    section7_data: &[u8],
    num_data_points: usize,
) -> Result<Vec<f64>, ConnectorError> {
    let nbits = section5.bits_per_value as usize;
    let r = section5.reference_value as f64;
    let e = section5.binary_scale_factor as i32;
    let d = section5.decimal_scale_factor as i32;
    let dec_div = 10f64.powi(d);
    if nbits == 0 {
        return Ok(vec![r / dec_div; num_data_points]);
    }
    let total_bits = num_data_points
        .checked_mul(nbits)
        .ok_or_else(|| ConnectorError::ParseError(
            "GRIB: Section 7 bit count overflow".into(),
        ))?;
    let needed_bytes = (total_bits + 7) / 8;
    if section7_data.len() < needed_bytes {
        return Err(ConnectorError::ParseError(format!(
            "GRIB: Section 7 truncated (need {} bytes, have {})",
            needed_bytes, section7_data.len()
        )));
    }
    let mut out = Vec::with_capacity(num_data_points);
    let two_e = 2f64.powi(e); // 2^E (1/2^|E| for negative E)
    for i in 0..num_data_points {
        let x = read_bits(section7_data, i * nbits, nbits).ok_or_else(|| {
            ConnectorError::ParseError(format!("GRIB: Section 7 bit read failed at point {}", i))
        })?;
        out.push((r + (x as f64) * two_e) / dec_div);
    }
    Ok(out)
}

/// Look up parameter name from discipline, category, and number.
pub fn lookup_parameter(discipline: u8, category: u8, number: u8) -> GribParameterInfo {
    match (discipline, category, number) {
        // Discipline 0: Meteorological
        (0, 0, 0) => GribParameterInfo {
            name: "Temperature",
            unit: "K",
            discipline: 0,
            category: 0,
            number: 0,
        },
        (0, 0, 2) => GribParameterInfo {
            name: "Potential Temperature",
            unit: "K",
            discipline: 0,
            category: 0,
            number: 2,
        },
        (0, 0, 4) => GribParameterInfo {
            name: "Maximum Temperature",
            unit: "K",
            discipline: 0,
            category: 0,
            number: 4,
        },
        (0, 0, 5) => GribParameterInfo {
            name: "Minimum Temperature",
            unit: "K",
            discipline: 0,
            category: 0,
            number: 5,
        },
        (0, 0, 6) => GribParameterInfo {
            name: "Dew Point Temperature",
            unit: "K",
            discipline: 0,
            category: 0,
            number: 6,
        },
        // Moisture
        (0, 1, 0) => GribParameterInfo {
            name: "Specific Humidity",
            unit: "kg/kg",
            discipline: 0,
            category: 1,
            number: 0,
        },
        (0, 1, 1) => GribParameterInfo {
            name: "Relative Humidity",
            unit: "%",
            discipline: 0,
            category: 1,
            number: 1,
        },
        (0, 1, 8) => GribParameterInfo {
            name: "Total Precipitation",
            unit: "kg/m²",
            discipline: 0,
            category: 1,
            number: 8,
        },
        // Momentum / Wind
        (0, 2, 2) => GribParameterInfo {
            name: "U-Component of Wind",
            unit: "m/s",
            discipline: 0,
            category: 2,
            number: 2,
        },
        (0, 2, 3) => GribParameterInfo {
            name: "V-Component of Wind",
            unit: "m/s",
            discipline: 0,
            category: 2,
            number: 3,
        },
        (0, 2, 1) => GribParameterInfo {
            name: "Wind Speed",
            unit: "m/s",
            discipline: 0,
            category: 2,
            number: 1,
        },
        (0, 2, 0) => GribParameterInfo {
            name: "Wind Direction",
            unit: "degrees",
            discipline: 0,
            category: 2,
            number: 0,
        },
        // Mass / Pressure
        (0, 3, 0) => GribParameterInfo {
            name: "Pressure",
            unit: "Pa",
            discipline: 0,
            category: 3,
            number: 0,
        },
        (0, 3, 1) => GribParameterInfo {
            name: "Pressure Reduced to MSL",
            unit: "Pa",
            discipline: 0,
            category: 3,
            number: 1,
        },
        (0, 3, 5) => GribParameterInfo {
            name: "Geopotential Height",
            unit: "gpm",
            discipline: 0,
            category: 3,
            number: 5,
        },
        // Cloud
        (0, 6, 1) => GribParameterInfo {
            name: "Total Cloud Cover",
            unit: "%",
            discipline: 0,
            category: 6,
            number: 1,
        },
        // Oceanographic
        (10, 0, 0) => GribParameterInfo {
            name: "Wave Spectra 1",
            unit: "-",
            discipline: 10,
            category: 0,
            number: 0,
        },
        (10, 0, 3) => GribParameterInfo {
            name: "Significant Wave Height",
            unit: "m",
            discipline: 10,
            category: 0,
            number: 3,
        },
        _ => GribParameterInfo {
            name: "Unknown Parameter",
            unit: "unknown",
            discipline,
            category,
            number,
        },
    }
}

fn empty_identification() -> GribIdentification {
    GribIdentification {
        center_id: 0, subcenter_id: 0, master_tables_version: 0, local_tables_version: 0,
        significance_of_ref_time: 0, year: 0, month: 0, day: 0, hour: 0, minute: 0,
        second: 0, production_status: 0, data_type: 0,
    }
}

/// Parse a complete GRIB2 message. `data` MUST start at the "GRIB" magic.
/// Walks all sections and unpacks Section 7 when Template 5.0 is present.
pub fn parse_grib_message(data: &[u8]) -> Result<GribMessage, ConnectorError> {
    let indicator = parse_grib_indicator(data)?;
    let total_len = (indicator.total_length as usize).max(16).min(data.len());
    let body = data.get(16..total_len).ok_or_else(|| {
        ConnectorError::ParseError("GRIB: message body slice out of range".into())
    })?;

    let mut identification: Option<GribIdentification> = None;
    let mut grid_definition: Option<GribGridDefinition> = None;
    let mut product_definition: Option<GribProductDefinition> = None;
    let mut data_representation: Option<GribDataRepresentation> = None;
    let mut data_template: u16 = 0xFFFF;
    let mut data_values: Option<Vec<f64>> = None;

    // Each section after S0: 4-byte BE length, 1-byte section number, payload.
    // Stop on Section 8 marker "7777" or buffer end.
    let mut cur = 0usize;
    while cur + 5 <= body.len() {
        if body.get(cur..cur + 4) == Some(b"7777") { break; }
        let lb = match body.get(cur..cur + 4) { Some(b) => b, None => break };
        let sec_len = u32::from_be_bytes([lb[0], lb[1], lb[2], lb[3]]) as usize;
        if sec_len < 5 || cur + sec_len > body.len() { break; }
        let sec_num = body[cur + 4];
        let sec = &body[cur..cur + sec_len];
        match sec_num {
            1 => identification = parse_grib_identification(sec).ok(),
            3 => grid_definition = parse_grib_grid_definition(sec).ok(),
            4 => product_definition = parse_grib_product_definition(sec).ok(),
            5 => if let Ok(d) = parse_grib_data_representation(sec) {
                data_template = d.template_number;
                data_representation = Some(d);
            },
            7 => if let Some(ref drep) = data_representation {
                let payload = sec.get(5..).unwrap_or(&[]);
                let n = drep.num_data_points as usize;
                match drep.template_number {
                    0 => match unpack_template_5_0(drep, payload, n) {
                        Ok(v) => data_values = Some(v),
                        Err(e) => tracing::warn!("GRIB Section 7 unpack failed: {}", e),
                    },
                    other => tracing::warn!(
                        "GRIB Section 5 template {} not yet supported, returning metadata only",
                        other
                    ),
                }
            },
            _ => {}
        }
        cur += sec_len;
    }

    Ok(GribMessage {
        indicator,
        identification: identification.unwrap_or_else(empty_identification),
        grid_definition,
        product_definition,
        data_representation,
        data_template,
        data_values,
    })
}

// ---------------------------------------------------------------------------
// GRIB → SourceEvent
// ---------------------------------------------------------------------------

/// Convert a GRIB message to a SourceEvent.
pub fn grib_to_source_event(
    msg: &GribMessage,
    connector_id: &str,
) -> SourceEvent {
    let mut properties = HashMap::new();
    properties.insert("discipline".into(), json!(msg.indicator.discipline));
    properties.insert(
        "discipline_name".into(),
        json!(msg.indicator.discipline_name()),
    );
    properties.insert("edition".into(), json!(msg.indicator.edition));
    properties.insert("center_id".into(), json!(msg.identification.center_id));
    properties.insert(
        "center_name".into(),
        json!(msg.identification.center_name()),
    );
    properties.insert(
        "reference_time".into(),
        json!(msg.identification.reference_time_string()),
    );

    let mut lat = None;
    let mut lon = None;

    if let Some(ref grid) = msg.grid_definition {
        properties.insert("grid_template".into(), json!(grid.grid_template_number));
        properties.insert("num_data_points".into(), json!(grid.num_data_points));
        properties.insert("ni".into(), json!(grid.ni));
        properties.insert("nj".into(), json!(grid.nj));
        properties.insert("lat_first".into(), json!(grid.lat_first));
        properties.insert("lon_first".into(), json!(grid.lon_first));
        properties.insert("lat_last".into(), json!(grid.lat_last));
        properties.insert("lon_last".into(), json!(grid.lon_last));
        properties.insert("di".into(), json!(grid.di));
        properties.insert("dj".into(), json!(grid.dj));

        // Use grid center as representative lat/lon
        lat = Some((grid.lat_first + grid.lat_last) / 2.0);
        lon = Some((grid.lon_first + grid.lon_last) / 2.0);
    }

    let mut param_name = "unknown";
    if let Some(ref prod) = msg.product_definition {
        let info = lookup_parameter(
            msg.indicator.discipline,
            prod.parameter_category,
            prod.parameter_number,
        );
        param_name = info.name;
        properties.insert("parameter_name".into(), json!(info.name));
        properties.insert("parameter_unit".into(), json!(info.unit));
        properties.insert("parameter_category".into(), json!(prod.parameter_category));
        properties.insert("parameter_number".into(), json!(prod.parameter_number));
        properties.insert("forecast_time".into(), json!(prod.forecast_time));
        properties.insert("surface_type".into(), json!(prod.surface_type));
    }

    if let Some(ref drep) = msg.data_representation {
        properties.insert("packing_template".into(), json!(drep.template_number));
        properties.insert("bits_per_value".into(), json!(drep.bits_per_value));
        properties.insert(
            "data_points".into(),
            json!(drep.num_data_points),
        );
    }

    properties.insert("data_template".into(), json!(msg.data_template));
    properties.insert("data_values_present".into(), json!(msg.data_values.is_some()));
    if let Some(ref values) = msg.data_values {
        if !values.is_empty() {
            let (mut min, mut max, mut sum) = (f64::INFINITY, f64::NEG_INFINITY, 0f64);
            for &v in values.iter() {
                if v.is_finite() {
                    if v < min { min = v; }
                    if v > max { max = v; }
                    sum += v;
                }
            }
            properties.insert("data_min".into(), json!(min));
            properties.insert("data_max".into(), json!(max));
            properties.insert("data_mean".into(), json!(sum / values.len() as f64));
            properties.insert("data_count".into(), json!(values.len()));
        }
    }

    let ref_time = msg.identification.reference_time_string();
    let entity_id = format!(
        "grib:{}:{}:{}",
        msg.identification.center_id,
        param_name.to_lowercase().replace(' ', "_"),
        ref_time
    );

    SourceEvent {
        connector_id: connector_id.to_string(),
        entity_id,
        entity_type: "weather_grid".to_string(),
        properties,
        timestamp: Utc::now(),
        latitude: lat,
        longitude: lon,
    }
}

// ---------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------

/// Emit one SourceEvent per visited grid cell, using `stride` to subsample.
async fn emit_cells(
    tx: &tokio::sync::mpsc::Sender<SourceEvent>,
    msg: &GribMessage, grid: &GribGridDefinition, values: &[f64],
    connector_id: &str, events_processed: &Arc<AtomicU64>, stride: u32,
) {
    let ni = grid.ni.max(1);
    let nj = grid.nj.max(1);
    let stride = stride.max(1) as usize;
    let param = msg.product_definition.as_ref().map(|p| {
        lookup_parameter(msg.indicator.discipline, p.parameter_category, p.parameter_number)
    });
    let ref_time = msg.identification.reference_time_string();
    let lat_sign = if grid.lat_last < grid.lat_first { -1.0 } else { 1.0 };
    let pname = param.as_ref().map(|p| p.name).unwrap_or("unknown")
        .to_lowercase().replace(' ', "_");
    for j in (0..nj).step_by(stride) {
        for i in (0..ni).step_by(stride) {
            let v = match values.get((j as usize) * (ni as usize) + i as usize) {
                Some(v) => *v, None => continue,
            };
            let mut props = HashMap::new();
            props.insert("value".into(), json!(v));
            props.insert("i".into(), json!(i));
            props.insert("j".into(), json!(j));
            if let Some(ref p) = param {
                props.insert("parameter_name".into(), json!(p.name));
                props.insert("parameter_unit".into(), json!(p.unit));
            }
            let evt = SourceEvent {
                connector_id: connector_id.to_string(),
                entity_id: format!("grib:{}:{}:{}:{}x{}",
                    msg.identification.center_id, pname, ref_time, i, j),
                entity_type: "weather_grid_cell".to_string(),
                properties: props,
                timestamp: Utc::now(),
                latitude: Some(grid.lat_first + (j as f64) * grid.dj * lat_sign),
                longitude: Some(grid.lon_first + (i as f64) * grid.di),
            };
            if tx.send(evt).await.is_err() { return; }
            events_processed.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub struct GribConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl GribConnector {
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
impl Connector for GribConnector {
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
                ConnectorError::ConfigError("GRIB: url (file path) required".into())
            })?;

        let data = tokio::fs::read(path)
            .await
            .map_err(ConnectorError::IoError)?;

        self.running.store(true, Ordering::SeqCst);
        let connector_id = self.config.connector_id.clone();
        let events_processed = Arc::clone(&self.events_processed);
        let errors = Arc::clone(&self.errors);
        let running = Arc::clone(&self.running);

        // Scan for GRIB messages (each starts with "GRIB").
        let mut offset = 0;
        while offset + 16 <= data.len() && running.load(Ordering::Relaxed) {
            if &data[offset..offset + 4] != b"GRIB" {
                offset += 1;
                continue;
            }
            match parse_grib_message(&data[offset..]) {
                Ok(grib_msg) => {
                    let msg_len = (grib_msg.indicator.total_length as usize).max(16);
                    // Summary event (carries min/max/mean when values present).
                    if tx.send(grib_to_source_event(&grib_msg, &connector_id)).await.is_err() {
                        break;
                    }
                    events_processed.fetch_add(1, Ordering::Relaxed);
                    // Per-cell events: full grid up to 1000 cells, else 10x stride.
                    if let (Some(ref values), Some(ref grid)) =
                        (&grib_msg.data_values, &grib_msg.grid_definition)
                    {
                        let stride = if values.len() > 1000 { 10 } else { 1 };
                        if !values.is_empty() {
                            emit_cells(&tx, &grib_msg, grid, values,
                                &connector_id, &events_processed, stride).await;
                        }
                    }
                    offset += msg_len;
                }
                Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    offset += 1;
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
                "GRIB connector is not running".into(),
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

    fn make_indicator_bytes(discipline: u8, total_length: u64) -> Vec<u8> {
        let mut data = vec![0u8; 16];
        data[0..4].copy_from_slice(b"GRIB");
        data[6] = discipline;
        data[7] = 2; // edition 2
        let len_bytes = total_length.to_be_bytes();
        data[8..16].copy_from_slice(&len_bytes);
        data
    }

    #[test]
    fn test_parse_grib_indicator_valid() {
        let data = make_indicator_bytes(0, 1024);
        let ind = parse_grib_indicator(&data).unwrap();
        assert_eq!(ind.discipline, 0);
        assert_eq!(ind.edition, 2);
        assert_eq!(ind.total_length, 1024);
        assert_eq!(ind.discipline_name(), "Meteorological");
    }

    #[test]
    fn test_parse_grib_indicator_invalid() {
        let data = vec![0x00, 0x01, 0x02, 0x03, 0x00, 0x00, 0x00, 0x02, 0, 0, 0, 0, 0, 0, 4, 0];
        assert!(parse_grib_indicator(&data).is_err());
    }

    #[test]
    fn test_parse_grib_indicator_edition1() {
        let mut data = make_indicator_bytes(0, 512);
        data[7] = 1; // edition 1
        assert!(parse_grib_indicator(&data).is_err());
    }

    #[test]
    fn test_parse_grib_identification() {
        let mut data = vec![0u8; 21];
        // section length (not validated by parser)
        data[0..4].copy_from_slice(&21u32.to_be_bytes());
        data[4] = 1; // section number
        // center_id = 7 (NCEP)
        data[5..7].copy_from_slice(&7u16.to_be_bytes());
        // subcenter_id = 0
        data[7..9].copy_from_slice(&0u16.to_be_bytes());
        data[9] = 2; // master tables version
        data[10] = 1; // local tables version
        data[11] = 1; // significance
        // year = 2026
        data[12..14].copy_from_slice(&2026u16.to_be_bytes());
        data[14] = 3;  // month
        data[15] = 26; // day
        data[16] = 12; // hour
        data[17] = 0;  // minute
        data[18] = 0;  // second
        data[19] = 0;  // production_status
        data[20] = 1;  // data_type

        let id = parse_grib_identification(&data).unwrap();
        assert_eq!(id.center_id, 7);
        assert_eq!(id.center_name(), "NCEP (US National Weather Service)");
        assert_eq!(id.year, 2026);
        assert_eq!(id.month, 3);
        assert_eq!(id.day, 26);
        assert_eq!(id.reference_time_string(), "2026-03-26T12:00:00Z");
    }

    #[test]
    fn test_parse_grib_grid_definition() {
        let mut data = vec![0u8; 80];
        // Section header
        data[0..4].copy_from_slice(&80u32.to_be_bytes());
        data[4] = 3; // section number
        data[5] = 0; // source of grid def
        data[6..10].copy_from_slice(&10000u32.to_be_bytes()); // num_data_points
        data[12..14].copy_from_slice(&0u16.to_be_bytes()); // template 0

        // Ni at offset 30 (14+16)
        data[30..34].copy_from_slice(&100u32.to_be_bytes());
        // Nj at offset 34 (14+20)
        data[34..38].copy_from_slice(&100u32.to_be_bytes());
        // lat_first at offset 38 (14+24), 90.0 degrees = 90000000
        data[38..42].copy_from_slice(&90_000_000i32.to_be_bytes());
        // lon_first at offset 42 (14+28)
        data[42..46].copy_from_slice(&0i32.to_be_bytes());
        // lat_last at offset 47 (14+33)
        data[47..51].copy_from_slice(&(-90_000_000i32).to_be_bytes());
        // lon_last at offset 51 (14+37)
        data[51..55].copy_from_slice(&360_000_000i32.to_be_bytes());
        // di at offset 55 (14+41)
        data[55..59].copy_from_slice(&1_000_000u32.to_be_bytes());
        // dj at offset 59 (14+45)
        data[59..63].copy_from_slice(&1_000_000u32.to_be_bytes());

        let grid = parse_grib_grid_definition(&data).unwrap();
        assert_eq!(grid.num_data_points, 10000);
        assert_eq!(grid.grid_template_number, 0);
        assert_eq!(grid.ni, 100);
        assert_eq!(grid.nj, 100);
        assert!((grid.lat_first - 90.0).abs() < 0.001);
        assert!((grid.di - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_grib_product_definition() {
        let mut data = vec![0u8; 34];
        data[0..4].copy_from_slice(&34u32.to_be_bytes());
        data[4] = 4; // section number
        data[7..9].copy_from_slice(&0u16.to_be_bytes()); // template 0
        data[9] = 0;  // parameter_category (Temperature)
        data[10] = 0; // parameter_number (Temperature)
        data[11] = 2; // generating_process_type
        data[18..22].copy_from_slice(&6u32.to_be_bytes()); // forecast_time = 6h
        data[22] = 100; // surface type (isobaric)
        data[23..27].copy_from_slice(&50000i32.to_be_bytes()); // 500 hPa

        let prod = parse_grib_product_definition(&data).unwrap();
        assert_eq!(prod.parameter_category, 0);
        assert_eq!(prod.parameter_number, 0);
        assert_eq!(prod.forecast_time, 6);
        assert_eq!(prod.surface_type, 100);
    }

    #[test]
    fn test_parse_grib_data_representation() {
        let mut data = vec![0u8; 21];
        data[0..4].copy_from_slice(&21u32.to_be_bytes());
        data[4] = 5; // section number
        data[5..9].copy_from_slice(&10000u32.to_be_bytes()); // num_data_points
        data[9..11].copy_from_slice(&0u16.to_be_bytes()); // template 0
        // reference_value as IEEE float (e.g., 273.15)
        let ref_val = 273.15f32;
        data[11..15].copy_from_slice(&ref_val.to_be_bytes());
        data[15..17].copy_from_slice(&(-2i16).to_be_bytes()); // binary_scale_factor
        data[17..19].copy_from_slice(&1i16.to_be_bytes()); // decimal_scale_factor
        data[19] = 16; // bits_per_value

        let drep = parse_grib_data_representation(&data).unwrap();
        assert_eq!(drep.num_data_points, 10000);
        assert_eq!(drep.template_number, 0);
        assert!((drep.reference_value - 273.15).abs() < 0.01);
        assert_eq!(drep.binary_scale_factor, -2);
        assert_eq!(drep.decimal_scale_factor, 1);
        assert_eq!(drep.bits_per_value, 16);
    }

    #[test]
    fn test_lookup_parameter_temperature() {
        let info = lookup_parameter(0, 0, 0);
        assert_eq!(info.name, "Temperature");
        assert_eq!(info.unit, "K");
    }

    #[test]
    fn test_lookup_parameter_wind() {
        let info = lookup_parameter(0, 2, 2);
        assert_eq!(info.name, "U-Component of Wind");
        assert_eq!(info.unit, "m/s");

        let info2 = lookup_parameter(0, 2, 3);
        assert_eq!(info2.name, "V-Component of Wind");
    }

    #[test]
    fn test_lookup_parameter_pressure() {
        let info = lookup_parameter(0, 3, 0);
        assert_eq!(info.name, "Pressure");
        assert_eq!(info.unit, "Pa");

        let info2 = lookup_parameter(0, 3, 1);
        assert_eq!(info2.name, "Pressure Reduced to MSL");
    }

    #[test]
    fn test_lookup_parameter_unknown() {
        let info = lookup_parameter(255, 255, 255);
        assert_eq!(info.name, "Unknown Parameter");
    }

    #[test]
    fn test_grib_to_source_event() {
        let msg = GribMessage {
            indicator: GribIndicator {
                discipline: 0,
                edition: 2,
                total_length: 1024,
            },
            identification: GribIdentification {
                center_id: 7,
                subcenter_id: 0,
                master_tables_version: 2,
                local_tables_version: 0,
                significance_of_ref_time: 1,
                year: 2026,
                month: 3,
                day: 26,
                hour: 12,
                minute: 0,
                second: 0,
                production_status: 0,
                data_type: 1,
            },
            grid_definition: Some(GribGridDefinition {
                source_of_grid_def: 0,
                num_data_points: 10000,
                grid_template_number: 0,
                ni: 100,
                nj: 100,
                lat_first: 90.0,
                lon_first: 0.0,
                lat_last: -90.0,
                lon_last: 360.0,
                di: 1.0,
                dj: 1.0,
            }),
            product_definition: Some(GribProductDefinition {
                product_template_number: 0,
                parameter_category: 0,
                parameter_number: 0,
                generating_process_type: 2,
                forecast_time: 6,
                surface_type: 100,
                surface_value: 50000,
            }),
            data_representation: None,
            data_template: 0xFFFF,
            data_values: None,
        };

        let event = grib_to_source_event(&msg, "grib-test");
        assert_eq!(event.entity_type, "weather_grid");
        assert!(event.entity_id.contains("grib:7:temperature"));
        assert_eq!(event.properties["parameter_name"], json!("Temperature"));
        assert_eq!(event.properties["center_name"], json!("NCEP (US National Weather Service)"));
        assert!(event.latitude.is_some());
        assert!(event.longitude.is_some());
    }

    #[test]
    fn test_grib_entity_type() {
        let msg = GribMessage {
            indicator: GribIndicator {
                discipline: 0,
                edition: 2,
                total_length: 100,
            },
            identification: GribIdentification {
                center_id: 85,
                subcenter_id: 0,
                master_tables_version: 2,
                local_tables_version: 0,
                significance_of_ref_time: 0,
                year: 2026,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                production_status: 0,
                data_type: 0,
            },
            grid_definition: None,
            product_definition: None,
            data_representation: None,
            data_template: 0xFFFF,
            data_values: None,
        };
        let event = grib_to_source_event(&msg, "test");
        assert_eq!(event.entity_type, "weather_grid");
    }

    #[test]
    fn test_grib_connector_id() {
        let config = ConnectorConfig {
            connector_id: "grib-1".to_string(),
            connector_type: "grib".to_string(),
            url: None,
            entity_type: "weather_grid".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = GribConnector::new(config);
        assert_eq!(connector.connector_id(), "grib-1");
    }

    #[tokio::test]
    async fn test_grib_health_check_not_running() {
        let config = ConnectorConfig {
            connector_id: "grib-h".to_string(),
            connector_type: "grib".to_string(),
            url: None,
            entity_type: "weather_grid".to_string(),
            enabled: true,
            trust_score: 0.8,
            properties: HashMap::new(),
        };
        let connector = GribConnector::new(config);
        assert!(connector.health_check().await.is_err());
    }

    #[test]
    fn test_grib_discipline_names() {
        let disciplines = vec![
            (0, "Meteorological"),
            (1, "Hydrological"),
            (2, "Land Surface"),
            (10, "Oceanographic"),
            (99, "Unknown"),
        ];
        for (code, name) in disciplines {
            let ind = GribIndicator {
                discipline: code,
                edition: 2,
                total_length: 0,
            };
            assert_eq!(ind.discipline_name(), name);
        }
    }

    // -------- Section 7 unpacking ---------------------------------------

    fn drep(nbits: u8, r: f32, e: i16, d: i16, n: u32) -> GribDataRepresentation {
        GribDataRepresentation {
            num_data_points: n,
            template_number: 0,
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: nbits,
        }
    }

    #[test]
    fn test_read_bits_basic() {
        // 0xB2=10110010, 0x59=01011001.
        let buf = [0xB2u8, 0x59u8];
        assert_eq!(read_bits(&buf, 0, 4), Some(0b1011));      // first nibble
        assert_eq!(read_bits(&buf, 4, 4), Some(0b0010));      // second nibble
        assert_eq!(read_bits(&buf, 6, 8), Some(0b10010110));  // cross-byte 8 bits
        assert_eq!(read_bits(&buf, 0, 0), Some(0));           // nbits=0
        assert_eq!(read_bits(&buf, 12, 8), None);             // out of range
        assert_eq!(read_bits(&buf, 0, 65), None);             // nbits > 64
    }

    #[test]
    fn test_unpack_constant_field_nbits_zero() {
        // nbits=0 ⇒ every value is R / 10^D, no per-point data needed.
        let out = unpack_template_5_0(&drep(0, 100.0, 0, 0, 5), &[], 5).unwrap();
        assert_eq!(out.len(), 5);
        assert!(out.iter().all(|v| (v - 100.0).abs() < 1e-9));
        let out2 = unpack_template_5_0(&drep(0, 100.0, 0, 2, 3), &[], 3).unwrap();
        assert!(out2.iter().all(|v| (v - 1.0).abs() < 1e-9));
    }

    #[test]
    fn test_unpack_simple_known_case() {
        // nbits=8, R=273.15, E=0, D=2, single 0xFF byte → X=255
        // Y = (273.15 + 255 * 1) / 100 = 528.15/100 = 5.2815
        let s5 = drep(8, 273.15, 0, 2, 1);
        let out = unpack_template_5_0(&s5, &[0xFF], 1).unwrap();
        assert_eq!(out.len(), 1);
        assert!((out[0] - 5.2815).abs() < 1e-6, "got {}", out[0]);
    }

    #[test]
    fn test_unpack_truncated_buffer_errors() {
        // nbits=16, 4 values needs 8 bytes; only 4 supplied → ParseError.
        let s5 = drep(16, 0.0, 0, 0, 4);
        match unpack_template_5_0(&s5, &[0, 0, 0, 0], 4) {
            Err(ConnectorError::ParseError(m)) => {
                assert!(m.contains("Section 7"), "msg: {}", m);
            }
            other => panic!("expected ParseError, got Ok or non-ParseError: {:?}", other.is_err()),
        }
    }

    #[test]
    fn test_unpack_multibyte_bit_aligned_extraction() {
        // Two 12-bit values 0x123, 0x456 → MSB-first stream 0x12 0x34 0x56.
        // R=0, E=0, D=0 ⇒ Y = X.
        let s5 = drep(12, 0.0, 0, 0, 2);
        let out = unpack_template_5_0(&s5, &[0x12, 0x34, 0x56], 2).unwrap();
        assert!((out[0] - 0x123 as f64).abs() < 1e-9, "got {}", out[0]);
        assert!((out[1] - 0x456 as f64).abs() < 1e-9, "got {}", out[1]);
    }

    #[test]
    fn test_parse_grib_message_unsupported_template_returns_none() {
        // Synthetic GRIB2: indicator + Section 5 (template 200) + Section 7 + "7777".
        let mut msg = Vec::new();
        msg.extend_from_slice(b"GRIB");
        msg.extend_from_slice(&[0, 0, 0, 2]); // reserved, reserved, discipline, edition
        let len_pos = msg.len();
        msg.extend_from_slice(&[0u8; 8]); // total length placeholder
        // Section 5 (length 21)
        msg.extend_from_slice(&21u32.to_be_bytes());
        msg.push(5);
        msg.extend_from_slice(&4u32.to_be_bytes());
        msg.extend_from_slice(&200u16.to_be_bytes());
        msg.extend_from_slice(&0f32.to_be_bytes());
        msg.extend_from_slice(&0i16.to_be_bytes());
        msg.extend_from_slice(&0i16.to_be_bytes());
        msg.push(8);
        msg.extend_from_slice(&[0u8; 1]); // pad to length 21
        // Section 7
        msg.extend_from_slice(&9u32.to_be_bytes());
        msg.push(7);
        msg.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        msg.extend_from_slice(b"7777");
        let total_len = msg.len() as u64;
        msg[len_pos..len_pos + 8].copy_from_slice(&total_len.to_be_bytes());

        let parsed = parse_grib_message(&msg).expect("parse should not fail");
        assert_eq!(parsed.data_template, 200);
        assert!(parsed.data_values.is_none(), "values should be None for template 200");
    }

    #[test]
    fn test_unpack_round_trip_10_elements() {
        // Pack 10 known values with simple packing (R=0, E=0, D=2, nbits=16),
        // then run unpacker and verify ≤ 1e-6 deviation per element.
        let values: Vec<f64> = vec![1.23, 4.56, 7.89, 0.01, 2.50, 99.99, 50.00, 33.33, 12.34, 0.00];
        let mut packed: Vec<u8> = Vec::new();
        for v in &values {
            packed.extend_from_slice(&((v * 100.0).round() as u16).to_be_bytes());
        }
        let s5 = drep(16, 0.0, 0, 2, values.len() as u32);
        let out = unpack_template_5_0(&s5, &packed, values.len()).unwrap();
        for (orig, rec) in values.iter().zip(out.iter()) {
            assert!((orig - rec).abs() <= 1e-6, "round-trip mismatch: {} vs {}", orig, rec);
        }
    }

    #[test]
    fn test_unpack_no_panic_on_malformed_inputs() {
        let s5 = drep(8, 0.0, 0, 0, 4);
        assert!(unpack_template_5_0(&s5, &[], 4).is_err());
        let s5b = drep(8, 0.0, 0, 0, 0);
        assert!(unpack_template_5_0(&s5b, &[], 0).unwrap().is_empty());
    }
}
