# ORP Connector Parser Audit — Version 2

**Auditor:** Senior Protocol Correctness Engineer  
**Date:** 2026-03-27  
**Scope:** All 32 parser files in `/Users/deepred/orp/crates/orp-connector/src/adapters/`  
**Methodology:** Static analysis of every production (non-test) code path; verification of all five issues flagged in the previous audit; review for panic paths, integer overflow, infinite loops, and protocol correctness.

---

## Executive Summary

The five previously-flagged issues were re-examined. **Three remain broken, one is technically safe but still bad style, and one is partially mitigated.**

| # | File | Previous Finding | Status |
|---|------|-----------------|--------|
| 1 | `pcap.rs` | 10 unwrap() in production | **STILL PRESENT** — safe by pre-check, but fragile |
| 2 | `database.rs` | 3 mutex lock().unwrap() in async runtime | **STILL PRESENT** — real panic/deadlock risk |
| 3 | `ais.rs` | parse_nmea_sentence() always returns None | **STILL BROKEN** — function is a stub |
| 4 | `nmea.rs` | AIS sentinel positions not filtered | **STILL BROKEN** — lat=91°/lon=181° published as real coords |
| 5 | `asterix.rs` | Extension loop silently truncates | **STILL BROKEN** — break instead of error |

**Overall fleet status: 4 files require immediate fixes before production use.**

---

## Grading Scale

| Grade | Meaning |
|-------|---------|
| A | No issues; production-safe |
| B | Minor style/robustness issues; no panic paths |
| C | Code smell or minor correctness issues; low panic risk |
| D | Real panic paths or correctness bugs under plausible inputs |
| F | Broken functionality or certain panic under untrusted input |

---

## Per-File Findings

---

### 1. `acars.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All 5 `.unwrap()` calls are inside `#[cfg(test)]`.

**Panic paths:** None identified.  
**Correctness:** `parse_acars_text` and `parse_acars_raw` both return `Result`/`Option`. Empty input returns `Err`. Field parsing uses `.ok()` chaining.  
**Integer overflow:** No length arithmetic on untrusted data.  
**Infinite loops:** None.

**Fix needed:** None.

---

### 2. `adsb.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All 2 `.unwrap()` calls are inside `#[cfg(test)]`.

**Panic paths:** None. ADS-B bit extraction uses bounds-checked `get_bits()` pattern.  
**Correctness:** CPR decoding uses standard Even/Odd algorithm; altitude encoding handles Gillham code and 25ft/100ft increments correctly.  
**Integer overflow:** No issues.

**Fix needed:** None.

---

### 3. `ais.rs` — Grade: **F**

**Production unwrap() calls:** 0

**CRITICAL — FUNCTIONAL STUB:**

```rust
// ais.rs:29-42
pub fn parse_nmea_sentence(sentence: &str) -> Option<AisMessage> {
    let parts: Vec<&str> = sentence.split(',').collect();
    if parts.len() < 7 {
        return None;
    }
    if !parts[0].starts_with("!AIVDM") && !parts[0].starts_with("!AIVDO") {
        return None;
    }
    // For demo purposes, parse a simplified CSV-like AIS format
    // Real implementation would decode the binary payload in parts[5]
    None   // <--- ALWAYS RETURNS None
}
```

This function is the primary entry point for the AIS TCP connector. Every real `!AIVDM`/`!AIVDO` sentence received from a live AIS feed or AIS multiplexer will silently return `None`. Zero position reports are emitted. The AIS connector is **completely non-functional** for its stated purpose.

The `parse_csv_line` alternative exists for testing only and accepts a non-standard CSV format that no real AIS source produces.

**Panic paths:** None (the stub just returns None).

