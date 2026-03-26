# ORP Connector Parser Audit Report

**Auditor:** Sentinel (automated protocol correctness review)  
**Date:** 2026-03-26  
**Scope:** `/Users/deepred/orp/crates/orp-connector/src/adapters/` — all 25 `.rs` files  
**Focus areas:** Malformed input safety · Checksum/CRC validation · Coordinate conversion correctness · Edge case coverage · Test data authenticity · `unwrap()` on user data

---

## Summary Table

| File | Tests | Prod `unwrap()` | Panic Risk | Checksum | Grade |
|------|-------|-----------------|------------|----------|-------|
| `adsb.rs` | 9 | 0 | None | N/A | **A** |
| `ais.rs` | 12 | 1 (safe) | None | N/A (stub parser) | **B** |
| `asterix.rs` | 19 | 0 | Low (ext-loop) | N/A (protocol) | **A-** |
| `canbus.rs` | 13 | 1 (safe fallback) | None | N/A | **A-** |
| `cap.rs` | 18 | 0 | None | N/A (XML) | **A** |
| `cot.rs` | 23 | 0 | None | N/A (XML) | **A** |
| `csv_watcher.rs` | 3 | 0 | None | N/A | **B** |
| `database.rs` | 18 | 3 (mutex poison) | Mutex panic | N/A | **B+** |
| `dnp3.rs` | 15 | 0 | None | ✅ CRC-16-DNP | **A** |
| `generic_api.rs` | 21 | 0 | None | N/A | **A** |
| `geojson.rs` | 16 | 0 | None | N/A | **A-** |
| `gtfs.rs` | 11 | 0 | None | N/A | **A-** |
| `http_poller.rs` | 3 | 0 | None | N/A | **B** |
| `metar.rs` | 23 | 0 | None | N/A (text) | **A** |
| `mod.rs` | 0 | 0 | None | N/A | N/A |
| `modbus.rs` | 22 | 0 | None | ✅ CRC-16 RTU | **A** |
| `mqtt.rs` | 3 | 0 | None | N/A | **B** |
| `netflow.rs` | 17 | 0 | None | N/A | **A-** |
| `nmea.rs` | 44 | 1 (logically safe) | None | ✅ XOR validated | **A** |
| `opcua.rs` | 21 | 0 | None | N/A | **A** |
| `pcap.rs` | 17 | 10 (post-guard) | Very low | N/A | **B+** |
| `sparkplugb.rs` | 12 | 0 | None | N/A | **A-** |
| `stix.rs` | 18 | 0 | None | N/A | **A** |
| `syslog.rs` | 22 | 0 | None | N/A | **A** |
| `websocket_client.rs` | 3 | 0 | None | N/A | **B** |
| `zeek.rs` | 16 | 0 | None | N/A | **A-** |

---

## Per-File Detailed Findings

---

### `adsb.rs` — ADS-B SBS BaseStation (TCP/CSV)
**Tests:** 9 | **Grade: A**

- ✅ All field parsing uses `.ok()?` or `.and_then(|s| s.parse().ok())` — no panics on malformed input.
- ✅ Empty ICAO rejected explicitly.
- ✅ Msg type filter (only types 2 & 3 accepted).
- ✅ Demo mode uses plausible ICAO hex codes / callsigns.
- ✅ Test data is realistic SBS format from dump1090.
- ✅ `on_ground` handles `-1`, `0`, `1` variants correctly.
- ✅ No `unwrap()` in production code paths.
- **Missing edge cases:**
  - No test for huge payload (>1 MB line).
  - No test for non-ASCII bytes embedded in a line.
  - No test for `MSG,3` with NaN/Inf floats in lat/lon.
  - Beast binary format parsing is not implemented (only SBS).

---

### `ais.rs` — AIS NMEA / CSV connector
**Tests:** 12 | **Grade: B**

