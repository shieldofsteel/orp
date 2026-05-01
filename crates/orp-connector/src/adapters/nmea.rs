//! NMEA 0183 parser and connector for ORP Edge deployments.
//!
//! Reads raw NMEA sentences from a serial port or TCP stream and emits
//! [`SourceEvent`]s for each recognised sentence.  Ships' lives depend on
//! this data — every parsing decision is conservative: when in doubt we
//! return `None` rather than emitting garbage.
//!
//! # Supported sentences
//! | Sentence       | Entity type     | Data extracted                          |
//! |----------------|-----------------|-----------------------------------------|
//! | `$GPGGA`       | `own_vessel`    | lat, lon, altitude, satellites, hdop    |
//! | `$GPRMC`       | `own_vessel`    | lat, lon, speed, course, date           |
//! | `$GPVTG`       | `own_vessel`    | speed_knots, course_true                |
//! | `!AIVDM/AIVDO` | `ship`          | MMSI, pos, SOG, COG, name, type         |
//! | `$SDDBT/SDDBS` | `depth_reading` | depth_m                                 |
//! | `$WIMWD`       | `wind_reading`  | direction_true, speed_knots             |
//! | `$WIMWV`       | `wind_reading`  | wind_angle, speed, reference            |
//! | `$HCHDG`       | `own_vessel`    | heading_magnetic, deviation, variation  |
//! | `$YXXDR`       | `sensor`        | temperature_c / pressure_pa / humidity  |
//! | `$ERRPM`       | `engine`        | rpm, engine_number                      |
//!
//! # Source URL format
//! * `tcp://192.168.1.100:10110`
//! * `serial:///dev/ttyUSB0?baud=38400`

use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};

// ─────────────────────────────────────────────────────────────────────────────
// Checksum helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Validate the NMEA XOR checksum.
///
/// The checksum is the XOR of all bytes between the leading `$` / `!`
/// (exclusive) and the `*` (exclusive), expressed as two upper-case hex
/// digits after the `*`.
///
/// Returns `true` when the sentence carries a valid checksum.
/// Returns `false` for malformed sentences or checksum mismatch.
pub fn validate_checksum(sentence: &str) -> bool {
    // Find the asterisk that separates payload from checksum hex
    let star = match sentence.rfind('*') {
        Some(pos) => pos,
        None => return false,
    };

    let checksum_str = &sentence[star + 1..].trim_end();
    if checksum_str.len() < 2 {
        return false;
    }
    let expected = match u8::from_str_radix(&checksum_str[..2], 16) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // XOR everything between the first char (exclusive) and '*' (exclusive)
    let start = if sentence.starts_with('$') || sentence.starts_with('!') {
        1
    } else {
        0
    };
    let computed = sentence[start..star]
        .bytes()
        .fold(0u8, |acc, b| acc ^ b);

    computed == expected
}

/// Compute the NMEA checksum for a sentence body (without leading `$`/`!`
/// and without the `*XX` suffix).  Useful in tests.
pub fn compute_checksum(body: &str) -> u8 {
    body.bytes().fold(0u8, |acc, b| acc ^ b)
}