**Fix required:**
```rust
pub fn parse_nmea_sentence(sentence: &str) -> Option<AisMessage> {
    let parts: Vec<&str> = sentence.split(',').collect();
    if parts.len() < 7 {
        return None;
    }
    if !parts[0].starts_with("!AIVDM") && !parts[0].starts_with("!AIVDO") {
        return None;
    }
    let payload = parts[5];
    let fill_bits: u8 = parts[6].trim_end_matches('*')
        .split('*').next()?.parse().ok()?;
    // Delegate to the fully-implemented ais_decoder in nmea.rs
    // (or replicate the decode_ais logic here)
    let bytes = ais_decoder::decode_payload(payload, fill_bits)?;
    let total_bits = payload.len() * 6 - fill_bits as usize;
    let buf = ais_decoder::make_buffer(&bytes, total_bits);
    let d = ais_decoder::decode_type_1_2_3(&buf)?;
    // filter sentinels
    if d.lon.abs() > 180.0 || d.lat.abs() > 90.0 { return None; }
    Some(AisMessage { mmsi: d.mmsi.to_string(), latitude: d.lat, longitude: d.lon, ... })
}
```

---

### 4. `asterix.rs` — Grade: **C**

**Production unwrap() calls:** 0  
All `panic!` calls are inside `#[cfg(test)]` (module starts at line 830).

**ISSUE 1 — Silent truncation in extension loop (lines 182–188):**

```rust
// asterix.rs:177-188
if first & 0x01 != 0 {
    if offset >= data.len() {
        return Err(ConnectorError::ParseError(
            "ASTERIX Cat048: truncated I048/020 ext".to_string(),
        ));
    }
    offset += 1; // skip extension byte
    // More extensions possible
    while offset > 0 && data[offset - 1] & 0x01 != 0 {
        if offset >= data.len() {
            break;  // <--- SILENT TRUNCATION, should return Err
        }
        offset += 1;
    }
}
```

The first extension byte correctly returns an error if missing. But subsequent extension bytes silently `break` when the buffer is exhausted. This means a crafted packet with the FX bit set on every extension byte but truncated data will be partially parsed without any error indication. Downstream consumers see incomplete records as if they were valid.

**ISSUE 2 — I048/130 subfield parsing assumes remaining buffer:**

The compound field I048/130 iterates over subfields in a nested loop. Truncated subfields return an error, but the subfield loop doesn't verify the total remaining bytes before starting, so a malformed subfield presence bitmap could cause multiple error returns mid-record, leaving the offset in an inconsistent state for subsequent records in the same data block.

**Fix required for Issue 1:**
```rust
while offset > 0 && data[offset - 1] & 0x01 != 0 {
    if offset >= data.len() {
        return Err(ConnectorError::ParseError(
            "ASTERIX Cat048: truncated I048/020 ext (continuation)".to_string(),
        ));
    }
    offset += 1;
}
```

---

### 5. `bacnet.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All 8 `.unwrap()` calls are inside `#[cfg(test)]`.

**Panic paths:** None. BACnet tag parsing uses explicit length guards.  
**Correctness:** Object ID encoding (10-bit type, 22-bit instance) is correct per BACnet standard. APDU type decoding covers all standard service codes.  
**Integer overflow:** Tag length computation uses `checked_add` pattern implicitly via bounds checks.

**Fix needed:** None.

---

### 6. `canbus.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All 2 `.unwrap()` calls are inside `#[cfg(test)]`.

**Panic paths:** None.  
**Correctness:** CAN ID masking (0x1FFFFFFF for extended, 0x7FF for standard) correct. DLC validation (max 8 bytes) enforced. Timestamp parsing uses `f64` which can represent microsecond-resolution UNIX timestamps accurately up to year 2255.

**Fix needed:** None.

---

### 7. `cap.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All CAP XML parsing returns `Result`. All `.unwrap()` calls are in `#[cfg(test)]`.

**Panic paths:** None.  
**Correctness:** CAP 1.2 polygon parsing handles both `lat,lon` and `lon,lat` order; centroid algorithm is correct for convex polygons. Circle radius parsing converts km correctly.

**Fix needed:** None.

---

### 8. `cef.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Panic paths:** None.  
**Correctness:** CEF extension parser correctly handles escaped `\|` and `\\`. Severity mapping (0-10 scale) is correct.

**Fix needed:** None.

---