- ⚠️ **`parse_nmea_sentence` always returns `None`** — the entire real AIS decoding is stubbed out. The comment says "real AIS uses 6-bit ASCII encoding" but provides no implementation. Real NMEA AIS parsing lives in `nmea.rs`.
- ✅ `parse_csv_line`: one `unwrap_or(course)` at line 57 (safe — falls back to course value).
- ✅ Insufficient fields → returns `None`.
- ✅ Invalid lat/lon → returns `None` via `parse().ok()?`.
- ❌ **No AIS coordinate conversion tested** in this file — the CSV format takes pre-decoded decimal degrees, so ddmm.mmmm → decimal is not relevant here but the real NMEA path is dead code.
- **Missing edge cases:**
  - MMSI boundary validation (should be 9 digits, 200000000–799999999).
  - Speed/course range validation (speed ≥ 0, course 0–359).
  - No test for empty MMSI string.

---

### `asterix.rs` — ASTERIX Cat 048/062 (binary, UDP)
**Tests:** 19 | **Grade: A-**

- ✅ All binary field accesses are bounds-checked with explicit error returns.
- ✅ Data-block length check before any record parsing.
- ✅ Unknown category handled gracefully (raw bytes stored).
- ✅ WGS-84 coordinate scaling correct for Cat 062: `raw * (180.0 / 2^25)`.
- ✅ ICAO 6-bit decoding implemented and tested.
- ✅ No `unwrap()` in production code.
- ⚠️ **I048/020 extension loop (line ~183):** The `while data[offset-1] & 0x01 != 0` loop silently breaks on data exhaustion instead of returning `Err`. Truncated extension fields go undetected.
- ⚠️ **No checksum** — ASTERIX data blocks don't have mandatory CRC (per spec), but the code doesn't validate CAT field against expected values for Cat 048/062. An adversary sending CAT=48 data with Cat 062 record bytes would be decoded incorrectly. This is inherent to the protocol but worth noting.
- **Missing edge cases:**
  - Zero-length record data (LEN=3, i.e., header only with no records).
  - Malicious large REP field in I048/250 MB Data (could cause `offset + mb_len > data.len()` — currently handled via `Err` check ✅).
  - No test for I048/130 compound subfields.
  - FSPEC with all FX bits set (max extension depth) — loop terminates correctly at data boundary.

---

### `canbus.rs` — CAN Bus / J1939 (candump log)
**Tests:** 13 | **Grade: A-**

- ✅ `parse_candump_line`: all fields via `parse().ok()` or `?` — no panics.
- ✅ `parse_can_frame_bytes`: explicit 16-byte minimum check.
- ✅ J1939 PGN data decoders check `data.len()` before indexing.
- ✅ One `unwrap_or_else(Utc::now)` at line 390 — safe fallback.
- ⚠️ **`parse_can_frame_bytes`:** `dlc = data[4].min(8)` but then accesses `data[8..8 + dlc as usize]` — if `data` is exactly 16 bytes and `dlc` is 8, `data[8..16]` is valid. However, there's no check that `8 + dlc as usize <= data.len()`. If a caller passes exactly 9 bytes (which fails the `< 16` check) this is safe; but if someone passes 16 bytes with DLC > 8 (before min), the min clamp protects it. **Safe as-is** but fragile.
- **Missing edge cases:**
  - ODD-length hex data string in candump (e.g., `1A3#A`) — `step_by(2)` with `i + 2 <= data_str.len()` check handles it correctly ✅.
  - No test for CAN FD frames (DLC > 8).
  - J1939 EngineSpeed with `data.len() < 5` (only 4 bytes) — the `if data.len() >= 4` check correctly skips RPM read ✅.

---

### `cap.rs` — CAP 1.2 XML (Emergency Alerts)
**Tests:** 18 | **Grade: A**

- ✅ XML parsing via `quick_xml` with SAX-style state machine — no panics on malformed XML (returns `Err`).
- ✅ All text extraction uses `.unescape().unwrap_or_default()` — safe.
- ✅ Namespace-aware tag parsing via `rsplit(':').next().unwrap_or(&tag)` — safe.
- ✅ CAP enum variants handle unknown values via `Unknown(String)`.
- ✅ `parse_cap_circle` and `parse_cap_polygon_centroid` tested with real CAP data.
- ✅ Test XML matches real CAP 1.2 schema (`urn:oasis:names:tc:emergency:cap:1.2`).
- **Missing edge cases:**
  - CAP document with no `<info>` element.
  - Deeply nested XML (stack overflow risk with recursive descent parsers — SAX approach used here mitigates this).
  - Malformed polygon with odd number of coordinates.
  - Circle with negative radius.