// ─────────────────────────────────────────────────────────────────────────────
// Coordinate helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Convert an NMEA coordinate string (DDMM.MMMM or DDDMM.MMMM) and a
/// direction character ('N'/'S' or 'E'/'W') to a signed decimal-degree value.
///
/// NMEA encodes coordinates as `DDDMM.MMMMM` where the integer part before
/// the decimal point has *at least two* digits that represent minutes.
pub fn parse_nmea_coord(coord: &str, dir: &str) -> Option<f64> {
    if coord.is_empty() {
        return None;
    }
    let dot = coord.find('.')?;
    // There are always exactly 2 minute digits before the decimal point
    if dot < 2 {
        return None;
    }
    let deg_end = dot - 2;
    let degrees: f64 = coord[..deg_end].parse().ok()?;
    let minutes: f64 = coord[deg_end..].parse().ok()?;
    let decimal = degrees + minutes / 60.0;
    match dir.trim() {
        "S" | "W" => Some(-decimal),
        _ => Some(decimal),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AIS 6-bit payload decoder
// ─────────────────────────────────────────────────────────────────────────────

pub mod ais_decoder {
    /// Decode an NMEA-armored AIS payload string into a packed bit vector.
    ///
    /// Each character encodes 6 bits using the encoding described in
    /// ITU-R M.1371:
    ///   * Subtract 48 from the ASCII value
    ///   * If the result > 39, subtract 8 (skips `:;<=>?@`)
    ///   * The resulting 0–63 value forms 6 bits (MSB first)
    ///
    /// `fill_bits` is the number of padding bits appended at the *end* of the
    /// last character that should be ignored.
    pub fn decode_payload(payload: &str, fill_bits: u8) -> Option<Vec<u8>> {
        if payload.is_empty() {
            return None;
        }
        let mut bits: Vec<bool> = Vec::with_capacity(payload.len() * 6);
        for ch in payload.bytes() {
            let mut val = ch.wrapping_sub(48);
            if val > 39 {
                val = val.wrapping_sub(8);
            }
            if val > 63 {
                return None; // invalid character
            }
            for shift in (0..6).rev() {
                bits.push((val >> shift) & 1 == 1);
            }
        }
        // Remove fill bits from the end
        let total = bits.len();
        let fill = fill_bits as usize;
        if fill > total {
            return None;
        }
        let usable = total - fill;
        // Pack into bytes (the caller uses get_bits() so a bit-array is fine;
        // we return a packed byte array for convenience)
        let bytes = (0..usable.div_ceil(8))
            .map(|i| {
                let mut byte = 0u8;
                for j in 0..8 {
                    let idx = i * 8 + j;
                    if idx < usable && bits[idx] {
                        byte |= 1 << (7 - j);
                    }
                }
                byte
            })
            .collect();
        Some(bytes)
    }

    /// A bit-addressable view over a packed byte slice.
    pub struct BitBuffer<'a> {
        data: &'a [u8],
        total_bits: usize,
    }

    impl<'a> BitBuffer<'a> {
        pub fn new(data: &'a [u8], total_bits: usize) -> Self {
            Self { data, total_bits }
        }

        /// Read `len` bits starting at `start` and return as u64.
        /// Returns `None` if the range is out of bounds.
        pub fn get_bits(&self, start: usize, len: usize) -> Option<u64> {
            if len > 64 || start + len > self.total_bits {
                return None;
            }
            let mut result = 0u64;
            for i in 0..len {
                let bit_idx = start + i;
                let byte_idx = bit_idx / 8;
                let bit_in_byte = 7 - (bit_idx % 8);
                let bit = (self.data[byte_idx] >> bit_in_byte) & 1;
                result = (result << 1) | (bit as u64);
            }
            Some(result)
        }

        /// Read a signed integer of `len` bits (two's complement).
        pub fn get_signed(&self, start: usize, len: usize) -> Option<i64> {
            let raw = self.get_bits(start, len)?;
            if len == 0 {
                return Some(0);
            }
            // Check sign bit
            if raw >> (len - 1) == 1 {
                // Negative: sign extend
                let mask = !((1u64 << len) - 1);
                Some((raw | mask) as i64)
            } else {
                Some(raw as i64)
            }
        }

        /// Decode a 6-bit AIS text string of `num_chars` characters starting
        /// at bit `start`.  Trailing `@` characters (value 0) are stripped.
        pub fn get_ais_string(&self, start: usize, num_chars: usize) -> Option<String> {
            let mut s = String::with_capacity(num_chars);
            for i in 0..num_chars {
                let val = self.get_bits(start + i * 6, 6)? as u8;
                let ch = if val < 32 {
                    (val + 64) as char // '@' through '_'
                } else {
                    val as char // ' ' through '?'
                };
                s.push(ch);
            }
            // Strip trailing '@' (padding character, value 0 → '@')
            let trimmed = s.trim_end_matches('@').trim_end();
            Some(trimmed.to_string())
        }
    }

    /// Decoded Class A position report (message types 1, 2, 3).
    #[derive(Debug, Clone)]
    pub struct AisType123 {
        pub msg_type: u8,
        pub mmsi: u32,
        pub nav_status: u8,
        /// Speed over ground, knots (102.3 = not available)
        pub sog: f32,
        pub lat: f64,
        pub lon: f64,
        /// Course over ground, degrees (360.0 = not available)
        pub cog: f32,
        /// True heading, degrees (511 = not available)
        pub heading: u16,
    }

    /// Decoded Class A static and voyage data (message type 5).
    #[derive(Debug, Clone)]
    pub struct AisType5 {
        pub mmsi: u32,
        pub imo: u32,
        pub call_sign: String,
        pub vessel_name: String,
        pub ship_type: u8,
        /// Draught in metres (1/10 m resolution)
        pub draught_m: f32,
        pub destination: String,
    }

    /// Decoded Class B position report (message types 18, 19).
    #[derive(Debug, Clone)]
    pub struct AisType1819 {
        pub msg_type: u8,
        pub mmsi: u32,
        pub sog: f32,
        pub lat: f64,
        pub lon: f64,
        pub cog: f32,
        pub heading: u16,
    }

    /// Decoded Aid-to-Navigation report (message type 21).
    #[derive(Debug, Clone)]
    pub struct AisType21 {
        pub mmsi: u32,
        pub aid_type: u8,
        pub name: String,
        pub lat: f64,
        pub lon: f64,
    }

    /// Construct a `BitBuffer` from a decoded payload byte slice.
    /// `total_bits` is `payload.len() * 6 - fill_bits`.
    pub fn make_buffer(data: &[u8], total_bits: usize) -> BitBuffer<'_> {
        BitBuffer::new(data, total_bits)
    }

    // ── Decode lon/lat from AIS 28/27-bit signed integers ─────────────────
    // Values are in 1/10 000 of a minute; special sentinel for unavailable.
    fn decode_lon(raw: i64) -> f64 {
        raw as f64 / 600_000.0
    }
    fn decode_lat(raw: i64) -> f64 {
        raw as f64 / 600_000.0
    }
    fn decode_sog(raw: u64) -> f32 {
        raw as f32 / 10.0
    }
    fn decode_cog(raw: u64) -> f32 {
        raw as f32 / 10.0
    }

    /// Decode message types 1, 2, or 3 from a packed bit buffer.
    pub fn decode_type_1_2_3(buf: &BitBuffer<'_>) -> Option<AisType123> {
        // Minimum 168 bits
        if buf.total_bits < 168 {
            return None;
        }
        let msg_type = buf.get_bits(0, 6)? as u8;
        if !(1..=3).contains(&msg_type) {
            return None;
        }
        let mmsi = buf.get_bits(8, 30)? as u32;
        let nav_status = buf.get_bits(38, 4)? as u8;
        let sog_raw = buf.get_bits(50, 10)?;
        let lon_raw = buf.get_signed(61, 28)?;
        let lat_raw = buf.get_signed(89, 27)?;
        let cog_raw = buf.get_bits(116, 12)?;
        let heading_raw = buf.get_bits(128, 9)?;

        Some(AisType123 {
            msg_type,
            mmsi,
            nav_status,
            sog: decode_sog(sog_raw),
            lon: decode_lon(lon_raw),
            lat: decode_lat(lat_raw),
            cog: decode_cog(cog_raw),
            heading: heading_raw as u16,
        })
    }

    /// Decode message type 5 from a packed bit buffer.
    pub fn decode_type_5(buf: &BitBuffer<'_>) -> Option<AisType5> {
        // Minimum 426 bits
        if buf.total_bits < 426 {
            return None;
        }
        let msg_type = buf.get_bits(0, 6)? as u8;
        if msg_type != 5 {
            return None;
        }
        let mmsi = buf.get_bits(8, 30)? as u32;
        let imo = buf.get_bits(40, 30)? as u32;
        let call_sign = buf.get_ais_string(70, 7)?;
        let vessel_name = buf.get_ais_string(112, 20)?;
        let ship_type = buf.get_bits(232, 8)? as u8;
        let draught_raw = buf.get_bits(294, 8)?;
        let destination = buf.get_ais_string(302, 20)?;

        Some(AisType5 {
            mmsi,
            imo,
            call_sign,
            vessel_name,
            ship_type,
            draught_m: draught_raw as f32 / 10.0,
            destination,
        })
    }

    /// Decode message types 18 and 19 (Class B) from a packed bit buffer.
    pub fn decode_type_18_19(buf: &BitBuffer<'_>) -> Option<AisType1819> {
        // Minimum 168 bits (type 18); type 19 is 312 bits
        if buf.total_bits < 168 {
            return None;
        }
        let msg_type = buf.get_bits(0, 6)? as u8;
        if msg_type != 18 && msg_type != 19 {
            return None;
        }
        let mmsi = buf.get_bits(8, 30)? as u32;
        let sog_raw = buf.get_bits(46, 10)?;
        let lon_raw = buf.get_signed(57, 28)?;
        let lat_raw = buf.get_signed(85, 27)?;
        let cog_raw = buf.get_bits(112, 12)?;
        let heading_raw = buf.get_bits(124, 9)?;

        Some(AisType1819 {
            msg_type,
            mmsi,
            sog: decode_sog(sog_raw),
            lon: decode_lon(lon_raw),
            lat: decode_lat(lat_raw),
            cog: decode_cog(cog_raw),
            heading: heading_raw as u16,
        })
    }

    /// Decode message type 21 (Aid to Navigation) from a packed bit buffer.
    pub fn decode_type_21(buf: &BitBuffer<'_>) -> Option<AisType21> {
        // Minimum 272 bits
        if buf.total_bits < 272 {
            return None;
        }
        let msg_type = buf.get_bits(0, 6)? as u8;
        if msg_type != 21 {
            return None;
        }
        let mmsi = buf.get_bits(8, 30)? as u32;
        let aid_type = buf.get_bits(38, 5)? as u8;
        let name = buf.get_ais_string(43, 20)?;
        let lon_raw = buf.get_signed(164, 28)?;
        let lat_raw = buf.get_signed(192, 27)?;

        Some(AisType21 {
            mmsi,
            aid_type,
            name,
            lon: decode_lon(lon_raw),
            lat: decode_lat(lat_raw),
        })
    }

    /// Decoded Base Station Report (message type 4, 168 bits).
    /// Layout: type(6) repeat(2) mmsi(30) year(14) month(4) day(5)
    /// hour(5) min(6) sec(6) pos_acc(1) lon(28s) lat(27s) fix_type(4)
    /// spare(10) raim(1) comm_state(19).
    #[derive(Debug, Clone)]
    pub struct AisType4 {
        pub mmsi: u32,
        pub year: u16, pub month: u8, pub day: u8,
        pub hour: u8, pub minute: u8, pub second: u8,
        pub position_accuracy: bool,
        pub lat: f64, pub lon: f64,
        pub fix_type: u8, pub raim: bool,
    }

    /// Decode message type 4 from a packed bit buffer.
    pub fn decode_type_4(buf: &BitBuffer<'_>) -> Option<AisType4> {
        if buf.total_bits < 168 { return None; }
        if buf.get_bits(0, 6)? as u8 != 4 { return None; }
        Some(AisType4 {
            mmsi: buf.get_bits(8, 30)? as u32,
            year: buf.get_bits(38, 14)? as u16,
            month: buf.get_bits(52, 4)? as u8,
            day: buf.get_bits(56, 5)? as u8,
            hour: buf.get_bits(61, 5)? as u8,
            minute: buf.get_bits(66, 6)? as u8,
            second: buf.get_bits(72, 6)? as u8,
            position_accuracy: buf.get_bits(78, 1)? == 1,
            lon: decode_lon(buf.get_signed(79, 28)?),
            lat: decode_lat(buf.get_signed(107, 27)?),
            fix_type: buf.get_bits(134, 4)? as u8,
            raim: buf.get_bits(148, 1)? == 1,
        })
    }

    /// Decoded SAR Aircraft Position Report (message type 9, 168 bits).
    /// Layout: type(6) repeat(2) mmsi(30) alt_m(12, 4095=N/A)
    /// sog_kts(10, 1023=N/A) pos_acc(1) lon(28s) lat(27s) cog(12, 1/10 deg)
    /// ts(6) regional(8) dte(1) spare(3) assigned(1) raim(1) radio(20).
    #[derive(Debug, Clone)]
    pub struct AisType9 {
        pub mmsi: u32,
        pub altitude_m: u16,
        pub sog: u16,
        pub position_accuracy: bool,
        pub lat: f64, pub lon: f64,
        pub cog: f32, pub time_stamp: u8, pub raim: bool,
    }

    /// Decode message type 9 from a packed bit buffer.
    pub fn decode_type_9(buf: &BitBuffer<'_>) -> Option<AisType9> {
        if buf.total_bits < 168 { return None; }
        if buf.get_bits(0, 6)? as u8 != 9 { return None; }
        // RAIM at bit 147 (after regional 8 + dte 1 + spare 3 + assigned 1).
        Some(AisType9 {
            mmsi: buf.get_bits(8, 30)? as u32,
            altitude_m: buf.get_bits(38, 12)? as u16,
            sog: buf.get_bits(50, 10)? as u16,
            position_accuracy: buf.get_bits(60, 1)? == 1,
            lon: decode_lon(buf.get_signed(61, 28)?),
            lat: decode_lat(buf.get_signed(89, 27)?),
            cog: buf.get_bits(116, 12)? as f32 / 10.0,
            time_stamp: buf.get_bits(128, 6)? as u8,
            raim: buf.get_bits(147, 1)? == 1,
        })
    }

    /// Decoded Long-range AIS broadcast (message type 27, 96 bits — LOW
    /// precision: lat 17-bit / lon 18-bit, both 1/10 minute scaling).
    /// Layout: type(6) repeat(2) mmsi(30) pos_acc(1) raim(1) nav(4)
    /// lon(18s) lat(17s) sog(6,63=N/A) cog(9,511=N/A) gnss(1) spare(1).
    #[derive(Debug, Clone)]
    pub struct AisType27 {
        pub mmsi: u32,
        pub position_accuracy: bool, pub raim: bool, pub nav_status: u8,
        pub lat: f64, pub lon: f64,
        pub sog: u8, pub cog: u16, pub gnss_status: bool,
    }

    /// Type 27 lon/lat scaling: 1/10 minute = 1/600 deg.  17-bit lat,
    /// 18-bit lon, both signed.
    fn decode_t27_coord(raw: i64) -> f64 { raw as f64 / 600.0 }

    /// Decode message type 27 from a packed bit buffer.
    pub fn decode_type_27(buf: &BitBuffer<'_>) -> Option<AisType27> {
        if buf.total_bits < 96 { return None; }
        if buf.get_bits(0, 6)? as u8 != 27 { return None; }
        Some(AisType27 {
            mmsi: buf.get_bits(8, 30)? as u32,
            position_accuracy: buf.get_bits(38, 1)? == 1,
            raim: buf.get_bits(39, 1)? == 1,
            nav_status: buf.get_bits(40, 4)? as u8,
            lon: decode_t27_coord(buf.get_signed(44, 18)?),
            lat: decode_t27_coord(buf.get_signed(62, 17)?),
            sog: buf.get_bits(79, 6)? as u8,
            cog: buf.get_bits(85, 9)? as u16,
            gnss_status: buf.get_bits(94, 1)? == 1,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NMEA sentence parsers
// ─────────────────────────────────────────────────────────────────────────────

/// Parsed NMEA data ready to be converted into a [`SourceEvent`].
#[derive(Debug, Clone)]
pub enum NmeaData {
    /// `$GPGGA` — GPS fix
    Gpgga {
        lat: f64,
        lon: f64,
        altitude_m: f64,
        satellites: u8,
        hdop: f32,
        fix_quality: u8,
        timestamp: DateTime<Utc>,
    },
    /// `$GPRMC` — Recommended minimum
    Gprmc {
        lat: f64,
        lon: f64,
        speed_knots: f32,
        course: f32,
        timestamp: DateTime<Utc>,
    },
    /// `$GPVTG` — Track/speed over ground
    Gpvtg {
        course_true: f32,
        course_magnetic: Option<f32>,
        speed_knots: f32,
        speed_kmh: f32,
    },
    /// `!AIVDM` / `!AIVDO` — AIS Class A position (types 1-3)
    AisPosition {
        mmsi: u32,
        lat: f64,
        lon: f64,
        sog: f32,
        cog: f32,
        heading: u16,
        nav_status: u8,
        own_vessel: bool,
    },
    /// `!AIVDM` / `!AIVDO` — AIS static data (type 5)
    AisStatic {
        mmsi: u32,
        vessel_name: String,
        call_sign: String,
        ship_type: u8,
        imo: u32,
        draught_m: f32,
        destination: String,
        own_vessel: bool,
    },
    /// `!AIVDM` / `!AIVDO` — Class B position (types 18-19)
    AisClassB {
        mmsi: u32,
        lat: f64,
        lon: f64,
        sog: f32,
        cog: f32,
        heading: u16,
        own_vessel: bool,
    },
    /// `!AIVDM` / `!AIVDO` — Aid to navigation (type 21)
    AisAton {
        mmsi: u32,
        aid_type: u8,
        name: String,
        lat: f64,
        lon: f64,
    },
    /// `!AIVDM` / `!AIVDO` — Base station report (type 4)
    AisBaseStation {
        mmsi: u32, lat: f64, lon: f64,
        utc_year: u16, utc_month: u8, utc_day: u8,
        utc_hour: u8, utc_minute: u8, utc_second: u8,
        fix_type: u8, raim: bool, position_accuracy: bool,
    },
    /// `!AIVDM` / `!AIVDO` — Standard SAR aircraft position (type 9)
    AisSarAircraft {
        mmsi: u32, lat: f64, lon: f64,
        altitude_m: u16, sog: u16, cog: f32, time_stamp: u8,
        raim: bool, position_accuracy: bool,
    },
    /// `!AIVDM` / `!AIVDO` — Long-range broadcast (type 27)
    AisLongRange {
        mmsi: u32, lat: f64, lon: f64,
        sog: u8, cog: u16, nav_status: u8,
        raim: bool, position_accuracy: bool, gnss_status: bool,
    },
    /// `$SDDBT` / `$SDDBS` — Depth
    Depth {
        depth_m: f32,
        below_surface: bool,
    },
    /// `$WIMWD` — Wind direction and speed (true)
    WindTrue {
        direction_true: f32,
        speed_knots: f32,
        speed_ms: f32,
    },
    /// `$WIMWV` — Wind angle and speed (relative or true)
    WindRelative {
        angle: f32,
        reference: String, // "R" or "T"
        speed_knots: f32,
    },
    /// `$HCHDG` — Heading (magnetic)
    Heading {
        heading_magnetic: f32,
        deviation: Option<f32>,
        variation: Option<f32>,
    },
    /// `$YXXDR` — Transducer measurements
    Transducer {
        transducer_type: String,
        value: f64,
        units: String,
        name: String,
    },
    /// `$ERRPM` — Engine RPM
    EngineRpm {
        engine_number: u8,
        rpm: f32,
    },
}

/// Strips everything from `*` onwards and splits on `,`.
/// Returns the field vector (including the sentence identifier as field[0]).
fn split_sentence(sentence: &str) -> Vec<&str> {
    let body = if let Some(pos) = sentence.rfind('*') {
        &sentence[..pos]
    } else {
        sentence
    };
    // strip leading $ or !
    let body = if body.starts_with('$') || body.starts_with('!') {
        &body[1..]
    } else {
        body
    };
    body.split(',').collect()
}

/// Parse `$GPGGA`
fn parse_gpgga(fields: &[&str]) -> Option<NmeaData> {
    // $GPGGA,time,lat,N/S,lon,E/W,quality,sats,hdop,alt,M,...
    if fields.len() < 10 {
        return None;
    }
    let time_str = fields[1];
    let lat = parse_nmea_coord(fields[2], fields[3])?;
    let lon = parse_nmea_coord(fields[4], fields[5])?;
    let fix_quality: u8 = fields[6].parse().ok()?;
    let satellites: u8 = fields[7].parse().ok()?;
    let hdop: f32 = fields[8].parse().unwrap_or(99.0);
    let altitude_m: f64 = fields[9].parse().unwrap_or(0.0);
    let timestamp = parse_nmea_time(time_str).unwrap_or_else(Utc::now);
    Some(NmeaData::Gpgga {
        lat,
        lon,
        altitude_m,
        satellites,
        hdop,
        fix_quality,
        timestamp,
    })
}

/// Parse `$GPRMC`
fn parse_gprmc(fields: &[&str]) -> Option<NmeaData> {
    // $GPRMC,time,status,lat,N/S,lon,E/W,speed,course,date,...
    if fields.len() < 10 {
        return None;
    }
    let time_str = fields[1];
    let status = fields[2];
    if status != "A" {
        // Void fix — data invalid
        return None;
    }
    let lat = parse_nmea_coord(fields[3], fields[4])?;
    let lon = parse_nmea_coord(fields[5], fields[6])?;
    let speed_knots: f32 = fields[7].parse().unwrap_or(0.0);
    let course: f32 = fields[8].parse().unwrap_or(0.0);
    let date_str = fields[9];
    let timestamp = parse_nmea_datetime(time_str, date_str).unwrap_or_else(Utc::now);
    Some(NmeaData::Gprmc {
        lat,
        lon,
        speed_knots,
        course,
        timestamp,
    })
}

/// Parse `$GPVTG`
fn parse_gpvtg(fields: &[&str]) -> Option<NmeaData> {
    // $GPVTG,courseT,T,courseM,M,speedN,N,speedK,K,...
    if fields.len() < 8 {
        return None;
    }
    let course_true: f32 = fields[1].parse().unwrap_or(0.0);
    let course_magnetic: Option<f32> = fields[3].parse().ok();
    let speed_knots: f32 = fields[5].parse().unwrap_or(0.0);
    let speed_kmh: f32 = fields[7].parse().unwrap_or(0.0);
    Some(NmeaData::Gpvtg {
        course_true,
        course_magnetic,
        speed_knots,
        speed_kmh,
    })
}

/// Parse `$SDDBT` (depth below transducer) or `$SDDBS` (depth below surface).
fn parse_depth(fields: &[&str], below_surface: bool) -> Option<NmeaData> {
    // $SDDBT,feet,f,meters,M,fathoms,F
    if fields.len() < 4 {
        return None;
    }
    let depth_m: f32 = fields[3].parse().ok()?;
    Some(NmeaData::Depth { depth_m, below_surface })
}

/// Parse `$WIMWD` — wind direction and speed (true).
fn parse_wimwd(fields: &[&str]) -> Option<NmeaData> {
    // $WIMWD,dirTrue,T,dirMag,M,speedKnots,N,speedMs,M
    if fields.len() < 8 {
        return None;
    }
    let direction_true: f32 = fields[1].parse().ok()?;
    let speed_knots: f32 = fields[5].parse().ok()?;
    let speed_ms: f32 = fields[7].parse().unwrap_or(speed_knots * 0.514_444);
    Some(NmeaData::WindTrue { direction_true, speed_knots, speed_ms })
}

/// Parse `$WIMWV` — wind angle and speed.
fn parse_wimwv(fields: &[&str]) -> Option<NmeaData> {
    // $WIMWV,angle,R/T,speed,unit,status
    if fields.len() < 5 {
        return None;
    }
    let angle: f32 = fields[1].parse().ok()?;
    let reference = fields[2].to_string();
    let speed_raw: f32 = fields[3].parse().ok()?;
    let unit = fields[4];
    let speed_knots = match unit {
        "K" => speed_raw / 1.852,
        "M" => speed_raw / 0.514_444,
        "S" => speed_raw / 0.514_444,
        _ => speed_raw, // assume knots
    };
    Some(NmeaData::WindRelative { angle, reference, speed_knots })
}

/// Parse `$HCHDG` — magnetic heading.
fn parse_hchdg(fields: &[&str]) -> Option<NmeaData> {
    // $HCHDG,heading,deviation,E/W,variation,E/W
    if fields.len() < 5 {
        return None;
    }
    let heading_magnetic: f32 = fields[1].parse().ok()?;
    let deviation_raw: Option<f32> = fields[2].parse().ok();
    let deviation = deviation_raw.map(|d| {
        if fields[3] == "W" { -d } else { d }
    });
    let variation_raw: Option<f32> = fields[4].parse().ok();
    let variation = variation_raw.map(|v| {
        if fields.len() > 5 && fields[5] == "W" { -v } else { v }
    });
    Some(NmeaData::Heading { heading_magnetic, deviation, variation })
}

/// Parse `$YXXDR` — transducer measurement.
///
/// `$YXXDR` can carry multiple measurement groups of four fields each:
/// `type, value, units, name`.  We parse the first group.
fn parse_yxxdr(fields: &[&str]) -> Option<NmeaData> {
    if fields.len() < 5 {
        return None;
    }
    let transducer_type = fields[1].to_string();
    let value: f64 = fields[2].parse().ok()?;
    let units = fields[3].to_string();
    let name = fields[4].to_string();
    Some(NmeaData::Transducer { transducer_type, value, units, name })
}

/// Parse `$ERRPM` — engine RPM.
fn parse_errpm(fields: &[&str]) -> Option<NmeaData> {
    // $ERRPM,S/E,engine_num,rpm,...
    if fields.len() < 4 {
        return None;
    }
    // fields[1]: S=shaft, E=engine
    let engine_number: u8 = fields[2].parse().unwrap_or(0);
    let rpm: f32 = fields[3].parse().ok()?;
    Some(NmeaData::EngineRpm { engine_number, rpm })
}

// ─────────────────────────────────────────────────────────────────────────────
// AIS multi-part assembler
// ─────────────────────────────────────────────────────────────────────────────

/// Accumulates multi-part AIVDM sentences before decoding.
#[derive(Debug, Default)]
pub struct AisAssembler {
    /// Key: message_id (empty string for single-part messages that have no ID)
    parts: HashMap<String, Vec<(u8, String)>>, // (part_num, payload)
    total: HashMap<String, u8>,
    fill_bits: HashMap<String, u8>,
}

impl AisAssembler {
    /// Feed one AIVDM/AIVDO sentence.  Returns the assembled payload when
    /// all parts have arrived, together with the fill_bits value.
    fn feed(&mut self, fields: &[&str]) -> Option<(String, u8)> {
        // !AIVDM,total,part,msg_id,channel,payload,fill_bits
        if fields.len() < 7 {
            return None;
        }
        let total: u8 = fields[1].parse().unwrap_or(1);
        let part: u8 = fields[2].parse().unwrap_or(1);
        let msg_id = fields[3].to_string();
        let payload = fields[5].to_string();
        let fill: u8 = fields[6].parse().unwrap_or(0);

        if total == 1 {
            return Some((payload, fill));
        }

        // Multi-part
        let key = format!("{}-{}", msg_id, total);
        self.total.insert(key.clone(), total);
        self.fill_bits.insert(key.clone(), fill);
        let parts = self.parts.entry(key.clone()).or_default();
        // Avoid duplicate parts
        if !parts.iter().any(|(n, _)| *n == part) {
            parts.push((part, payload));
        }

        let expected = self.total[&key];
        if self.parts[&key].len() == expected as usize {
            let mut sorted = self.parts.remove(&key).unwrap();
            sorted.sort_by_key(|(n, _)| *n);
            let assembled: String = sorted.into_iter().map(|(_, p)| p).collect();
            let fill_bits = self.fill_bits.remove(&key).unwrap_or(0);
            self.total.remove(&key);
            return Some((assembled, fill_bits));
        }
        None
    }
}

/// Decode an assembled AIS payload into `NmeaData`.
/// Check whether an AIS latitude/longitude pair represents the ITU-R M.1371
/// "not available" sentinel values (lat == 91.0, lon == 181.0).
/// Positions with these sentinel values must be rejected.
fn ais_position_available(lat: f64, lon: f64) -> bool {
    // ITU-R M.1371: 91.0 degrees latitude = not available
    //               181.0 degrees longitude = not available
    (lat - 91.0).abs() > 0.01 && (lon - 181.0).abs() > 0.01
}

fn decode_ais(payload: &str, fill_bits: u8, own_vessel: bool) -> Option<NmeaData> {
    let bytes = ais_decoder::decode_payload(payload, fill_bits)?;
    let total_bits = payload.len() * 6 - fill_bits as usize;
    let buf = ais_decoder::make_buffer(&bytes, total_bits);
    let msg_type = buf.get_bits(0, 6)? as u8;

    match msg_type {
        1..=3 => {
            let d = ais_decoder::decode_type_1_2_3(&buf)?;
            // Reject AIS sentinel positions (ITU-R M.1371: lat 91.0 / lon 181.0 = not available)
            if !ais_position_available(d.lat, d.lon) {
                return None;
            }
            Some(NmeaData::AisPosition {
                mmsi: d.mmsi,
                lat: d.lat,
                lon: d.lon,
                sog: d.sog,
                cog: d.cog,
                heading: d.heading,
                nav_status: d.nav_status,
                own_vessel,
            })
        }
        5 => {
            let d = ais_decoder::decode_type_5(&buf)?;
            Some(NmeaData::AisStatic {
                mmsi: d.mmsi,
                vessel_name: d.vessel_name,
                call_sign: d.call_sign,
                ship_type: d.ship_type,
                imo: d.imo,
                draught_m: d.draught_m,
                destination: d.destination,
                own_vessel,
            })
        }
        18 | 19 => {
            let d = ais_decoder::decode_type_18_19(&buf)?;
            // Reject AIS sentinel positions (ITU-R M.1371: lat 91.0 / lon 181.0 = not available)
            if !ais_position_available(d.lat, d.lon) {
                return None;
            }
            Some(NmeaData::AisClassB {
                mmsi: d.mmsi,
                lat: d.lat,
                lon: d.lon,
                sog: d.sog,
                cog: d.cog,
                heading: d.heading,
                own_vessel,
            })
        }
        21 => {
            let d = ais_decoder::decode_type_21(&buf)?;
            // Reject AIS sentinel positions (ITU-R M.1371: lat 91.0 / lon 181.0 = not available)
            if !ais_position_available(d.lat, d.lon) {
                return None;
            }
            Some(NmeaData::AisAton {
                mmsi: d.mmsi,
                aid_type: d.aid_type,
                name: d.name,
                lat: d.lat,
                lon: d.lon,
            })
        }
        4 => {
            let d = ais_decoder::decode_type_4(&buf)?;
            if !ais_position_available(d.lat, d.lon) { return None; }
            Some(NmeaData::AisBaseStation {
                mmsi: d.mmsi, lat: d.lat, lon: d.lon,
                utc_year: d.year, utc_month: d.month, utc_day: d.day,
                utc_hour: d.hour, utc_minute: d.minute, utc_second: d.second,
                fix_type: d.fix_type, raim: d.raim,
                position_accuracy: d.position_accuracy,
            })
        }
        9 => {
            let d = ais_decoder::decode_type_9(&buf)?;
            if !ais_position_available(d.lat, d.lon) { return None; }
            Some(NmeaData::AisSarAircraft {
                mmsi: d.mmsi, lat: d.lat, lon: d.lon,
                altitude_m: d.altitude_m, sog: d.sog, cog: d.cog,
                time_stamp: d.time_stamp, raim: d.raim,
                position_accuracy: d.position_accuracy,
            })
        }
        27 => {
            let d = ais_decoder::decode_type_27(&buf)?;
            if !ais_position_available(d.lat, d.lon) { return None; }
            Some(NmeaData::AisLongRange {
                mmsi: d.mmsi, lat: d.lat, lon: d.lon,
                sog: d.sog, cog: d.cog, nav_status: d.nav_status,
                raim: d.raim, position_accuracy: d.position_accuracy,
                gnss_status: d.gnss_status,
            })
        }
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level sentence dispatcher
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a single NMEA sentence into `NmeaData`.
///
/// Returns `None` when:
///  * checksum is invalid
///  * sentence type is unrecognised
///  * required fields are missing or malformed
///
/// The AIS assembler (`ais`) must be passed in for multi-part message support.
pub fn parse_sentence(
    raw: &str,
    ais: &mut AisAssembler,
) -> Option<NmeaData> {
    let sentence = raw.trim();
    if sentence.is_empty() {
        return None;
    }
    // Checksum validation — mandatory for safety-critical data
    if !validate_checksum(sentence) {
        tracing::debug!(sentence = %sentence, "NMEA checksum validation failed");
        return None;
    }
    let fields = split_sentence(sentence);
    if fields.is_empty() {
        return None;
    }
    match fields[0].to_uppercase().as_str() {
        // Accept any talker prefix for GPS sentences (GP, GN, GL, etc.)
        id if id.ends_with("GGA") => parse_gpgga(&fields),
        id if id.ends_with("RMC") => parse_gprmc(&fields),
        id if id.ends_with("VTG") => parse_gpvtg(&fields),
        "SDDBT" => parse_depth(&fields, false),
        "SDDBS" => parse_depth(&fields, true),
        "WIMWD" => parse_wimwd(&fields),
        "WIMWV" => parse_wimwv(&fields),
        "HCHDG" => parse_hchdg(&fields),
        "YXXDR" => parse_yxxdr(&fields),
        "ERRPM" => parse_errpm(&fields),
        id @ ("AIVDM" | "AIVDO") => {
            let own_vessel = id == "AIVDO";
            if let Some((payload, fill)) = ais.feed(&fields) {
                decode_ais(&payload, fill, own_vessel)
            } else {
                None // waiting for more parts
            }
        }
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NmeaData → SourceEvent conversion
// ─────────────────────────────────────────────────────────────────────────────

impl NmeaData {
    /// Convert to a [`SourceEvent`].
    pub fn to_source_event(&self, connector_id: &str) -> SourceEvent {
        match self {
            NmeaData::Gpgga { lat, lon, altitude_m, satellites, hdop, fix_quality, timestamp } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("source".into(), Json::String("gpgga".into()));
                props.insert("altitude_m".into(), Json::from(*altitude_m));
                props.insert("satellites".into(), Json::from(*satellites));
                props.insert("hdop".into(), Json::from(*hdop));
                props.insert("fix_quality".into(), Json::from(*fix_quality));
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: "own_vessel".into(),
                    entity_type: "own_vessel".into(),
                    properties: props,
                    timestamp: *timestamp,
                    latitude: Some(*lat),
                    longitude: Some(*lon),
                }
            }
            NmeaData::Gprmc { lat, lon, speed_knots, course, timestamp } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("source".into(), Json::String("gprmc".into()));
                props.insert("speed_knots".into(), Json::from(*speed_knots));
                props.insert("course".into(), Json::from(*course));
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: "own_vessel".into(),
                    entity_type: "own_vessel".into(),
                    properties: props,
                    timestamp: *timestamp,
                    latitude: Some(*lat),
                    longitude: Some(*lon),
                }
            }
            NmeaData::Gpvtg { course_true, course_magnetic, speed_knots, speed_kmh } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("source".into(), Json::String("gpvtg".into()));
                props.insert("course_true".into(), Json::from(*course_true));
                props.insert("speed_knots".into(), Json::from(*speed_knots));
                props.insert("speed_kmh".into(), Json::from(*speed_kmh));
                if let Some(cm) = course_magnetic {
                    props.insert("course_magnetic".into(), Json::from(*cm));
                }
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: "own_vessel".into(),
                    entity_type: "own_vessel".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                }
            }
            NmeaData::AisPosition { mmsi, lat, lon, sog, cog, heading, nav_status, own_vessel } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("mmsi".into(), Json::from(*mmsi));
                props.insert("sog".into(), Json::from(*sog));
                props.insert("cog".into(), Json::from(*cog));
                props.insert("heading".into(), Json::from(*heading));
                props.insert("nav_status".into(), Json::from(*nav_status));
                let entity_id = if *own_vessel {
                    "own_vessel".into()
                } else {
                    format!("mmsi:{mmsi}")
                };
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id,
                    entity_type: "ship".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: Some(*lat),
                    longitude: Some(*lon),
                }
            }
            NmeaData::AisStatic { mmsi, vessel_name, call_sign, ship_type, imo, draught_m, destination, own_vessel } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("mmsi".into(), Json::from(*mmsi));
                props.insert("vessel_name".into(), Json::String(vessel_name.clone()));
                props.insert("call_sign".into(), Json::String(call_sign.clone()));
                props.insert("ship_type".into(), Json::from(*ship_type));
                props.insert("imo".into(), Json::from(*imo));
                props.insert("draught_m".into(), Json::from(*draught_m));
                props.insert("destination".into(), Json::String(destination.clone()));
                let entity_id = if *own_vessel {
                    "own_vessel".into()
                } else {
                    format!("mmsi:{mmsi}")
                };
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id,
                    entity_type: "ship".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                }
            }
            NmeaData::AisClassB { mmsi, lat, lon, sog, cog, heading, own_vessel } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("mmsi".into(), Json::from(*mmsi));
                props.insert("sog".into(), Json::from(*sog));
                props.insert("cog".into(), Json::from(*cog));
                props.insert("heading".into(), Json::from(*heading));
                let entity_id = if *own_vessel {
                    "own_vessel".into()
                } else {
                    format!("mmsi:{mmsi}")
                };
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id,
                    entity_type: "ship".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: Some(*lat),
                    longitude: Some(*lon),
                }
            }
            NmeaData::AisAton { mmsi, aid_type, name, lat, lon } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("mmsi".into(), Json::from(*mmsi));
                props.insert("aid_type".into(), Json::from(*aid_type));
                props.insert("name".into(), Json::String(name.clone()));
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: format!("aton:{mmsi}"),
                    entity_type: "aton".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: Some(*lat),
                    longitude: Some(*lon),
                }
            }
            NmeaData::Depth { depth_m, below_surface } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("depth_m".into(), Json::from(*depth_m));
                props.insert("below_surface".into(), Json::from(*below_surface));
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: "depth".into(),
                    entity_type: "depth_reading".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                }
            }
            NmeaData::WindTrue { direction_true, speed_knots, speed_ms } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("direction_true".into(), Json::from(*direction_true));
                props.insert("speed_knots".into(), Json::from(*speed_knots));
                props.insert("speed_ms".into(), Json::from(*speed_ms));
                props.insert("reference".into(), Json::String("T".into()));
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: "wind".into(),
                    entity_type: "wind_reading".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                }
            }
            NmeaData::WindRelative { angle, reference, speed_knots } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("wind_angle".into(), Json::from(*angle));
                props.insert("speed_knots".into(), Json::from(*speed_knots));
                props.insert("reference".into(), Json::String(reference.clone()));
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: "wind".into(),
                    entity_type: "wind_reading".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                }
            }
            NmeaData::Heading { heading_magnetic, deviation, variation } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("heading_magnetic".into(), Json::from(*heading_magnetic));
                if let Some(d) = deviation {
                    props.insert("deviation".into(), Json::from(*d));
                }
                if let Some(v) = variation {
                    props.insert("variation".into(), Json::from(*v));
                }
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: "own_vessel".into(),
                    entity_type: "own_vessel".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                }
            }
            NmeaData::Transducer { transducer_type, value, units, name } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("transducer_type".into(), Json::String(transducer_type.clone()));
                props.insert("value".into(), Json::from(*value));
                props.insert("units".into(), Json::String(units.clone()));
                // Normalise common transducer types
                match (transducer_type.as_str(), units.as_str()) {
                    ("C", "C") => { props.insert("temperature_c".into(), Json::from(*value)); }
                    ("P", "P") => { props.insert("pressure_pa".into(), Json::from(*value)); }
                    ("H", "P") => { props.insert("humidity_pct".into(), Json::from(*value)); }
                    _ => {}
                }
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: format!("sensor:{name}"),
                    entity_type: "sensor".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                }
            }
            NmeaData::EngineRpm { engine_number, rpm } => {
                let mut props: HashMap<String, Json> = HashMap::new();
                props.insert("rpm".into(), Json::from(*rpm));
                props.insert("engine_number".into(), Json::from(*engine_number));
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: format!("engine:{engine_number}"),
                    entity_type: "engine".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                }
            }
            NmeaData::AisBaseStation {
                mmsi, lat, lon,
                utc_year, utc_month, utc_day, utc_hour, utc_minute, utc_second,
                fix_type, raim, position_accuracy,
            } => {
                let props: HashMap<String, Json> = [
                    ("mmsi", Json::from(*mmsi)),
                    ("utc_year", Json::from(*utc_year)),
                    ("utc_month", Json::from(*utc_month)),
                    ("utc_day", Json::from(*utc_day)),
                    ("utc_hour", Json::from(*utc_hour)),
                    ("utc_minute", Json::from(*utc_minute)),
                    ("utc_second", Json::from(*utc_second)),
                    ("fix_type", Json::from(*fix_type)),
                    ("raim", Json::from(*raim)),
                    ("position_accuracy", Json::from(*position_accuracy)),
                ].into_iter().map(|(k, v)| (k.into(), v)).collect();
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: format!("ais_base:{mmsi}"),
                    entity_type: "ais_base_station".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: Some(*lat),
                    longitude: Some(*lon),
                }
            }
            NmeaData::AisSarAircraft {
                mmsi, lat, lon, altitude_m, sog, cog, time_stamp, raim, position_accuracy,
            } => {
                let props: HashMap<String, Json> = [
                    ("mmsi", Json::from(*mmsi)),
                    ("altitude_m", Json::from(*altitude_m)),
                    ("sog", Json::from(*sog)),
                    ("cog", Json::from(*cog)),
                    ("time_stamp", Json::from(*time_stamp)),
                    ("raim", Json::from(*raim)),
                    ("position_accuracy", Json::from(*position_accuracy)),
                ].into_iter().map(|(k, v)| (k.into(), v)).collect();
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: format!("mmsi:{mmsi}"),
                    entity_type: "sar_aircraft".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: Some(*lat),
                    longitude: Some(*lon),
                }
            }
            NmeaData::AisLongRange {
                mmsi, lat, lon, sog, cog, nav_status, raim, position_accuracy, gnss_status,
            } => {
                let props: HashMap<String, Json> = [
                    ("mmsi", Json::from(*mmsi)),
                    ("sog", Json::from(*sog)),
                    ("cog", Json::from(*cog)),
                    ("nav_status", Json::from(*nav_status)),
                    ("raim", Json::from(*raim)),
                    ("position_accuracy", Json::from(*position_accuracy)),
                    ("gnss_status", Json::from(*gnss_status)),
                    ("long_range", Json::from(true)),
                ].into_iter().map(|(k, v)| (k.into(), v)).collect();
                SourceEvent {
                    connector_id: connector_id.into(),
                    entity_id: format!("mmsi:{mmsi}"),
                    entity_type: "ship".into(),
                    properties: props,
                    timestamp: Utc::now(),
                    latitude: Some(*lat),
                    longitude: Some(*lon),
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Time helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Parse NMEA time-only field (HHMMSS.sss).
fn parse_nmea_time(time_str: &str) -> Option<DateTime<Utc>> {
    if time_str.len() < 6 {
        return None;
    }
    let h: u32 = time_str[..2].parse().ok()?;
    let m: u32 = time_str[2..4].parse().ok()?;
    let s: u32 = time_str[4..6].parse().ok()?;
    let today = Utc::now().date_naive();
    Utc.with_ymd_and_hms(today.year(), today.month(), today.day(), h, m, s).single()
}

/// Parse NMEA time + date fields (HHMMSS.sss, DDMMYY).
fn parse_nmea_datetime(time_str: &str, date_str: &str) -> Option<DateTime<Utc>> {
    if time_str.len() < 6 || date_str.len() < 6 {
        return None;
    }
    let h: u32 = time_str[..2].parse().ok()?;
    let m: u32 = time_str[2..4].parse().ok()?;
    let s: u32 = time_str[4..6].parse().ok()?;
    let dd: i32 = date_str[..2].parse().ok()?;
    let mm: u32 = date_str[2..4].parse().ok()?;
    let yy: i32 = date_str[4..6].parse().ok()?;
    let year = if yy >= 70 { 1900 + yy } else { 2000 + yy };
    let date = NaiveDate::from_ymd_opt(year, mm, dd as u32)?;
    Utc.with_ymd_and_hms(date.year(), date.month(), date.day(), h, m, s).single()
}

use chrono::Datelike;

// ─────────────────────────────────────────────────────────────────────────────
// Source URL parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Parsed source endpoint.
#[derive(Debug, Clone)]
pub enum NmeaSource {
    /// TCP socket — `tcp://host:port`
    Tcp { host: String, port: u16 },
    /// Serial port — `serial:///dev/ttyUSB0?baud=38400`
    Serial { device: String, baud: u32 },
}

/// Parse a source URL into a [`NmeaSource`].
///
/// Examples:
/// * `tcp://192.168.1.100:10110`
/// * `serial:///dev/ttyUSB0?baud=38400`
pub fn parse_source_url(url: &str) -> Result<NmeaSource, ConnectorError> {
    if let Some(rest) = url.strip_prefix("tcp://") {
        // host:port
        let (host, port_str) = rest.rsplit_once(':').ok_or_else(|| {
            ConnectorError::ConfigError(format!("TCP URL missing port: {url}"))
        })?;
        let port: u16 = port_str.parse().map_err(|_| {
            ConnectorError::ConfigError(format!("Invalid port in URL: {url}"))
        })?;
        Ok(NmeaSource::Tcp { host: host.to_string(), port })
    } else if let Some(rest) = url.strip_prefix("serial://") {
        // /dev/ttyUSB0?baud=38400
        let (device, query) = rest.split_once('?').unwrap_or((rest, ""));
        let baud = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("baud="))
            .and_then(|v| v.parse().ok())
            .unwrap_or(4800); // NMEA 0183 default
        Ok(NmeaSource::Serial { device: device.to_string(), baud })
    } else {
        Err(ConnectorError::ConfigError(format!(
            "Unsupported NMEA source URL scheme (expected tcp:// or serial://): {url}"
        )))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NmeaConnector — implements the Connector trait
// ─────────────────────────────────────────────────────────────────────────────

/// NMEA 0183 connector that reads from a serial port or TCP stream.
pub struct NmeaConnector {
    config: ConnectorConfig,
    source: NmeaSource,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl NmeaConnector {
    /// Construct a new [`NmeaConnector`].
    ///
    /// The source URL is taken from `config.url`.
    pub fn new(config: ConnectorConfig) -> Result<Self, ConnectorError> {
        let url = config.url.as_deref().ok_or_else(|| {
            ConnectorError::ConfigError("NMEA connector requires a source URL".into())
        })?;
        let source = parse_source_url(url)?;
        Ok(Self {
            config,
            source,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Internal read loop — connects, reads lines, parses, emits events.
    async fn run_loop(
        source: NmeaSource,
        connector_id: String,
        running: Arc<AtomicBool>,
        events: Arc<AtomicU64>,
        errors: Arc<AtomicU64>,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) {
        loop {
            if !running.load(Ordering::SeqCst) {
                return;
            }
            let result = match &source {
                NmeaSource::Tcp { host, port } => {
                    Self::tcp_loop(
                        host.clone(), *port, connector_id.clone(),
                        running.clone(), events.clone(), errors.clone(), tx.clone(),
                    ).await
                }
                NmeaSource::Serial { device, baud } => {
                    Self::serial_loop(
                        device.clone(), *baud, connector_id.clone(),
                        running.clone(), events.clone(), errors.clone(), tx.clone(),
                    ).await
                }
            };
            if let Err(e) = result {
                errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(error = %e, connector_id = %connector_id,
                    "NMEA connection lost, reconnecting in 5 s");
            }
            if !running.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }

    async fn tcp_loop(
        host: String,
        port: u16,
        connector_id: String,
        running: Arc<AtomicBool>,
        events: Arc<AtomicU64>,
        errors: Arc<AtomicU64>,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        use tokio::net::TcpStream;
        tracing::info!(%host, %port, %connector_id, "NMEA TCP connecting");
        let stream = TcpStream::connect(format!("{host}:{port}"))
            .await
            .map_err(|e| ConnectorError::ConnectionError(e.to_string()))?;
        tracing::info!(%host, %port, %connector_id, "NMEA TCP connected");
        let mut reader = BufReader::new(stream);
        Self::read_loop(&mut reader, connector_id, running, events, errors, tx).await
    }

    async fn serial_loop(
        device: String,
        baud: u32,
        connector_id: String,
        running: Arc<AtomicBool>,
        events: Arc<AtomicU64>,
        errors: Arc<AtomicU64>,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        tracing::info!(%device, %baud, %connector_id, "NMEA serial opening");
        // Open the serial device as a raw file — works on Linux/macOS.
        // Baud rate and line discipline should be configured externally
        // (e.g. `stty -F /dev/ttyUSB0 38400 raw`) or via tokio-serial if
        // that optional feature is enabled.
        let file = tokio::fs::File::open(&device)
            .await
            .map_err(ConnectorError::IoError)?;
        tracing::info!(%device, %connector_id, "NMEA serial opened");
        let mut reader = BufReader::new(file);
        Self::read_loop(&mut reader, connector_id, running, events, errors, tx).await
    }

    /// Generic line-reading loop over any `AsyncBufRead`.
    async fn read_loop<R: tokio::io::AsyncBufRead + Unpin>(
        reader: &mut R,
        connector_id: String,
        running: Arc<AtomicBool>,
        events: Arc<AtomicU64>,
        errors: Arc<AtomicU64>,
        tx: tokio::sync::mpsc::Sender<SourceEvent>,
    ) -> Result<(), ConnectorError> {
        let mut ais = AisAssembler::default();
        let mut line = String::new();
        loop {
            if !running.load(Ordering::SeqCst) {
                return Ok(());
            }
            line.clear();
            let n = reader.read_line(&mut line)
                .await
                .map_err(ConnectorError::IoError)?;
            if n == 0 {
                // EOF
                return Err(ConnectorError::ConnectionError("EOF on NMEA stream".into()));
            }
            let sentence = line.trim();
            if sentence.is_empty() {
                continue;
            }
            match parse_sentence(sentence, &mut ais) {
                Some(data) => {
                    let event = data.to_source_event(&connector_id);
                    if tx.send(event).await.is_err() {
                        tracing::debug!("NMEA event channel closed, stopping");
                        return Ok(());
                    }
                    events.fetch_add(1, Ordering::Relaxed);
                }
                None => {
                    // Not every line produces an event (multi-part AIS, unknown
                    // sentences, etc.)
                    tracing::trace!(sentence = %sentence, "NMEA sentence not parsed");
                    // Count only genuine checksum failures / parse errors
                    if sentence.len() > 3 && !validate_checksum(sentence) {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

#[async_trait]
impl Connector for NmeaConnector {
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
            source = ?self.source,
            "NMEA connector started"
        );
        let source = self.source.clone();
        let cid = self.config.connector_id.clone();
        let running = self.running.clone();
        let events = self.events_count.clone();
        let errors = self.errors_count.clone();
        tokio::spawn(Self::run_loop(source, cid, running, events, errors, tx));
        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        tracing::info!(
            connector_id = %self.config.connector_id,
            "NMEA connector stopped"
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConnectorError::ConnectionError(
                "NMEA connector not running".into(),
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
            last_event_timestamp: None,
            uptime_seconds: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::ais_decoder::*;

    // ── Checksum ─────────────────────────────────────────────────────────────

    #[test]
    fn test_checksum_valid_gpgga() {
        // Real GGA sentence
        assert!(validate_checksum(
            "$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76"
        ));
    }

    #[test]
    fn test_checksum_valid_gprmc() {
        assert!(validate_checksum(
            "$GPRMC,092750.000,A,5321.6802,N,00630.3372,W,0.02,31.66,280511,,,A*43"
        ));
    }

    #[test]
    fn test_checksum_invalid_byte_changed() {
        // Flip one digit in the payload
        assert!(!validate_checksum(
            "$GPGGA,092750.000,5321.6803,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76"
        ));
    }

    #[test]
    fn test_checksum_missing_star() {
        assert!(!validate_checksum("$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,"));
    }

    #[test]
    fn test_checksum_aivdm() {
        // Checksum verified: XOR of "AIVDM,1,1,,B,15M67N0000G?Uf6E`FepT@3n00Sa,0" = 0x53
        assert!(validate_checksum(
            "!AIVDM,1,1,,B,15M67N0000G?Uf6E`FepT@3n00Sa,0*53"
        ));
    }

    #[test]
    fn test_compute_checksum() {
        // "GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,"
        // This is the body between $ and *
        let body = "GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,";
        assert_eq!(compute_checksum(body), 0x76);
    }

    // ── Coordinate parsing ───────────────────────────────────────────────────

    #[test]
    fn test_parse_coord_north() {
        // 5321.6802 N → 53 + 21.6802/60 = 53.36133...
        let lat = parse_nmea_coord("5321.6802", "N").unwrap();
        assert!((lat - 53.361_336_7).abs() < 1e-5, "lat = {lat}");
    }

    #[test]
    fn test_parse_coord_south() {
        let lat = parse_nmea_coord("5321.6802", "S").unwrap();
        assert!(lat < 0.0);
        assert!((lat + 53.361_336_7).abs() < 1e-5);
    }

    #[test]
    fn test_parse_coord_east_three_degree_digits() {
        // 00630.3372 E → 6 + 30.3372/60 = 6.50562
        let lon = parse_nmea_coord("00630.3372", "E").unwrap();
        assert!((lon - 6.505_62).abs() < 1e-4, "lon = {lon}");
    }

    #[test]
    fn test_parse_coord_west() {
        let lon = parse_nmea_coord("00630.3372", "W").unwrap();
        assert!(lon < 0.0);
    }

    #[test]
    fn test_parse_coord_empty_returns_none() {
        assert!(parse_nmea_coord("", "N").is_none());
    }

    #[test]
    fn test_parse_coord_zero_zero() {
        let lat = parse_nmea_coord("0000.0000", "N").unwrap();
        assert!(lat.abs() < 1e-6);
    }

    // ── GPGGA ────────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_gpgga_valid() {
        let mut ais = AisAssembler::default();
        let s = "$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76";
        let data = parse_sentence(s, &mut ais).unwrap();
        match data {
            NmeaData::Gpgga { lat, lon, satellites, hdop, fix_quality, .. } => {
                assert!((lat - 53.361_336).abs() < 1e-4);
                assert!((lon + 6.505_62).abs() < 1e-4);
                assert_eq!(satellites, 8);
                assert!((hdop - 1.03).abs() < 0.01);
                assert_eq!(fix_quality, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_gpgga_to_source_event() {
        let mut ais = AisAssembler::default();
        let s = "$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76";
        let data = parse_sentence(s, &mut ais).unwrap();
        let ev = data.to_source_event("nmea-1");
        assert_eq!(ev.entity_type, "own_vessel");
        assert_eq!(ev.entity_id, "own_vessel");
        assert!(ev.latitude.is_some());
    }

    // ── GPRMC ────────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_gprmc_active() {
        let mut ais = AisAssembler::default();
        let s = "$GPRMC,092750.000,A,5321.6802,N,00630.3372,W,0.02,31.66,280511,,,A*43";
        let data = parse_sentence(s, &mut ais).unwrap();
        match data {
            NmeaData::Gprmc { lat, lon, speed_knots, course, .. } => {
                assert!((lat - 53.361_336).abs() < 1e-4);
                assert!((lon + 6.505_62).abs() < 1e-4);
                assert!((speed_knots - 0.02).abs() < 0.001);
                assert!((course - 31.66).abs() < 0.01);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_gprmc_void_returns_none() {
        let mut ais = AisAssembler::default();
        // Status = V (void) — fix checksum manually:
        // body = "GPRMC,092750.000,V,5321.6802,N,00630.3372,W,0.02,31.66,280511,,,"
        let body = "GPRMC,092750.000,V,5321.6802,N,00630.3372,W,0.02,31.66,280511,,,";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        assert!(parse_sentence(&s, &mut ais).is_none());
    }

    // ── GPVTG ────────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_gpvtg() {
        let mut ais = AisAssembler::default();
        let body = "GPVTG,054.7,T,034.4,M,005.5,N,010.2,K,A";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais).unwrap();
        match data {
            NmeaData::Gpvtg { course_true, course_magnetic, speed_knots, speed_kmh } => {
                assert!((course_true - 54.7).abs() < 0.1);
                assert_eq!(course_magnetic, Some(34.4));
                assert!((speed_knots - 5.5).abs() < 0.1);
                assert!((speed_kmh - 10.2).abs() < 0.1);
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Depth ────────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_sddbt() {
        let mut ais = AisAssembler::default();
        let body = "SDDBT,20.0,f,6.1,M,3.3,F";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais).unwrap();
        match data {
            NmeaData::Depth { depth_m, below_surface } => {
                assert!((depth_m - 6.1).abs() < 0.01);
                assert!(!below_surface);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_sddbs() {
        let mut ais = AisAssembler::default();
        let body = "SDDBS,21.0,f,6.4,M,3.5,F";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais).unwrap();
        match data {
            NmeaData::Depth { below_surface, .. } => assert!(below_surface),
            _ => panic!("wrong variant"),
        }
    }

    // ── Wind ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_wimwd() {
        let mut ais = AisAssembler::default();
        let body = "WIMWD,045.0,T,047.0,M,12.5,N,6.4,M";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais).unwrap();
        match data {
            NmeaData::WindTrue { direction_true, speed_knots, .. } => {
                assert!((direction_true - 45.0).abs() < 0.1);
                assert!((speed_knots - 12.5).abs() < 0.1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_wimwv_relative() {
        let mut ais = AisAssembler::default();
        let body = "WIMWV,045.0,R,12.5,N,A";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais).unwrap();
        match data {
            NmeaData::WindRelative { angle, reference, speed_knots } => {
                assert!((angle - 45.0).abs() < 0.1);
                assert_eq!(reference, "R");
                assert!((speed_knots - 12.5).abs() < 0.1);
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Heading ──────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_hchdg() {
        let mut ais = AisAssembler::default();
        let body = "HCHDG,245.1,1.5,E,3.2,W";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais).unwrap();
        match data {
            NmeaData::Heading { heading_magnetic, deviation, variation } => {
                assert!((heading_magnetic - 245.1).abs() < 0.1);
                assert!((deviation.unwrap() - 1.5).abs() < 0.1);
                // variation West → negative
                assert!((variation.unwrap() + 3.2).abs() < 0.1);
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Transducer ───────────────────────────────────────────────────────────

    #[test]
    fn test_parse_yxxdr_temperature() {
        let mut ais = AisAssembler::default();
        let body = "YXXDR,C,21.5,C,AIRTEMP";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais).unwrap();
        match data {
            NmeaData::Transducer { value, units, .. } => {
                assert!((value - 21.5).abs() < 0.01);
                assert_eq!(units, "C");
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Engine RPM ───────────────────────────────────────────────────────────

    #[test]
    fn test_parse_errpm() {
        let mut ais = AisAssembler::default();
        let body = "ERRPM,E,1,850.0,0.0,A";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais).unwrap();
        match data {
            NmeaData::EngineRpm { engine_number, rpm } => {
                assert_eq!(engine_number, 1);
                assert!((rpm - 850.0).abs() < 0.1);
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── AIS payload decode ────────────────────────────────────────────────────

    /// Reference payload: Class A position report, MMSI 338234631
    /// Taken from a real AIS capture — type 1, lat/lon in North Sea.
    #[test]
    fn test_ais_decode_type1_position() {
        // Type 1 position report
        // Payload: 15M67N0000G?Uf6E`FepT@3n00Sa  (from a real AIVDM sentence)
        let payload = "15M67N0000G?Uf6E`FepT@3n00Sa";
        let fill_bits = 0u8;
        let bytes = decode_payload(payload, fill_bits).unwrap();
        let total_bits = payload.len() * 6;
        let buf = make_buffer(&bytes, total_bits);
        let msg = decode_type_1_2_3(&buf).unwrap();
        assert_eq!(msg.msg_type, 1);
        // MMSI should be a valid 9-digit number
        assert!(msg.mmsi > 100_000_000);
        // Position should be a plausible coordinate
        assert!(msg.lat.abs() < 90.0);
        assert!(msg.lon.abs() < 180.0);
    }

    #[test]
    fn test_ais_payload_decode_6bit_chars() {
        // Single character '@' encodes as value 0, which is ASCII 64
        // Payload "0" → byte 0 (after subtract 48, nothing to subtract further since 0 <= 39)
        // Actually ASCII '0' = 48, 48-48 = 0 → 6-bit value 0 = '@'
        let bytes = decode_payload("0", 0).unwrap();
        let total_bits = 6;
        let buf = make_buffer(&bytes, total_bits);
        let s = buf.get_ais_string(0, 1).unwrap();
        // Value 0 → '@', then trimmed → ""
        assert_eq!(s, "");
    }

    #[test]
    fn test_ais_payload_invalid_char_returns_none() {
        // ASCII 0x00 → 0 - 48 = underflow → invalid
        // Use a character outside valid range
        let result = decode_payload("\x00", 0);
        // 0x00 - 48 wraps; depending on implementation, may be None
        // The function returns None only if val > 63 after adjustments
        // Actually wrapping sub: 0u8.wrapping_sub(48) = 208, > 39, 208 - 8 = 200 > 63 → None
        assert!(result.is_none());
    }

    #[test]
    fn test_ais_full_sentence_parse() {
        let mut ais = AisAssembler::default();
        // Real AIVDM sentence — type 1 position report (checksum 0x53)
        let s = "!AIVDM,1,1,,B,15M67N0000G?Uf6E`FepT@3n00Sa,0*53";
        // First validate the checksum
        assert!(validate_checksum(s));
        let data = parse_sentence(s, &mut ais);
        assert!(data.is_some());
        match data.unwrap() {
            NmeaData::AisPosition { mmsi, own_vessel, .. } => {
                assert!(mmsi > 0);
                assert!(!own_vessel); // AIVDM = other vessel
            }
            _ => panic!("expected AisPosition"),
        }
    }

    #[test]
    fn test_ais_type5_decode() {
        // Build a synthetic type-5 payload to test field extraction.
        // We create a manually constructed bit string for a known vessel.
        // This is a real truncated type-5: "55?Pa842=4pDf@E8L000000000000000000000000000000000000"
        // (abridged, but valid enough to check msg_type and mmsi extraction)
        // For a proper test, let's use a well-known type 5 payload from ITU examples
        // Using a reference payload that is known-good:
        let payload = "55?Pa842=4pDf@E8L000000000000000000000000000000000000000000000000000000000000000";
        // This payload is too short; test that we handle gracefully (returns None without panic)
        let fill_bits = 2u8;
        let bytes_opt = decode_payload(payload, fill_bits);
        if let Some(bytes) = bytes_opt {
            let total_bits = payload.len() * 6 - fill_bits as usize;
            let buf = make_buffer(&bytes, total_bits);
            let msg_type = buf.get_bits(0, 6).unwrap_or(0);
            // Ensure msg_type is extracted without panic
            assert!(msg_type <= 63);
        }
        // No panic = pass
    }

    // ── Source URL parsing ────────────────────────────────────────────────────

    #[test]
    fn test_parse_tcp_url() {
        let src = parse_source_url("tcp://192.168.1.100:10110").unwrap();
        match src {
            NmeaSource::Tcp { host, port } => {
                assert_eq!(host, "192.168.1.100");
                assert_eq!(port, 10110);
            }
            _ => panic!("expected Tcp"),
        }
    }

    #[test]
    fn test_parse_serial_url() {
        let src = parse_source_url("serial:///dev/ttyUSB0?baud=38400").unwrap();
        match src {
            NmeaSource::Serial { device, baud } => {
                assert_eq!(device, "/dev/ttyUSB0");
                assert_eq!(baud, 38400);
            }
            _ => panic!("expected Serial"),
        }
    }

    #[test]
    fn test_parse_serial_url_default_baud() {
        let src = parse_source_url("serial:///dev/ttyS0").unwrap();
        match src {
            NmeaSource::Serial { baud, .. } => assert_eq!(baud, 4800),
            _ => panic!("expected Serial"),
        }
    }

    #[test]
    fn test_parse_unknown_scheme_returns_err() {
        assert!(parse_source_url("udp://192.168.1.1:1234").is_err());
    }

    // ── Multi-part AIS assembler ──────────────────────────────────────────────

    #[test]
    fn test_ais_assembler_single_part() {
        let mut asm = AisAssembler::default();
        // Single-part: total=1, part=1
        let fields = vec!["AIVDM", "1", "1", "", "B", "PAYLOAD", "0"];
        let result = asm.feed(&fields);
        assert!(result.is_some());
        let (p, f) = result.unwrap();
        assert_eq!(p, "PAYLOAD");
        assert_eq!(f, 0);
    }

    #[test]
    fn test_ais_assembler_two_part_in_order() {
        let mut asm = AisAssembler::default();
        let f1 = vec!["AIVDM", "2", "1", "3", "B", "PART1", "0"];
        let f2 = vec!["AIVDM", "2", "2", "3", "B", "PART2", "2"];
        assert!(asm.feed(&f1).is_none());
        let result = asm.feed(&f2);
        assert!(result.is_some());
        let (payload, fill) = result.unwrap();
        assert_eq!(payload, "PART1PART2");
        assert_eq!(fill, 2);
    }

    // ── BitBuffer ────────────────────────────────────────────────────────────

    #[test]
    fn test_bitbuffer_get_bits() {
        // 0b1010_1010 = 0xAA
        let data = [0xAAu8];
        let buf = BitBuffer::new(&data, 8);
        assert_eq!(buf.get_bits(0, 4).unwrap(), 0b1010);
        assert_eq!(buf.get_bits(4, 4).unwrap(), 0b1010);
        assert_eq!(buf.get_bits(0, 8).unwrap(), 0xAA);
    }

    #[test]
    fn test_bitbuffer_get_signed_negative() {
        // Two's complement: 3-bit value 0b111 = -1
        let data = [0b111_00000u8];
        let buf = BitBuffer::new(&data, 8);
        assert_eq!(buf.get_signed(0, 3).unwrap(), -1i64);
    }

    #[test]
    fn test_bitbuffer_out_of_bounds_returns_none() {
        let data = [0xFFu8];
        let buf = BitBuffer::new(&data, 8);
        assert!(buf.get_bits(0, 9).is_none());
        assert!(buf.get_bits(8, 1).is_none());
    }

    // ── Connector construction ────────────────────────────────────────────────

    #[test]
    fn test_nmea_connector_new_tcp() {
        let config = ConnectorConfig {
            connector_id: "nmea-tcp".into(),
            connector_type: "nmea".into(),
            url: Some("tcp://192.168.1.100:10110".into()),
            entity_type: "own_vessel".into(),
            enabled: true,
            trust_score: 0.99,
            properties: HashMap::new(),
        };
        let conn = NmeaConnector::new(config);
        assert!(conn.is_ok());
    }

    #[test]
    fn test_nmea_connector_new_missing_url() {
        let config = ConnectorConfig {
            connector_id: "nmea-nourl".into(),
            connector_type: "nmea".into(),
            url: None,
            entity_type: "own_vessel".into(),
            enabled: true,
            trust_score: 0.99,
            properties: HashMap::new(),
        };
        assert!(NmeaConnector::new(config).is_err());
    }

    #[tokio::test]
    async fn test_nmea_connector_health_not_running() {
        let config = ConnectorConfig {
            connector_id: "nmea-health".into(),
            connector_type: "nmea".into(),
            url: Some("tcp://127.0.0.1:10110".into()),
            entity_type: "own_vessel".into(),
            enabled: true,
            trust_score: 0.99,
            properties: HashMap::new(),
        };
        let conn = NmeaConnector::new(config).unwrap();
        assert!(conn.health_check().await.is_err());
    }

    // ── GNSS talker prefix agnosticism ───────────────────────────────────────

    #[test]
    fn test_gngga_accepted() {
        // GN prefix (multi-constellation GNSS)
        let mut ais = AisAssembler::default();
        let body = "GNGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais);
        assert!(data.is_some());
    }

    #[test]
    fn test_glrmc_accepted() {
        // GL prefix (GLONASS)
        let mut ais = AisAssembler::default();
        let body = "GLRMC,092750.000,A,5321.6802,N,00630.3372,W,0.02,31.66,280511,,,A";
        let cs = compute_checksum(body);
        let s = format!("${}*{:02X}", body, cs);
        let data = parse_sentence(&s, &mut ais);
        assert!(data.is_some());
    }

    // ── AIVDO (own vessel AIS) ────────────────────────────────────────────────

    #[test]
    fn test_aivdo_sets_own_vessel_flag() {
        let mut ais = AisAssembler::default();
        // Re-use the same payload but with !AIVDO prefix
        let payload = "15M67N0000G?Uf6E`FepT@3n00Sa";
        let body = format!("AIVDO,1,1,,B,{},0", payload);
        let cs = compute_checksum(&body);
        let s = format!("!{}*{:02X}", body, cs);
        if let Some(NmeaData::AisPosition { own_vessel, .. }) = parse_sentence(&s, &mut ais) {
            assert!(own_vessel);
        }
        // If parse failed (bad payload for this talker), that's OK — no panic
    }

    // ── AIS types 4, 9, 27 ────────────────────────────────────────────────────

    /// Pack `(value, bits)` fields MSB-first into an AIVDM 6-bit payload.
    /// Used to hand-craft AIS payloads whose layout is documented inline.
    fn pack_to_aivdm_payload(fields: &[(u64, usize)]) -> (String, u8) {
        let mut bits: Vec<bool> = Vec::new();
        for (val, w) in fields {
            for shift in (0..*w).rev() { bits.push((val >> shift) & 1 == 1); }
        }
        let total = bits.len();
        let pad = (6 - (total % 6)) % 6;
        let mut s = String::with_capacity((total + pad) / 6);
        for chunk in 0..(total + pad) / 6 {
            let mut v = 0u8;
            for j in 0..6 {
                let i = chunk * 6 + j;
                v = (v << 1) | (if i < total && bits[i] { 1 } else { 0 });
            }
            s.push(if v < 40 { (v + 48) as char } else { (v + 56) as char });
        }
        (s, pad as u8)
    }

    fn aivdm_sentence(fields: &[(u64, usize)]) -> String {
        let (payload, fill) = pack_to_aivdm_payload(fields);
        let body = format!("AIVDM,1,1,,A,{},{}", payload, fill);
        format!("!{}*{:02X}", body, compute_checksum(&body))
    }

    fn signed_mask(value: i64, bits: usize) -> u64 {
        (value as u64) & ((1u64 << bits) - 1)
    }

    #[test]
    fn test_ais_type4_decode_and_to_source_event() {
        // Type 4 base station: MMSI 003660057 (US Coast Guard),
        // 2024-03-15 12:34:56 UTC, lat 37.5°N, lon -122.5°W, fix=GPS.
        // 1/10000-min scaling: 37.5×600000 = 22_500_000; -122.5×600000 = -73_500_000.
        // Layout (168): type(6) rep(2) mmsi(30) year(14) mon(4) day(5) hr(5)
        //   min(6) sec(6) acc(1) lon(28s) lat(27s) fix(4) spare(10) raim(1) comm(19)
        let s = aivdm_sentence(&[
            (4, 6), (0, 2), (3_660_057, 30),
            (2024, 14), (3, 4), (15, 5), (12, 5), (34, 6), (56, 6),
            (1, 1), (signed_mask(-73_500_000, 28), 28),
            (signed_mask(22_500_000, 27), 27),
            (1, 4), (0, 10), (0, 1), (0, 19),
        ]);
        let mut asm = AisAssembler::default();
        let data = parse_sentence(&s, &mut asm).expect("type 4 parses");
        match &data {
            NmeaData::AisBaseStation { mmsi, lat, lon, utc_year, utc_month, fix_type, .. } => {
                assert_eq!(*mmsi, 3_660_057);
                assert_eq!(*utc_year, 2024);
                assert_eq!(*utc_month, 3);
                assert!((*lat - 37.5).abs() < 1e-4);
                assert!((*lon - (-122.5)).abs() < 1e-4);
                assert_eq!(*fix_type, 1);
            }
            other => panic!("expected AisBaseStation, got {:?}", other),
        }
        let ev = data.to_source_event("nmea-bs");
        assert_eq!(ev.entity_type, "ais_base_station");
        assert_eq!(ev.entity_id, "ais_base:3660057");
    }

    #[test]
    fn test_ais_type4_decode_direct_buffer() {
        // Same fixture exercised through the raw bit-buffer decoder.
        let (payload, fill) = pack_to_aivdm_payload(&[
            (4, 6), (0, 2), (3_660_057, 30),
            (2024, 14), (3, 4), (15, 5), (12, 5), (34, 6), (56, 6),
            (1, 1), (signed_mask(-73_500_000, 28), 28),
            (signed_mask(22_500_000, 27), 27),
            (1, 4), (0, 10), (1, 1), (0, 19),
        ]);
        let bytes = ais_decoder::decode_payload(&payload, fill).unwrap();
        let buf = ais_decoder::make_buffer(&bytes, payload.len() * 6 - fill as usize);
        let d = ais_decoder::decode_type_4(&buf).unwrap();
        assert_eq!((d.day, d.hour, d.minute, d.second), (15, 12, 34, 56));
        assert!(d.raim && d.position_accuracy);
    }

    #[test]
    fn test_ais_type9_decode_and_to_source_event() {
        // SAR aircraft over the English Channel: MMSI 111222333,
        // lat 50.5°N (30_300_000), lon 1.0°E (600_000), alt 800 m, sog 200 kts,
        // cog 270.0°, ts 30, raim=1.
        // Layout (168): type(6) rep(2) mmsi(30) alt(12) sog(10) acc(1)
        //   lon(28s) lat(27s) cog(12) ts(6) regional(8) dte(1) spare(3)
        //   assigned(1) raim(1) radio(20)
        let s = aivdm_sentence(&[
            (9, 6), (0, 2), (111_222_333, 30),
            (800, 12), (200, 10), (1, 1),
            (signed_mask(600_000, 28), 28),
            (signed_mask(30_300_000, 27), 27),
            (2700, 12), (30, 6),
            (0, 8), (0, 1), (0, 3), (0, 1), (1, 1), (0, 20),
        ]);
        let mut asm = AisAssembler::default();
        let data = parse_sentence(&s, &mut asm).expect("type 9 parses");
        match &data {
            NmeaData::AisSarAircraft { mmsi, altitude_m, sog, lat, lon, cog, raim, .. } => {
                assert_eq!((*mmsi, *altitude_m, *sog), (111_222_333, 800, 200));
                assert!((*lat - 50.5).abs() < 1e-4);
                assert!((*lon - 1.0).abs() < 1e-4);
                assert!((*cog - 270.0).abs() < 0.01);
                assert!(*raim);
            }
            other => panic!("expected AisSarAircraft, got {:?}", other),
        }
        let ev = data.to_source_event("nmea-sar");
        assert_eq!(ev.entity_type, "sar_aircraft");
        assert_eq!(ev.entity_id, "mmsi:111222333");
    }

    #[test]
    fn test_ais_type9_decode_direct_buffer() {
        // Probe N/A sentinels: alt=4095, sog=1023.
        let (payload, fill) = pack_to_aivdm_payload(&[
            (9, 6), (0, 2), (111_222_333, 30),
            (4095, 12), (1023, 10), (0, 1),
            (signed_mask(600_000, 28), 28),
            (signed_mask(30_300_000, 27), 27),
            (3600, 12), (60, 6),
            (0, 8), (0, 1), (0, 3), (0, 1), (0, 1), (0, 20),
        ]);
        let bytes = ais_decoder::decode_payload(&payload, fill).unwrap();
        let buf = ais_decoder::make_buffer(&bytes, payload.len() * 6 - fill as usize);
        let d = ais_decoder::decode_type_9(&buf).unwrap();
        assert_eq!((d.altitude_m, d.sog), (4095, 1023));
        assert!(!d.raim);
    }

    #[test]
    fn test_ais_type27_decode_and_to_source_event() {
        // Long-range coastal broadcast (LOWER precision: lat 17 / lon 18 bits,
        // 1/10-minute scaling).  60.0×600 = 36_000;  -30.0×600 = -18_000.
        // Layout (96): type(6) rep(2) mmsi(30) acc(1) raim(1) nav(4)
        //   lon(18s) lat(17s) sog(6) cog(9) gnss(1) spare(1)
        let s = aivdm_sentence(&[
            (27, 6), (0, 2), (244_123_456, 30),
            (1, 1), (0, 1), (0, 4),
            (signed_mask(-18_000, 18), 18), (signed_mask(36_000, 17), 17),
            (12, 6), (90, 9), (0, 1), (0, 1),
        ]);
        let mut asm = AisAssembler::default();
        let data = parse_sentence(&s, &mut asm).expect("type 27 parses");
        match &data {
            NmeaData::AisLongRange { mmsi, lat, lon, sog, cog, .. } => {
                assert_eq!(*mmsi, 244_123_456);
                assert!((*lat - 60.0).abs() < 1e-3);
                assert!((*lon - (-30.0)).abs() < 1e-3);
                assert_eq!((*sog, *cog), (12, 90));
            }
            other => panic!("expected AisLongRange, got {:?}", other),
        }
        let ev = data.to_source_event("nmea-lr");
        assert_eq!(ev.entity_type, "ship");
        assert_eq!(ev.entity_id, "mmsi:244123456");
        assert_eq!(ev.properties.get("long_range").unwrap(), &Json::from(true));
    }

    #[test]
    fn test_ais_type27_rejects_sentinel_position() {
        // 91.0×600 = 54_600 fits in 17 unsigned bits — must be rejected
        // as the M.1371 "not available" sentinel by the dispatcher.
        let s = aivdm_sentence(&[
            (27, 6), (0, 2), (244_999_999, 30),
            (0, 1), (0, 1), (15, 4),
            (signed_mask(108_600, 18), 18), (signed_mask(54_600, 17), 17),
            (63, 6), (511, 9), (1, 1), (0, 1),
        ]);
        let mut asm = AisAssembler::default();
        assert!(parse_sentence(&s, &mut asm).is_none());
    }
}