### 9. `cot.rs` — Grade: **A**

**Production unwrap() calls:** 0

**Panic paths:** `loop {}` at line 214 is in XML text-node walking; bounded by XML document size. No infinite loop risk.  
**Correctness:** CoT time parsing handles both ISO 8601 and epoch formats. Stale/unknown position handling is correct.

**Fix needed:** None.

---

### 10. `csv_watcher.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Panic paths:** None. CSV row parsing uses column index map, returns `None` on missing columns.

**Fix needed:** None.

---

### 11. `database.rs` — Grade: **D**

**Production unwrap() calls:** 3

```
database.rs:374  let mut lock = self.watermark.lock().unwrap();
database.rs:428  let wm = watermark.lock().unwrap().clone();
database.rs:437  let mut lock = watermark_clone.lock().unwrap();
```

**ISSUE 1 — Mutex poisoning panic:**

All three lines call `.lock().unwrap()` on a `std::sync::Mutex` in production code. If any thread panics while holding the watermark lock, the mutex becomes *poisoned*, and all subsequent `.lock().unwrap()` calls will **panic**. In a long-running connector polling a database, this means a single threading error cascades to kill the entire connector process.

**ISSUE 2 — std::sync::Mutex held across await points:**

Lines 428 and 437 are inside a `tokio::spawn(async move { ... })` block. At line 428, the lock is taken, `.clone()` is called (fast, no await), and the lock is dropped before any await. At line 437, the lock is held while iterating over `rows` (synchronous loop, no await). In the current code, no `.await` is called while holding the lock, so there is no *current* deadlock. However, this is fragile — any future refactor that adds an `.await` inside the locked section will deadlock the tokio executor thread.

**ISSUE 3 — False safety of the `#[allow(dead_code)]` watermark update method:**

`update_watermark` at line 374 is marked `#[allow(dead_code)]`, suggesting it is a duplicate of inline watermark logic. Having two paths for the same operation increases maintenance risk.

**Fix required:**

Replace `std::sync::Mutex` with `tokio::sync::Mutex` for async safety, and use `?` instead of `.unwrap()`:

```rust
// Before
let mut lock = self.watermark.lock().unwrap();

// After
let mut lock = self.watermark.lock().await;
// (watermark field type changes from Arc<Mutex<...>> to Arc<tokio::sync::Mutex<...>>)
```

If std::sync::Mutex is retained for performance, use `.lock().unwrap_or_else(|e| e.into_inner())` to recover from poisoning rather than panicking.

---

### 12. `dnp3.rs` — Grade: **B**

**Production unwrap() calls:** 0  
All `panic!` calls are inside `#[cfg(test)]` (module at line 899).

**Minor issue:** `parse_dnp3_application` uses `data[offset]` indexing inside a `while offset + 3 <= data.len()` loop, but then accesses `data[offset + 1]` and `data[offset + 2]` without separate bounds check — these are safe because the `while` condition guarantees 3 bytes remain.

**CRC verification:** DNP3 CRC is computed and checked; a mismatch returns an error, not a panic.

**Fix needed:** None (technically safe; minor code clarity could be improved).

---

### 13. `generic_api.rs` — Grade: **B**

**Production unwrap() calls:** 0  
One `.expect()` at line 813: `serde_json::to_value(&api_config).expect("serialise api config")` — this can only fail if `ApiConfig` contains a non-serializable type, which is a compile-time invariant. Risk is negligible.

**Panic paths:** `panic!` at line 1090 is inside `#[cfg(test)]`.  
All three `loop {}` blocks (lines 648, 691, 725) are in HTTP polling retry loops with `break` conditions and backoff; no infinite loop risk under normal operation.

**Fix needed:** Replace `.expect()` with `?` propagation for consistency.

---

### 14. `geojson.rs` — Grade: **A**

**Production unwrap() calls:** 0

**Correctness:** GeoJSON coordinate order is `[longitude, latitude]` per RFC 7946; the parser correctly maps index 0 to longitude and index 1 to latitude. Polygon centroid uses Shoelace formula — correct for non-self-intersecting polygons.