---

### `cot.rs` — Cursor-on-Target XML (TAK/ATAK)
**Tests:** 23 | **Grade: A**

- ✅ XML parsing with `quick_xml` — malformed XML returns `Err`.
- ✅ Lat/lon attribute parsing uses `val.parse().unwrap_or(0.0)` — silently defaults to 0.0 on malformed data. Acceptable for CoT (0.0/0.0 is an obviously invalid position).
- ✅ CoT type classification (`a-f-G-U-C`, etc.) is robust with fallback to `"unknown"`.
- ✅ Round-trip serialization tested (serialize → parse → compare).
- ✅ Test data is real CoT format matching ATAK specification.
- **Missing edge cases:**
  - No test for stale or future timestamps (CoT has `stale` attribute).
  - No test for `how` attribute edge cases.
  - Lat/lon silently defaulting to 0.0/0.0 on garbage input means a malformed position looks like "Gulf of Guinea" — no validation against ±90/±180 bounds.

---

### `csv_watcher.rs` — CSV file watcher
**Tests:** 3 | **Grade: B**

- ✅ No `unwrap()` in production code.
- ✅ Field mapping via config keys — missing keys default safely.
- ⚠️ **Only 3 tests** — the parser is mostly configuration-driven with little protocol logic, but integration tests are sparse.
- **Missing edge cases:**
  - Empty CSV file.
  - CSV with header only, no data rows.
  - CSV with mismatched column counts between rows.
  - Files with Windows line endings (`\r\n`).

---

### `database.rs` — Generic database connector
**Tests:** 18 | **Grade: B+**

- ⚠️ **3 `mutex.lock().unwrap()` calls** (lines 374, 428, 437) in non-test async code. If any thread panics while holding the watermark mutex, subsequent calls will panic with "poisoned lock". This is a known Rust pattern and unlikely in practice, but not zero-risk in production.
- ✅ All SQL operations parameterized via query builder — no SQL injection.
- ✅ Config parsing uses `get(key).ok_or_else(...)` — returns `Err` on missing keys.
- ✅ Row-to-event conversion handles type mismatches gracefully.
- **Missing edge cases:**
  - No test for DB connection failure recovery.
  - No test for very wide rows (hundreds of columns).

---

### `dnp3.rs` — DNP3 (SCADA/ICS binary protocol)
**Tests:** 15 | **Grade: A**

- ✅ **CRC-16-DNP implemented and validated** on link layer header (lines 375-380) — returns `Err` on mismatch.
- ✅ CRC table pre-computed at compile time via `const`.
- ✅ All binary field accesses bounds-checked before indexing.
- ✅ Link layer header: START bytes (0x0564) validated.
- ✅ Length field validated against available data.
- ✅ JSON ingestion path (`parse_dnp3_json`) handles missing/null fields via `unwrap_or_default()`.
- ✅ CRC test uses real DNP3 polynomial (0xA6BC).
- **Missing edge cases:**
  - No test for data blocks with CRC embedded every 16 bytes (only link header CRC tested).
  - Application layer function code validation (only some codes decoded).
  - No test for truncated application layer with valid link header CRC.

---

### `generic_api.rs` — Generic HTTP API poller
**Tests:** 21 | **Grade: A**

- ✅ JSON field extraction all via `.get(key).and_then(...)` chains — no panics.
- ✅ Template system (`builtin_template`) returns `Result`.
- ✅ Field path navigation uses `unwrap_or` fallbacks throughout.
- ✅ Rate limit / error handling tested.
- **Missing edge cases:**
  - No test for HTTP 429 (rate limit) response handling.
  - No test for non-UTF8 response bodies.

---

### `geojson.rs` — GeoJSON FeatureCollection
**Tests:** 16 | **Grade: A-**

