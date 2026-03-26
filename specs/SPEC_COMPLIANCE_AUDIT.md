# ORP Protocol Parser — Specification Compliance Audit

**Auditor:** Domain Expert Subagent  
**Date:** 2026-03-27  
**Scope:** `/Users/deepred/orp/crates/orp-connector/src/adapters/`  
**Standards referenced:** IEC 61162, ITU-R M.1371-5, NMEA 2000, Eurocontrol ASTERIX Ed 2.1,  
WMO METAR, STIX 2.1, CEF (ArcSight), Modbus Application Protocol, IEEE 1815 (DNP3)

---

## Summary Table

| Parser | Compliance | Critical Issues |
|--------|-----------|-----------------|
| `nmea.rs` — NMEA 0183 checksum + coordinates | **PASS** | None |
| `nmea.rs` — AIS decoder (ITU-R M.1371-5) | **PASS** | None |
| `ais.rs` — AIS connector | **FAIL** | Stub only — no real AIS binary decoding |
| `nmea2000.rs` — NMEA 2000 PGN decoder | **PASS** | None |
| `adsb.rs` — ADS-B / Mode S | **PARTIAL** | SBS format only; no Mode S binary, no CPR |
| `asterix.rs` — ASTERIX Cat 048/062 | **PASS** | Minor: Cat 048 I048/130 subfield parsing simplified |
| `metar.rs` — METAR / WMO | **PASS** | Minor: no multi-word fractional visibility (e.g. "1 1/2SM") |
| `stix.rs` — STIX 2.1 | **PARTIAL** | Missing `spec_version` field; `created`/`modified` are optional |
| `cef.rs` — CEF format | **PASS** | None |
| `modbus.rs` — Modbus TCP + RTU CRC | **PASS** | None |
| `dnp3.rs` — DNP3 CRC + data link | **PASS** | Minor: binary input flags bit interpretation |

---

## Detailed Findings

---

### 1. `nmea.rs` — NMEA 0183 (IEC 61162)

**Compliance: PASS**

#### Checksum Algorithm
```rust
let start = if sentence.starts_with('$') || sentence.starts_with('!') { 1 } else { 0 };
let computed = sentence[start..star].bytes().fold(0u8, |acc, b| acc ^ b);
```
✅ **Correct.** XOR of all bytes between the `$`/`!` (exclusive) and `*` (exclusive), expressed as two uppercase hex digits. Matches IEC 61162-1 §6.2.

#### Coordinate Format
```rust
let deg_end = dot - 2;
let degrees: f64 = coord[..deg_end].parse().ok()?;
let minutes: f64 = coord[deg_end..].parse().ok()?;
let decimal = degrees + minutes / 60.0;
```
✅ **Correct.** NMEA `DDDMM.MMMM` encoding: last 2 digits before decimal are minutes. Formula `deg + min/60` is correct. Handles both 2-digit (lat) and 3-digit (lon) degree prefixes correctly.

#### Sentence Types
✅ All documented sentence types (GGA, RMC, VTG, AIVDM/AIVDO, SDDBT, SDDBS, WIMWD, WIMWV, HCHDG, YXXDR, ERRPM) parsed correctly.

✅ GNSS talker agnosticism: matcher uses `ends_with("GGA")` etc., accepting GP, GN, GL prefixes per IEC 61162-1 Table 1.

**No field offset errors. No mathematical conversion errors. No missing required fields.**

---

### 2. `nmea.rs` — AIS Decoder (ITU-R M.1371-5)

**Compliance: PASS**

#### 6-Bit Armoring Decode
```rust
let mut val = ch.wrapping_sub(48);
if val > 39 { val = val.wrapping_sub(8); }
if val > 63 { return None; }
```
✅ **Correct.** ITU-R M.1371-5 §3.4: subtract 48, if result >39 subtract 8, yielding 0–63. Invalid characters (result >63) correctly rejected.