**Fix needed:** None.

---

### 15. `grib.rs` — Grade: **B**

**Production unwrap() calls:** 0

**Minor issue — integer truncation:**  
At line 764, `indicator.total_length as usize` converts a `u64` to `usize`. On 64-bit platforms this is safe. A GRIB2 message claiming `total_length = 0` is handled by `.max(16)` at line 803, preventing infinite loop.

**Minor issue — total_length = u64::MAX attack:**  
A crafted message with `total_length = u64::MAX` would compute `msg_end = (offset + usize::MAX).min(data.len())` — on 64-bit, `offset + usize::MAX` overflows. However, `data.len()` is bounded by the actual file read, so `.min(data.len())` produces a safe result. Not a practical attack vector for file-based input, but worth documenting.

**Fix needed:** Add explicit guard: `if msg_len == 0 || msg_len > data.len() - offset { offset += 1; continue; }`.

---

### 16. `gtfs.rs` — Grade: **A**

**Production unwrap() calls:** 0

**Correctness:** GTFS-RT vehicle position uses `latitude`/`longitude` fields from the protobuf spec. Speed conversion (m/s → knots) is not performed — speed is stored raw in m/s which is correct for the internal data model.

**Fix needed:** None.

---

### 17. `http_poller.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Fix needed:** None.

---

### 18. `lorawan.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `panic!` calls are inside `#[cfg(test)]`.

**Correctness:** MIC computation uses AES-128-CMAC. FPort handling is correct — FPort 0 means MAC commands in FRMPayload. FCnt rollover is noted but not handled (acceptable for a passive parser).

**Fix needed:** None.

---

### 19. `metar.rs` — Grade: **A**

**Production unwrap() calls:** 0

**Correctness:** Altimeter parsing handles both `A` (inches Hg × 100) and `Q` (hPa). Temperature/dewpoint parsing correctly handles `M` prefix for negative values. Wind parsing handles VRB, direction in degrees, gusts.

**Potential issue:** Visibility parsing for `M1/4SM` (less than 1/4 SM) correctly returns 0.25; `1/4SM` returns 0.25. However, `1 1/2SM` (mixed fraction) is not handled — would be parsed as just `1.0`. This is an edge case in real METAR reports.

**Fix needed:** None critical.

---

### 20. `mod.rs` — Grade: **A**

Module declaration file only. No parsing logic.

---

### 21. `modbus.rs` — Grade: **B**

**Production unwrap() calls:** 0

**Minor issue — register_values.len() * 2 overflow:**  
At line 759: `(register_values.len() * 2) as u8`. If more than 127 registers are provided, this truncates silently. Modbus spec limits to 125 holding registers per request (250 bytes), so `len * 2 <= 250` which fits in `u8`. The caller should validate count before calling.

**Fix needed:** Add assertion `debug_assert!(register_values.len() <= 125)`.

---

### 22. `mqtt.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Fix needed:** None.

---

### 23. `netflow.rs` — Grade: **B**

**Production unwrap() calls:** 0

**Minor issue — v5 count field not bounded:**  
At line 319: `let count = header.count as usize`. The NetFlow v5 header count field is a `u16` (max 30 records per packet per RFC, but not enforced in the header). A crafted packet with `count = 65535` will attempt to parse 65535 × 48 = 3,145,680 bytes of records. The `if offset + 48 > data.len() { break; }` guard at line 322 prevents a panic, but it will iterate the full 65535 times hitting the break condition each time — a minor DoS amplification if called in a tight loop.

**Fix needed:** `let count = header.count.min(30) as usize;` (enforce protocol maximum).

---

### 24. `nffi.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `panic!` calls are inside `#[cfg(test)]`.

**Correctness:** NFFI XML track parsing handles both position formats. The `loop {}` at line 210 is bounded by the XML element tree depth.

**Fix needed:** None.

---

### 25. `nmea.rs` — Grade: **C**

**Production unwrap() calls:** 1

```
nmea.rs:739  let mut sorted = self.parts.remove(&key).unwrap();
```