- ✅ All JSON field access uses `Option` chains — no panics on missing fields.
- ✅ Unknown geometry types handled gracefully.
- ✅ `.unwrap_or((0.0, 0.0))` for centroid on geometry-less features — silent, but 0.0/0.0 is a detectable sentinel.
- ✅ Feature collection test uses real GeoJSON RFC 7946 format.
- **Missing edge cases:**
  - No test for deeply nested `GeometryCollection`.
  - No test for empty `coordinates` arrays (e.g., `{"type":"Polygon","coordinates":[]}`).
  - Polygon centroid with collinear points / zero-area polygon.

---

### `gtfs.rs` — GTFS-RT (Transit Realtime protobuf via JSON)
**Tests:** 11 | **Grade: A-**

- ✅ JSON path access all via `.get().and_then()` — no panics.
- ✅ Timestamp handling uses `unwrap_or_else(Utc::now)` — safe fallback.
- ✅ Entity type determined from presence of `vehicle`/`trip_update`/`alert` fields.
- ✅ Test data matches real GTFS-RT JSON structure.
- **Missing edge cases:**
  - No test for protobuf binary input (only JSON path implemented).
  - No test for entities missing both vehicle and trip_update.
  - Speed/bearing range validation missing (negative speed accepted).

---

### `http_poller.rs` — HTTP JSON poller
**Tests:** 3 | **Grade: B**

- ✅ No `unwrap()` in production code.
- ✅ Config defaults via `unwrap_or`.
- ⚠️ **Only 3 tests** — only tests config parsing, not the actual HTTP fetch/parse path.
- **Missing edge cases:**
  - No test for non-JSON response bodies.
  - No test for HTTP redirect loops.
  - No test for extremely large JSON payloads.

---

### `metar.rs` — METAR weather reports (text)
**Tests:** 23 | **Grade: A**

- ✅ All field parsing uses regex matching or token splitting with explicit `Option` returns.
- ✅ `std::str::from_utf8(chunk).unwrap_or("")` (line 443) — safe fallback for weather phenomenon codes.
- ✅ Wind parsing handles VRB (variable) direction explicitly.
- ✅ Visibility parsing handles fractional values (`3/4SM`), CAVOK, `9999`.
- ✅ Cloud layer parsing handles `CB`/`TCU` modifiers.
- ✅ Temperature parsing handles negative Celsius (`M02` = -2°C).
- ✅ Test data uses real METAR strings from KJFK, EGLL, KSFO, EDDF, CYUL, LFPG — authentic stations.
- **Missing edge cases:**
  - METAR with TEMPO/BECMG groups.
  - AUTO/COR/RTD modifiers.
  - Wind shear (`WS`) field.
  - METAR with runway visual range (RVR) — `R28L/2000FT`.

---

### `modbus.rs` — Modbus TCP/RTU
**Tests:** 22 | **Grade: A**

- ✅ **Modbus RTU CRC-16 implemented** with correct polynomial (0xA001) and verified.
- ✅ MBAP header parsing validates `unit_id` and `length` fields.
- ✅ Exception responses (FC | 0x80) handled — returns `ModbusError` variant.
- ✅ Register interpretation (`f32`, `u16`, `i16`, `u32`) with bounds checks.
- ✅ `RegisterInterpreter::read_f32` returns `None` on insufficient data.
- ✅ CRC test uses known Modbus RTU frame (device 1, FC03, addr 0000, qty 0001, CRC 0x0A84).
- **Missing edge cases:**
  - No test for Modbus TCP response with function code mismatch from request.
  - No test for coil/discrete-input response parsing (FC01/FC02).
  - Maximum register count validation (Modbus spec: max 125 holding registers per request).

---

### `mqtt.rs` — MQTT connector
**Tests:** 3 | **Grade: B**

- ✅ No `unwrap()` in production code.
- ✅ Topic-to-entity mapping via config.
- ⚠️ **Only 3 tests** — connector lifecycle not tested.
- **Missing edge cases:**
  - Retained message handling.
  - QoS level validation.
  - Malformed UTF-8 in topic strings.
  - Reconnect behavior on connection loss.

---

### `netflow.rs` — NetFlow v5/v9/IPFIX (UDP binary)
**Tests:** 17 | **Grade: A-**