#### Message Type 1/2/3 Field Offsets
| Field | Spec (ITU-R M.1371-5 Table 2) | Implementation | Status |
|-------|-------------------------------|----------------|--------|
| Message type | bits 0–5 (6 bits) | `get_bits(0, 6)` | ✅ |
| Repeat indicator | bits 6–7 (2 bits) | _not stored, OK_ | ✅ |
| MMSI | bits 8–37 (30 bits) | `get_bits(8, 30)` | ✅ |
| Nav status | bits 38–41 (4 bits) | `get_bits(38, 4)` | ✅ |
| SOG | bits 50–59 (10 bits) | `get_bits(50, 10)` | ✅ |
| Longitude | bits 61–88 (28 bits, signed) | `get_signed(61, 28)` | ✅ |
| Latitude | bits 89–115 (27 bits, signed) | `get_signed(89, 27)` | ✅ |
| COG | bits 116–127 (12 bits) | `get_bits(116, 12)` | ✅ |
| True heading | bits 128–136 (9 bits) | `get_bits(128, 9)` | ✅ |

✅ Lon/lat scaling: `raw / 600_000.0` — spec defines units as 1/10,000 minute = 1/600,000 degree. Correct.  
✅ SOG scaling: `raw / 10.0` — spec defines 1/10 knot. Correct.  
✅ COG scaling: `raw / 10.0` — spec defines 1/10 degree. Correct.

#### Message Type 5 Field Offsets
| Field | Spec bits | Implementation | Status |
|-------|-----------|----------------|--------|
| MMSI | 8–37 | `get_bits(8, 30)` | ✅ |
| IMO | 40–69 | `get_bits(40, 30)` | ✅ |
| Call sign | 70–111 (7 × 6-bit) | `get_ais_string(70, 7)` | ✅ |
| Vessel name | 112–231 (20 × 6-bit) | `get_ais_string(112, 20)` | ✅ |
| Ship type | 232–239 (8 bits) | `get_bits(232, 8)` | ✅ |
| Draught | 294–301 (8 bits, 1/10m) | `get_bits(294, 8)` then `/10.0` | ✅ |
| Destination | 302–421 (20 × 6-bit) | `get_ais_string(302, 20)` | ✅ |

#### Message Types 18/19 (Class B)
| Field | Spec | Implementation | Status |
|-------|------|----------------|--------|
| MMSI | bits 8–37 | `get_bits(8, 30)` | ✅ |
| SOG | bits 46–55 | `get_bits(46, 10)` | ✅ |
| Longitude | bits 57–84 | `get_signed(57, 28)` | ✅ |
| Latitude | bits 85–111 | `get_signed(85, 27)` | ✅ |
| COG | bits 112–123 | `get_bits(112, 12)` | ✅ |
| True heading | bits 124–132 | `get_bits(124, 9)` | ✅ |

#### Message Type 21 (Aid to Navigation)
| Field | Spec | Implementation | Status |
|-------|------|----------------|--------|
| MMSI | bits 8–37 | `get_bits(8, 30)` | ✅ |
| Aid type | bits 38–42 (5 bits) | `get_bits(38, 5)` | ✅ |
| Name | bits 43–162 (20 × 6-bit) | `get_ais_string(43, 20)` | ✅ |
| Longitude | bits 164–191 (28 bits) | `get_signed(164, 28)` | ✅ |
| Latitude | bits 192–218 (27 bits) | `get_signed(192, 27)` | ✅ |

**No field offset errors. No mathematical conversion errors.**

---

### 3. `ais.rs` — Standalone AIS Connector

**Compliance: FAIL**

#### Critical Issue: No Real AIS Binary Decoding
The `parse_nmea_sentence()` function explicitly returns `None` with the comment:
```rust
// Very simplified parser for demo; real AIS uses 6-bit ASCII encoding
// Real implementation would decode the binary payload in parts[5]
None
```