**Analysis of line 739:** This is inside the multi-part AIS reassembly in `AisAssembler::feed()`. The call to `.remove(&key)` is preceded by `if self.parts[&key].len() == expected as usize` — since `self.parts[&key]` only succeeds if the key exists, and `remove` of an existing key returns `Some(...)`, this `.unwrap()` is logically safe. However, it is fragile: a refactor removing the indexing check would introduce a panic.

**CRITICAL — AIS Sentinel Positions Not Filtered (Issue 4 from previous audit, still present):**

In `decode_ais()` (line 751) and the `to_source_event` mapping (line 922):

```rust
// decode_type_1_2_3 returns d.lat = 91.0 when lat unavailable
// decode_type_18_19 returns d.lat = 91.0 when lat unavailable
// No filter applied before publishing:
Some(NmeaData::AisPosition {
    lat: d.lat,   // could be 91.0 (sentinel = "not available")
    lon: d.lon,   // could be 181.0 (sentinel = "not available")
    ...
})
```

The AIS spec (ITU-R M.1371-5) defines:
- Longitude not available: `0x6791AC0` raw → `181.0°` after `/600_000`
- Latitude not available: `0x3412140` raw → `91.0°` after `/600_000`

These sentinel values are passed directly to `SourceEvent { latitude: Some(91.0), longitude: Some(181.0) }`. Consumers of this data who trust the `Some()` wrapper will process bogus coordinates as real positions. This is a **data quality defect** — in a maritime operations context, a ship appearing at lat=91° lon=181° is clearly wrong but systems trusting the event stream won't know to ignore it.

**Fix required:**

```rust
// In decode_ais(), after decoding:
fn ais_coords_valid(lat: f64, lon: f64) -> bool {
    lat.abs() <= 90.0 && lon.abs() <= 180.0
}

// In decode_type_1_2_3 / decode_type_18_19 result handling:
let (lat, lon) = if ais_coords_valid(d.lat, d.lon) {
    (d.lat, d.lon)
} else {
    return Some(NmeaData::AisPosition { lat: 0.0, lon: 0.0, ... }); 
    // OR: use Option<f64> for lat/lon fields
};
```

**Better fix (structural):** Change `AisPosition` lat/lon fields to `Option<f64>` and emit `None` for sentinel values.

**Fix for line 739:**
```rust
// Replace:
let mut sorted = self.parts.remove(&key).unwrap();
// With:
let Some(mut sorted) = self.parts.remove(&key) else { return None; };
```

---

### 26. `nmea2000.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Correctness:** NMEA 2000 PGN decoding uses the correct resolution factors per NMEA 2000 standard (e.g., heading in radians with 0.0001 rad resolution, speed in 0.01 m/s steps).

**Fix needed:** None.

---

### 27. `opcua.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Correctness:** OPC-UA NodeId parsing handles all four forms (numeric, string, GUID, opaque). StatusCode interpretation is correct per OPC-UA Part 4 Table B.1.

**Fix needed:** None.

---

### 28. `pcap.rs` — Grade: **C**

**Production unwrap() calls:** 10

```
pcap.rs:267  version_major: read_u16(data, 4, endian).unwrap(),
pcap.rs:268  version_minor: read_u16(data, 6, endian).unwrap(),
pcap.rs:269  thiszone: read_i32(data, 8, endian).unwrap(),
pcap.rs:270  sigfigs: read_u32(data, 12, endian).unwrap(),
pcap.rs:271  snaplen: read_u32(data, 16, endian).unwrap(),
pcap.rs:272  network: read_u32(data, 20, endian).unwrap(),
pcap.rs:287  let ts_sec = read_u32(data, offset, endian).unwrap();
pcap.rs:288  let ts_usec = read_u32(data, offset + 4, endian).unwrap();
pcap.rs:289  let incl_len = read_u32(data, offset + 8, endian).unwrap();
pcap.rs:290  let orig_len = read_u32(data, offset + 12, endian).unwrap();
```