- ✅ All binary reads bounds-checked before access.
- ✅ Version detection via 2-byte magic at offset 0.
- ✅ v5 header: `count` field validated against actual packet length.
- ✅ `unwrap_or_else(Utc::now)` for timestamp fallback — safe.
- ✅ `unwrap_or("0.0.0.0")` for IP address fallback — acceptable sentinel.
- ⚠️ **IPFIX**: only version detection implemented — actual IPFIX record parsing not done, returns minimal stub. No test covers IPFIX record content.
- **Missing edge cases:**
  - v5 packet with `count` field > actual records (buffer over-read check present via bounds check ✅).
  - v9 template with 0 fields.
  - Fragmented NetFlow packets (partial headers).

---

### `nmea.rs` — NMEA 0183 + AIS (comprehensive)
**Tests:** 44 | **Grade: A**

- ✅ **NMEA XOR checksum validated** on every sentence — rejects invalid checksums.
- ✅ **AIS coordinate conversion correct**: `raw / 600_000.0` (1/10000 minute × 1/60 degrees/minute = 1/600000) — matches ITU-R M.1371.
- ✅ **NMEA ddmm.mmmm → decimal correct**: splits at `dot - 2` for minutes, divides by 60.
- ✅ AIS 6-bit payload decoder handles fill bits, invalid characters (val > 63 → `None`).
- ✅ `BitBuffer::get_bits` bounds-checked — returns `None` on out-of-range.
- ✅ Multi-part AIS message assembly with part deduplication.
- ✅ `parse_sentence` returns `None` (not panic) on all malformed inputs.
- ⚠️ **Line 739**: `self.parts.remove(&key).unwrap()` — the key was just used as an index 2 lines above, so this is logically infallible. However, technically a panic surface if code is refactored. Should be `expect("key was just validated")` at minimum.
- ✅ Test sentences are real NMEA 0183 data (GGA, RMC, AIS type 1/5/18/21 with valid checksums).
- **Missing edge cases:**
  - AIS unavailable position sentinel (lon=181°, lat=91°) not filtered — caller receives sentinel coords as real position.
  - NMEA sentences from non-GPS talkers (e.g., `$PSRF...` proprietary) not rejected gracefully (they fall through to no-match and return `None` ✅).
  - AIS type 14 (safety-related broadcast) not decoded.
  - AIS type 24 (Class B static) not decoded.

---

### `opcua.rs` — OPC-UA connector
**Tests:** 21 | **Grade: A**

- ✅ NodeId parsing handles numeric, string, and GUID variants.
- ✅ Quality/status code decoding via bitmask.
- ✅ JSON ingestion for node values — all via `Option` chains.
- ✅ Timestamp fallback via `unwrap_or_else(Utc::now)`.
- ✅ Test data uses real OPC-UA NodeId formats (`ns=2;s=...`).
- **Missing edge cases:**
  - No test for OPC-UA array-valued nodes.
  - No test for very large node value payloads.

---

### `pcap.rs` — PCAP file parser (network captures)
**Tests:** 17 | **Grade: B+**

- ⚠️ **10 `unwrap()` calls in production code** (lines 267-272, 287-290). All are guarded by prior bounds checks:
  - `parse_pcap_global_header` checks `data.len() < 24` before calling `read_u16`/`read_u32` on offsets 4-20 → safe.
  - `parse_pcap_packet_header` checks `offset + 16 > data.len()` before the four `read_u32` calls → safe.
  - However, this pattern is fragile: if a bounds check is ever moved or loosened during refactoring, 10 panics become possible. **Should be converted to `?` with proper error propagation.**
- ✅ Ethernet/IPv4/TCP/UDP parsers all have explicit minimum-length checks.
- ✅ PCAPNG format detected and rejected cleanly.
- ✅ Magic number validated — unknown magic returns `Err`.
- ✅ Packet `incl_len` validated against remaining data before access.
- ✅ IPv4 header validates `version == 4` and `ihl >= 5`.
- **Missing edge cases:**
  - No test for IPv6 packets (ethertype 0x86DD).
  - No test for VLAN-tagged frames (ethertype 0x8100 with 4-byte tag).
  - No test for jumbo frames (incl_len > snaplen).
  - IPv4 fragmented packets (fragment_offset > 0) not reassembled — this is by design for a summary tool.

---