The connector operates in **demo mode only**, generating hardcoded fake ship positions near Rotterdam. It does **not** decode actual NMEA-armored AIS payloads.

**Missing:**
- 6-bit armored payload decoding (implemented correctly in `nmea.rs` but not referenced here)
- Any message type parsing (types 1–27)
- MMSI extraction from actual AIS payload

**Note:** The full AIS decoder *is* correctly implemented in `nmea.rs::ais_decoder` and would serve as the authoritative implementation. The `ais.rs` connector is a separate stub that should delegate to it or be removed.

---

### 4. `nmea2000.rs` — NMEA 2000 / IEC 61162-3

**Compliance: PASS**

#### PGN Numbers
| PGN | Standard Name | Implementation | Status |
|-----|--------------|----------------|--------|
| 127250 | Vessel Heading | `N2kPgn::VesselHeading` | ✅ |
| 128259 | Speed, Water Referenced | `N2kPgn::Speed` | ✅ |
| 128267 | Water Depth | `N2kPgn::WaterDepth` | ✅ |
| 130306 | Wind Data | `N2kPgn::WindData` | ✅ |
| 129025 | Position, Rapid Update | `N2kPgn::PositionRapid` | ✅ |
| 129038 | AIS Class A Position Report | `N2kPgn::AisClassAPosition` | ✅ |
| 129039 | AIS Class B Position Report | `N2kPgn::AisClassBPosition` | ✅ |
| 129026 | COG & SOG, Rapid Update | `N2kPgn::CogSogRapid` | ✅ |
| 129029 | GNSS Position Data | `N2kPgn::GnssFix` | ✅ |
| 127245 | Rudder | `N2kPgn::Rudder` | ✅ |

#### CAN ID → PGN Extraction
```rust
let pgn = if pf >= 240 { (dp << 16) | ((pf as u32) << 8) | (ps as u32) }
          else         { (dp << 16) | ((pf as u32) << 8) };
```
✅ **Correct.** ISO 11898 / NMEA 2000 PGN calculation: PDU2 (PF ≥ 240) includes PS in PGN; PDU1 (PF < 240) destination-specific, PS not part of PGN.

#### PGN 127250 — Vessel Heading Scaling
- Heading: `u16 × 0.0001 rad` → spec: 0.0001 rad/LSB ✅
- Deviation/Variation: `i16 × 0.0001 rad` ✅
- Reference: 2-bit (True=0, Magnetic=1) ✅

#### PGN 128259 — Speed Scaling
- `u16 × 0.01 m/s` → spec: 0.01 m/s per LSB ✅

#### PGN 128267 — Depth Scaling
- `u32 × 0.01 m` → spec: 0.01 m/LSB ✅
- Offset: `i16 × 0.001 m` → spec: 0.001 m/LSB ✅

#### PGN 130306 — Wind Data Scaling
- Speed: `u16 × 0.01 m/s` ✅
- Angle: `u16 × 0.0001 rad` ✅

#### PGN 129025 — Position Rapid
- `i32 × 1e-7 degrees` → spec: 1e-7 deg/LSB ✅

**No errors detected. All PGN numbers, field offsets, and scaling factors correct.**

---

### 5. `adsb.rs` — ADS-B / Mode S

**Compliance: PARTIAL**

#### What Is Implemented
The connector implements parsing of **SBS BaseStation format** (dump1090 port 30003 CSV output):
```
MSG,{type},{session},{aircraft},{icao},{flight},{date},{time},{date},{time},{callsign},{alt},...
```
✅ MSG type 3 (airborne position) and type 2 (surface position) correctly handled.  
✅ ICAO address (6-char hex) correctly extracted from field 5.  
✅ Position, altitude, speed, heading, vertical rate, squawk extracted correctly.