**Analysis:** These 10 unwraps are **currently safe** due to pre-existing bounds checks:

- Lines 267–272 are inside `parse_pcap_global_header()`. The function returns early with `Err` if `data.len() < 24`. All reads are at offsets ≤ 20, each needing at most 4 bytes, so they always succeed within the verified 24-byte minimum. The `read_u16/read_u32/read_i32` functions return `None` only if bounds are violated — which cannot happen here.

- Lines 287–290 are inside `parse_pcap_packet_header()`. The function returns early with `Err` if `offset + 16 > data.len()`. All reads are at offsets within that 16-byte window.

**The unwraps cannot panic in the current code.** However, they are a maintenance hazard:
1. If the bounds check is ever removed or relaxed, the unwraps become panics.
2. Any code review tool will flag them as unsafe.
3. They set a bad precedent for future contributors.

**Fix required (style/hardening):**
```rust
// Replace unwrap() with ? by converting the functions to return Result:

fn read_u16(data: &[u8], offset: usize, endian: PcapEndian) -> Result<u16, ConnectorError> {
    if offset + 2 > data.len() {
        return Err(ConnectorError::ParseError("PCAP: read_u16 out of bounds".into()));
    }
    let bytes = [data[offset], data[offset + 1]];
    Ok(match endian {
        PcapEndian::Little => u16::from_le_bytes(bytes),
        PcapEndian::Big => u16::from_be_bytes(bytes),
    })
}

// Then use ? throughout:
version_major: read_u16(data, 4, endian)?,
```

---

### 29. `sparkplugb.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Correctness:** Sparkplug B topic format `spBv1.0/group/message_type/edge_node[/device]` is parsed correctly. Metric type mapping covers all Sparkplug B DataType enum values.

**Fix needed:** None.

---

### 30. `stix.rs` — Grade: **A**

**Production unwrap() calls:** 0

**Correctness:** STIX 2.1 bundle parsing handles all object types (indicator, malware, threat-actor, attack-pattern, tool, vulnerability). TAXII 2.1 discovery/collection parsing is correct.

**Fix needed:** None.

---

### 31. `syslog.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Correctness:** RFC 5424 structured data parsing handles multiple SD-IDs and SD-PARAMs. RFC 3164 priority extraction (facility × 8 + severity) is correct. BSD syslog hostname/tag parsing handles both formats.

**Fix needed:** None.

---

### 32. `websocket_client.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Fix needed:** None.

---

### 33. `zeek.rs` — Grade: **A**

**Production unwrap() calls:** 0  
All `.unwrap()` calls are in `#[cfg(test)]`.

**Correctness:** Zeek TSV log parser correctly handles `#separator`, `#fields`, `#types` headers. Unset value (`-`) is mapped to `None`. Timestamp parsing handles both `epoch.microseconds` float format and ISO 8601.

**Fix needed:** None.

---

## Consolidated Issues Requiring Fixes

### Priority 1 — Critical (Fix Before Any Production Deployment)

| File | Line(s) | Issue | Fix |
|------|---------|-------|-----|
| `ais.rs` | 29–42 | `parse_nmea_sentence()` always returns `None`; connector is non-functional | Implement real 6-bit payload decoding using the `ais_decoder` module already present in `nmea.rs` |
| `nmea.rs` | 760–769, 785–795 | AIS sentinel lat=91.0°/lon=181.0° published as real coordinates | Filter: `if lat.abs() > 90.0 \|\| lon.abs() > 180.0 { use None }` |

### Priority 2 — High (Fix Before Load Testing)

| File | Line(s) | Issue | Fix |
|------|---------|-------|-----|
| `database.rs` | 374, 428, 437 | `std::sync::Mutex::lock().unwrap()` panics on mutex poison; fragile in async runtime | Switch to `tokio::sync::Mutex` and propagate errors with `?` |
| `asterix.rs` | 185–188 | Extension byte loop `break`s on truncation instead of `Err` | Return `Err(ConnectorError::ParseError(...))` |

### Priority 3 — Medium (Fix in Next Refactor)