### `sparkplugb.rs` — Sparkplug B (MQTT/Protobuf via JSON)
**Tests:** 12 | **Grade: A-**

- ✅ Topic parsing validates `spBv1.0/` prefix and correct segment count.
- ✅ Metric value decoding via datatype code — all branches guarded.
- ✅ `unwrap_or_default()` for missing `metrics` array — safe.
- ✅ Timestamp fallback `unwrap_or_else(Utc::now)`.
- ✅ Test data uses real Sparkplug B topic format.
- **Missing edge cases:**
  - Datatype code 0 (unknown) handled via raw JSON ✅.
  - No test for death certificate (NDEATH/DDEATH) messages.
  - No test for rebirth request handling.

---

### `stix.rs` — STIX 2.1 / TAXII (JSON threat intel)
**Tests:** 18 | **Grade: A**

- ✅ All JSON access via `.get().and_then()` — no panics on malformed STIX.
- ✅ Object type dispatch handles unknown types gracefully.
- ✅ TAXII discovery/collections parsing robust.
- ✅ Test bundle contains real STIX 2.1 SDO types (indicator, malware, threat-actor).
- ✅ STIX pattern field preserved as raw string (not interpreted — correct for this use case).
- **Missing edge cases:**
  - No test for STIX bundle with malformed `objects` array (non-object items).
  - No test for relationship objects (STIX SRO).
  - `confidence` field: no validation that value is in 0-100 range.

---

### `syslog.rs` — Syslog RFC 3164/5424 + CEF
**Tests:** 22 | **Grade: A**

- ✅ RFC 5424 structured data parsing handles missing fields via `unwrap_or("")`.
- ✅ CEF extension parsing handles escaped `=` and `\n` in values.
- ✅ Priority octet decoded: facility × 8 + severity validated.
- ✅ `parse_rfc5424` falls back gracefully on bad timestamps.
- ✅ Test data is real syslog output format from common systems.
- ✅ `severity_str.trim().parse().unwrap_or(0)` — safe, only parses a sub-token.
- **Missing edge cases:**
  - Syslog message with embedded NUL bytes.
  - Extremely long hostname (>255 chars — RFC limit).
  - CEF extension with unbalanced quotes.

---

### `websocket_client.rs` — WebSocket client connector
**Tests:** 3 | **Grade: B**

- ✅ No `unwrap()` in production code.
- ⚠️ **Only 3 tests** — connector lifecycle minimally tested.
- **Missing edge cases:**
  - Binary WebSocket frames vs text frames.
  - WebSocket close frame handling.
  - Reconnect on disconnect.
  - Malformed JSON payload from server.

---

### `zeek.rs` — Zeek TSV log parser
**Tests:** 16 | **Grade: A-**

- ✅ Header parsing (`#fields`, `#types`, `#separator`) is robust.
- ✅ Field count mismatch between header and data rows handled (zips `fields` with `types`).
- ✅ Timestamp parsing returns `Err` on invalid format.
- ✅ `unwrap_or('\t')` for separator default — safe.
- ✅ Unset values (`-`) and empty values handled.
- ✅ Test data is real Zeek conn.log / dns.log / http.log / ssl.log TSV format.
- **Missing edge cases:**
  - No test for `#close` timestamp header.
  - Zeek log with UTF-8 in HTTP URI fields — handled by string pass-through ✅.
  - Log rotation mid-stream (partial header).

---

## Critical Issues (Must Fix)

### 🔴 HIGH: `pcap.rs` — 10 `.unwrap()` on `Option` returns (lines 267-272, 287-290)
Although currently guarded by prior bounds checks, these are maintenance liabilities. If the guard is ever changed, 10 panics appear. Convert to `?` with `ok_or_else(|| ConnectorError::ParseError(...))`.

```rust
// Current (fragile):
version_major: read_u16(data, 4, endian).unwrap(),

// Fix:
version_major: read_u16(data, 4, endian)
    .ok_or_else(|| ConnectorError::ParseError("PCAP: truncated global header".into()))?,
```

### 🔴 MEDIUM: `database.rs` — 3 `mutex.lock().unwrap()` in async runtime
```rust
// Lines 374, 428, 437
let mut lock = self.watermark.lock().unwrap();
```
If another task panics while holding the watermark lock, all subsequent calls panic with "poisoned lock". Convert to:
```rust
let mut lock = self.watermark.lock().unwrap_or_else(|e| e.into_inner());
```