#### Missing: Mode S Binary Framing
The connector **does not** implement actual Mode S message parsing:
- ❌ No DF (Downlink Format) extraction from raw Mode S frames  
- ❌ No 24-bit ICAO address extraction from Mode S short/long squitter  
- ❌ No CRC-24 computation/verification (Mode S uses CRC-24/OpenPGP)  
- ❌ No Compact Position Reporting (CPR) decoding for lat/lon from raw ADS-B  
- ❌ No Beast binary format decoding  
- ❌ No TC (Type Code) field parsing for ADS-B message subtypes  

#### Summary
The SBS format parser is correct and complete for that format. For production use with raw Mode S or Beast format receivers, the binary decoding layer is absent. The implementation should be documented as "SBS/BaseStation format only."

---

### 6. `asterix.rs` — ASTERIX Cat 048 / Cat 062

**Compliance: PASS**

#### FSPEC Parsing
```rust
if byte & 0x01 == 0 { break; } // FX bit
```
✅ **Correct.** Each FSPEC octet's bit 0 (FX) indicates extension. 7 data bits per octet. Matches ASTERIX Framing ed 2.1.

`fspec_has_field()` correctly maps field index to byte and bit position (bit 7 = FRN 1, bit 6 = FRN 2, etc.).

#### Category 048 UAP Verification
| FRN | Item | Spec | Implementation | Status |
|-----|------|------|----------------|--------|
| 1 | I048/010 SAC/SIC | 2 bytes | `offset += 2` | ✅ |
| 2 | I048/140 Time of Day | 3 bytes, 1/128 s | `raw / 128.0` | ✅ |
| 3 | I048/020 Target Descriptor | variable | 1 byte + ext | ✅ |
| 4 | I048/040 Measured Polar | 4 bytes | rho/256 NM, theta×360/65536° | ✅ |
| 5 | I048/070 Mode-3/A | 2 bytes | `& 0x0FFF` | ✅ |
| 6 | I048/090 Flight Level | 2 bytes, 1/4 FL | `& 0x3FFF) / 4.0` | ✅ |
| 7 | I048/130 Radar Plot Char. | compound variable | simplified skip | ⚠️ |
| 8 | I048/220 Aircraft Address | 3 bytes | hex formatted | ✅ |
| 9 | I048/240 Aircraft ID | 6 bytes, ICAO 6-bit | `decode_icao_6bit` | ✅ |
| 10 | I048/250 Mode S MB Data | variable (1+N×8) | skip with count | ✅ |
| 11 | I048/161 Track Number | 2 bytes | `& 0x0FFF` | ✅ |
| 12 | I048/042 Cartesian Pos. | 4 bytes, 1/128 NM | `/ 128.0` | ✅ |
| 13 | I048/200 Track Velocity | 4 bytes | gs×3600/16384, hdg×360/65536 | ✅ |

⚠️ **Minor:** I048/130 subfield parsing is simplified — reads primary indicator and skips 1 byte per active bit. This is correct for the primary subfields but does not handle the extended compound format fully. Non-critical for core tracking.

#### Category 062 UAP Verification
| FRN | Item | Spec | Implementation | Status |
|-----|------|------|----------------|--------|
| 1 | I062/010 SAC/SIC | 2 bytes | direct | ✅ |
| 2 | I062/015 Service ID | 1 byte | direct | ✅ |
| 3 | I062/070 Time of Track | 3 bytes, 1/128 s | `/ 128.0` | ✅ |
| 4 | I062/105 WGS-84 Position | 8 bytes | `× (180 / 2^25)` | ✅ |
| 5 | I062/100 Cartesian Pos. | 6 bytes (3+3), 0.5 m | sign extension + `× 0.5` | ✅ |
| 6 | I062/185 Velocity Cart. | 4 bytes, 0.25 m/s | `× 0.25` | ✅ |
| 7 | I062/210 Acceleration | 2 bytes, 0.25 m/s² | `× 0.25` | ✅ |

✅ I062/105 scaling: `180.0 / (1i64 << 25)` = `180 / 33,554,432` ≈ `5.364e-6°` per LSB. Correct per Eurocontrol Cat 062 spec.