| File | Line(s) | Issue | Fix |
|------|---------|-------|-----|
| `pcap.rs` | 267–272, 287–290 | 10 `unwrap()` calls that are currently safe but fragile | Convert `read_u16/read_u32/read_i32` to return `Result` and use `?` |
| `nmea.rs` | 739 | `self.parts.remove(&key).unwrap()` — logically safe but fragile | Use `let Some(mut sorted) = self.parts.remove(&key) else { return None; };` |
| `netflow.rs` | 319 | `count` field uncapped; `count = 65535` causes 65535-iteration loop | Cap at `header.count.min(30)` per NetFlow v5 spec |
| `grib.rs` | 764, 803 | `total_length as usize` with no overflow guard on adversarial input | Add `if msg_len > data.len() - offset { offset += 1; continue; }` |
| `generic_api.rs` | 813 | `.expect()` in production code | Use `?` with a `map_err` |

---

## Fleet Summary

| File | Grade | Production unwrap() | Panic Risk | Correctness Issues |
|------|-------|--------------------|-----------|--------------------|
| acars.rs | A | 0 | None | None |
| adsb.rs | A | 0 | None | None |
| **ais.rs** | **F** | 0 | None | **Stub: always returns None** |
| **asterix.rs** | **C** | 0 | Low | **Extension truncation silent** |
| bacnet.rs | A | 0 | None | None |
| canbus.rs | A | 0 | None | None |
| cap.rs | A | 0 | None | None |
| cef.rs | A | 0 | None | None |
| cot.rs | A | 0 | None | None |
| csv_watcher.rs | A | 0 | None | None |
| **database.rs** | **D** | **3** | **Medium (mutex poison)** | Async mutex misuse |
| dnp3.rs | B | 0 | None | None |
| generic_api.rs | B | 0 (1 expect) | Negligible | None |
| geojson.rs | A | 0 | None | None |
| grib.rs | B | 0 | Low | Integer truncation on u64→usize |
| gtfs.rs | A | 0 | None | None |
| http_poller.rs | A | 0 | None | None |
| lorawan.rs | A | 0 | None | None |
| metar.rs | A | 0 | None | Minor: mixed fractions not parsed |
| mod.rs | A | — | — | — |
| modbus.rs | B | 0 | None | Minor: len*2 truncates at >127 regs |
| mqtt.rs | A | 0 | None | None |
| netflow.rs | B | 0 | None | count field uncapped |
| nffi.rs | A | 0 | None | None |
| **nmea.rs** | **C** | **1** | Low | **AIS sentinel positions published** |
| nmea2000.rs | A | 0 | None | None |
| opcua.rs | A | 0 | None | None |
| **pcap.rs** | **C** | **10** | None (safe by pre-check) | Style/fragility |
| sparkplugb.rs | A | 0 | None | None |
| stix.rs | A | 0 | None | None |
| syslog.rs | A | 0 | None | None |
| websocket_client.rs | A | 0 | None | None |
| zeek.rs | A | 0 | None | None |

**Total production unwrap() calls:** 14  
**Files with correctness defects:** 4 (ais, asterix, database, nmea)  
**Files grade D or F:** 2 (ais, database)

---

## Verification of Previous Audit Issues

| # | Issue | Previous Status | Current Status | Notes |
|---|-------|----------------|----------------|-------|
| 1 | `pcap.rs` — 10 unwrap() in production | Found | **Still present** | Technically safe due to pre-checks; no panic path exists today |
| 2 | `database.rs` — 3 mutex unwrap() in async | Found | **Still present** | Lines 374, 428, 437 confirmed |
| 3 | `ais.rs` — parse_nmea_sentence() stubbed | Found | **Still broken** | Returns `None` unconditionally |
| 4 | `nmea.rs` — AIS sentinels not filtered | Found | **Still broken** | lat/lon values 91°/181° pass through to SourceEvent |
| 5 | `asterix.rs` — extension loop silent truncation | Found | **Still broken** | `break` at line 188 confirmed |

---

*End of Audit Report*