---

## Protocol Correctness Summary

| Protocol | Checksum Required | Status |
|----------|------------------|--------|
| NMEA 0183 | XOR checksum | ✅ Validated and enforced |
| AIS (via NMEA) | Via NMEA checksum | ✅ Inherited |
| DNP3 | CRC-16-DNP | ✅ Link header validated |
| Modbus RTU | CRC-16 | ✅ Verified |
| Modbus TCP | MBAP framing | ✅ Length validated |
| ASTERIX | No mandatory CRC | N/A — length field validated |
| CAN Bus | Hardware CRC | N/A — not available in log format |
| PCAP | Magic number | ✅ Validated |
| NetFlow v5 | No CRC | N/A — count vs length validated |

---

## Coordinate Conversion Correctness

| Parser | Conversion | Correct? |
|--------|-----------|---------|
| `nmea.rs` NMEA | ddmm.mmmm → decimal: `deg + min/60` | ✅ Correct |
| `nmea.rs` AIS | 1/10000 minute: `raw / 600_000.0` | ✅ Correct (ITU-R M.1371) |
| `asterix.rs` Cat 062 | `raw * (180.0 / 2^25)` | ✅ Correct (EUROCONTROL spec) |
| `asterix.rs` Cat 048 | Polar (rho/theta) — no WGS-84 | N/A — raw polar coords |

---

## Test Data Authenticity

| File | Real Data? | Notes |
|------|-----------|-------|
| `nmea.rs` | ✅ Yes | Real NMEA sentences with valid checksums |
| `adsb.rs` | ✅ Yes | Real SBS format from dump1090 |
| `metar.rs` | ✅ Yes | Real METAR from KJFK, EGLL, KSFO, EDDF, CYUL, LFPG |
| `asterix.rs` | ✅ Yes | Hand-crafted but protocol-correct binary frames |
| `dnp3.rs` | ✅ Yes | Real CRC values tested |
| `modbus.rs` | ✅ Yes | Known CRC verified against spec |
| `syslog.rs` | ✅ Yes | Real RFC 3164/5424/CEF format |
| `zeek.rs` | ✅ Yes | Real Zeek conn.log/dns.log format |
| `cap.rs` | ✅ Yes | Real CAP 1.2 with proper namespace |
| `cot.rs` | ✅ Yes | Real CoT format (ATAK compatible) |
| `stix.rs` | ✅ Yes | Real STIX 2.1 SDO bundle |
| `canbus.rs` | ✅ Yes | Real candump format, real J1939 CAN ID |
| `ais.rs` | ⚠️ CSV only | NMEA path is stubbed — not tested with real AIS sentences |

---

## Recommendations by Priority

### P0 (Fix Before Merge)
1. **`pcap.rs`**: Convert 10 production `unwrap()` calls to `?` — cosmetically safe now but a refactoring time bomb.
2. **`database.rs`**: Handle poisoned mutex with `unwrap_or_else(|e| e.into_inner())`.

### P1 (Fix in Next Sprint)
3. **`ais.rs`**: The `AisConnector::parse_nmea_sentence` stub must be implemented (or removed and replaced with delegation to `nmea.rs`). Currently advertising capability it doesn't deliver.
4. **`nmea.rs`**: Filter AIS sentinel positions (lon=0x6791AC0 = 181°, lat=0x3412140 = 91°) before publishing to consumers.
5. **`asterix.rs`**: I048/020 extension loop should return `Err` on truncation, not silently break.

### P2 (Improve Coverage)
6. **`csv_watcher.rs`, `http_poller.rs`, `mqtt.rs`, `websocket_client.rs`**: All have ≤3 tests. Add edge case tests for empty payloads, malformed JSON, and connection loss.
7. **`cot.rs`**: Add lat/lon range validation (lat ±90, lon ±180).
8. **`netflow.rs`**: Implement IPFIX record parsing (currently only version detection).

---

*Audit generated by automated review. Manual review recommended for P0/P1 items before production deployment.*