**All field positions and scaling factors correct.**

---

### 7. `metar.rs` — METAR / WMO No.49

**Compliance: PASS**

#### Wind Format Verification
```rust
// Handles: "22006KT", "VRB03KT", "18010G25KT", "27015MPS"
```
✅ Direction: 3-digit degrees (000–360)  
✅ Speed: 2-digit knots  
✅ Gust: `G` separator + 2-digit value  
✅ VRB (variable) direction  
✅ KT, MPS, KMH units  
✅ Variable wind sector (e.g. `180V240`) handled

#### Visibility
✅ SM format (statute miles, including fractions like `1/2SM`)  
✅ 4-digit meter format (9999, 0600, etc.)  
✅ CAVOK sets visibility ≥6.21 SM and sky_clear

⚠️ **Minor:** Compound fractional visibility `1 1/2SM` (with space) would not be parsed as a single token — would need to be handled as two consecutive tokens. Most METAR feeds do not include this but it is valid WMO syntax.

#### Cloud Layers
✅ FEW, SCT, BKN, OVC, VV all parsed  
✅ Altitude in hundreds of feet (`FEW050` → 5000 ft)  
✅ Cloud type suffix (CB, TCU) extracted correctly

#### Temperature/Dewpoint
✅ `TT/DD` and `MT/MD` (M = minus) correctly handled

#### Altimeter
✅ `A` prefix: inHg (divide by 100) — `A3012` → 30.12 inHg  
✅ `Q` prefix: hPa direct — `Q1013` → 1013 hPa

#### Present Weather Codes
✅ Intensity modifiers (+, -, VC) recognized  
✅ WMO precipitation, obscuration, descriptor codes all listed

**No mathematical conversion errors. Standard compliance is solid.**

---

### 8. `stix.rs` — STIX 2.1

**Compliance: PARTIAL**

#### Bundle Parsing
✅ `type`, `id`, `objects` fields correctly parsed via serde  
✅ Generic `StixObject` struct with `flatten` for unknown extension fields

#### Missing Required Fields (STIX 2.1 §3.2)
Per STIX 2.1 specification, all SDOs and SROs **must** include `spec_version`:

```json
{ "spec_version": "2.1", ... }
```

❌ **`spec_version` is absent from the `StixObject` struct.** Bundles from STIX 2.1 servers will include it; it will be captured by the `extra` HashMap (due to `#[serde(flatten)]`) but not validated or surfaced explicitly. Parsers that enforce strict compliance may reject or mis-route objects.

#### Optional vs Required Field Enforcement
Per STIX 2.1:
- `created` and `modified` are **required** for all SDOs/SROs
- Implementation declares them `Option<String>` — no validation that they are present

This means malformed objects (missing required timestamps) will be accepted silently.

#### Per-Type Required Field Gaps
| STIX Type | Required (spec) | Missing validation |
|-----------|----------------|-------------------|
| `indicator` | pattern, pattern_type, valid_from | No enforcement |
| `malware` | is_family | No enforcement |
| `relationship` | relationship_type, source_ref, target_ref | No enforcement |

The struct will silently deserialize incomplete objects. For a read-only intelligence feed this is acceptable; for writing or validating STIX objects it is not.

#### Type ID Format
Per STIX 2.1, IDs must follow the format `<type>--<UUIDv4>`. The parser does not validate this constraint, but this is a minor concern for read-only parsing.

---

### 9. `cef.rs` — Common Event Format (ArcSight CEF)

**Compliance: PASS**

#### Format Structure
Format: `CEF:Version|Device Vendor|Device Product|Device Version|Signature ID|Name|Severity|Extensions`

✅ 8 pipe-delimited fields required (validated: `fields.len() < 8` → error)  
✅ Version parsed as `u8`  
✅ All 7 header fields correctly mapped

#### Escape Handling
✅ `\|` → `|` (pipe in field value)  
✅ `\\` → `\`  
✅ `\n` → newline  
✅ `\r` → carriage return  
✅ `\=` → `=` (for extension values)

#### Severity Mapping (CEF spec §6)
| Numeric | Level | Implementation | Status |
|---------|-------|----------------|--------|
| 0–3 | Low | `CefSeverity::Low` | ✅ |
| 4–6 | Medium | `CefSeverity::Medium` | ✅ |
| 7–8 | High | `CefSeverity::High` | ✅ |
| 9–10 | Very-High | `CefSeverity::VeryHigh` | ✅ |

✅ Text severity variants (Low, Medium, High, Very-High, Critical) correctly handled.

#### Extension Parsing
✅ Key=value parsing respects values with spaces by scanning for next `key=` boundary  
✅ Well-known keys (src, dst, spt, dpt, act, msg, cat) promoted to top-level properties

**No issues detected. Format fully compliant.**

---

### 10. `modbus.rs` — Modbus (IEC 61158)

**Compliance: PASS**

#### Function Code Mapping
| Hex | Function | Implementation | Status |
|-----|----------|----------------|--------|
| 0x01 | Read Coils | `FunctionCode::ReadCoils` | ✅ |
| 0x02 | Read Discrete Inputs | `FunctionCode::ReadDiscreteInputs` | ✅ |
| 0x03 | Read Holding Registers | `FunctionCode::ReadHoldingRegisters` | ✅ |
| 0x04 | Read Input Registers | `FunctionCode::ReadInputRegisters` | ✅ |
| 0x05 | Write Single Coil | `FunctionCode::WriteSingleCoil` | ✅ |
| 0x06 | Write Single Register | `FunctionCode::WriteSingleRegister` | ✅ |
| 0x0F | Write Multiple Coils | `FunctionCode::WriteMultipleCoils` | ✅ |
| 0x10 | Write Multiple Registers | `FunctionCode::WriteMultipleRegisters` | ✅ |

#### CRC-16 Polynomial
```rust
crc = (crc >> 1) ^ 0xA001;  // Modbus RTU CRC-16
```
✅ **Correct.** Modbus RTU uses CRC-16/IBM (reflected polynomial 0xA001). Test vector verified:
- Input: `[0x01, 0x03, 0x00, 0x00, 0x00, 0x01]` → CRC `0x0A84` ✅ (matches known test vector)

#### MBAP Header
✅ Transaction ID (2 bytes BE), Protocol ID (2 bytes BE, always 0), Length (2 bytes BE), Unit ID (1 byte) — 7 bytes total. Correct.

#### Exception Response Detection
```rust
if fc_raw & 0x80 != 0 { ... }  // High bit set = exception
```
✅ **Correct.** Modbus protocol: exception response sets bit 7 of function code.

#### Request Frame Builder
✅ `build_read_holding_registers` correctly encodes FC=0x03  
✅ `build_read_input_registers` correctly encodes FC=0x04  
✅ MBAP length field = 6 (unit_id + FC + start_addr + quantity) ✅

**No issues detected.**

---

### 11. `dnp3.rs` — DNP3 (IEEE 1815)

**Compliance: PASS**

#### CRC-16-DNP Polynomial
```rust
crc = (crc >> 1) ^ 0xA6BC;
```
✅ **Correct.** DNP3 uses CRC-16-DNP (also known as CRC-16-DNP3). The reflected polynomial for the standard DNP3 polynomial (0x3D65 normal form) is **0xA6BC**. The final `!crc` (bitwise NOT) is also correct per the DNP3 spec (inverted remainder).

#### Data Link Frame Format
Per IEEE 1815:
- Start bytes: `0x05 0x64` ✅
- Length field: 1 byte (byte index 2) ✅
- Control byte: 1 byte ✅
- Destination address: 2 bytes little-endian ✅
- Source address: 2 bytes little-endian ✅
- CRC: 2 bytes over first 8 bytes ✅

Total header: 10 bytes ✅

```rust
if data[0] != 0x05 || data[1] != 0x64 { return Err(...) }
let destination = u16::from_le_bytes([data[4], data[5]]);
let source = u16::from_le_bytes([data[6], data[7]]);
let header_crc = u16::from_le_bytes([data[8], data[9]]);
```
✅ All byte positions correct.

#### Application Control Byte
```rust
fir: byte & 0x80 != 0,  // bit 7
fin: byte & 0x40 != 0,  // bit 6
con: byte & 0x20 != 0,  // bit 5
uns: byte & 0x10 != 0,  // bit 4
seq: byte & 0x0F,       // bits 3–0
```
✅ Correct per IEEE 1815 Table 8-1.

#### Data Object Groups
| Group | Variation | Description | Status |
|-------|-----------|-------------|--------|
| 1 | 1 | Binary Input packed | ✅ parsed |
| 1 | 2 | Binary Input with flags | ✅ parsed |
| 20 | 1 | Counter 32-bit with flags | ✅ parsed |
| 30 | 1 | Analog Input 32-bit with flags | ✅ parsed |
| 30 | 2 | Analog Input 16-bit with flags | ✅ parsed |
| 30 | 5 | Analog Input float with flags | ✅ parsed |
| 50 | 1 | Time and Date (48-bit ms) | ✅ parsed |

⚠️ **Minor:** Binary Input group 1 var 2 flags: the spec defines bit 7 as the value flag and bit 0 as the online/restart flag. The implementation reads `flags & 0x80` for value and `flags & 0x01` for online — correct. However, for group 1 var 1 (packed), the implementation reads the entire byte as the "value" bit, which is correct for the packed format where bits represent individual point values.

#### Function Codes
✅ All standard DNP3 function codes (0x00–0x17, 0x81, 0x82) correctly mapped.

**No polynomial errors. Data link layer format correct.**

---

## Prioritized Issue List

### P0 — Critical (Functional Failure)
1. **`ais.rs`**: `parse_nmea_sentence()` always returns `None`. The standalone AIS connector cannot decode any real AIS data. Should delegate to `nmea.rs::ais_decoder`.

### P1 — High (Spec Non-Compliance)
2. **`adsb.rs`**: No Mode S binary framing (DF extraction, CRC-24, CPR). Only SBS format supported. Document clearly or implement Mode S binary layer.
3. **`stix.rs`**: Missing `spec_version` field in `StixObject`. Required by STIX 2.1 §3.2 for all domain objects.

### P2 — Medium (Silent Acceptance of Invalid Data)
4. **`stix.rs`**: `created` and `modified` declared optional. These are required per STIX 2.1 for all SDOs/SROs.
5. **`stix.rs`**: No per-type required field validation (e.g. `indicator` missing `pattern` would be accepted silently).

### P3 — Minor (Edge Cases)
6. **`asterix.rs`** I048/130: Simplified compound subfield parsing. Sufficient for common cases; does not handle extended compound subfields.
7. **`metar.rs`**: Compound fractional visibility (`1 1/2SM` as two tokens) not handled.
8. **`dnp3.rs`**: Binary input group 1 var 1 packed format interprets full byte value rather than per-bit values. For multi-point reads this would need bit extraction per point index.

---

## Conclusion

The mathematical foundations (checksums, CRCs, coordinate scaling, unit conversions) are **uniformly correct** across all parsers. The NMEA 0183, NMEA 2000, ASTERIX, CEF, Modbus, and DNP3 implementations are specification-compliant and production-ready.

The three areas requiring attention are:
- The `ais.rs` connector stub (P0 — non-functional for real AIS)
- The `adsb.rs` SBS-only limitation (P1 — should be documented)
- STIX 2.1 `spec_version` and required-field enforcement (P1/P2)
