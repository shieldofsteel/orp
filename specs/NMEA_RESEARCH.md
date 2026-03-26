# NMEA Research for ORP Edge Connector

> **Author:** Sentinel (subagent research run)  
> **Date:** 2026-03-26  
> **Purpose:** Foundation research for the ORP NMEA connector — all the crates, specs, and cross-compilation details needed to build the edge binary.

---

## 1. NMEA 0183 Protocol Specification

### Overview

NMEA 0183 is the dominant serial marine data protocol. It runs over RS-422 (EIA-422) physically, though devices commonly output compatible RS-232. AIS devices use a variant at 38,400 baud.

### Physical Layer
| Parameter   | Value              |
|-------------|--------------------|
| Baud rate   | 4800 (standard), 38400 (NMEA-0183HS / AIS) |
| Data bits   | 8                  |
| Parity      | None               |
| Stop bits   | 1                  |
| Handshake   | None               |
| Transport   | RS-422 (optionally RS-232 or UDP/IP) |

### Sentence Structure

```
$TTSSS,data,data,...,data*HH<CR><LF>
!TTSSS,data,...*HH<CR><LF>   ← AIS encapsulated sentences use !
```

| Field       | Description |
|-------------|-------------|
| `$` or `!`  | Start delimiter. `$` = conventional, `!` = encapsulated (AIS) |
| `TT`        | Talker ID — 2 chars. e.g. `GP` (GPS), `GN` (GNSS), `II` (integrated), `AI` (AIS), `P` (proprietary) |
| `SSS`       | Sentence type — 3 chars. e.g. `GGA`, `RMC`, `VTG` |
| `,data`     | Comma-delimited fields. Empty field = no data |
| `*`         | Checksum delimiter (present if checksum included) |
| `HH`        | Two-hex-digit checksum |
| `<CR><LF>`  | Terminator (0x0D 0x0A) |
| Max length  | 82 characters total (including `$` and `<CR><LF>`) |

### Checksum Calculation

```rust
// XOR of all bytes BETWEEN $ (or !) and * (exclusive)
fn nmea_checksum(sentence: &str) -> u8 {
    sentence
        .bytes()
        .skip_while(|&b| b == b'$' || b == b'!')
        .take_while(|&b| b != b'*')
        .fold(0u8, |acc, b| acc ^ b)
}
// Result compared against the two hex chars after *
// e.g. *6A → checksum must equal 0x6A
```

**Important:** Checksum is optional for most sentences but **mandatory** for `RMA`, `RMB`, `RMC` and all AIS sentences. Always validate it.

### Reserved Characters
| Char | Hex  | Use |
|------|------|-----|
| `<CR>` | 0x0D | Carriage return |
| `<LF>` | 0x0A | Line feed / end |
| `!`  | 0x21 | AIS encapsulation start |
| `$`  | 0x24 | Sentence start |
| `*`  | 0x2A | Checksum delimiter |
| `,`  | 0x2C | Field delimiter |
| `\`  | 0x5C | TAG block delimiter |
| `^`  | 0x5E | HEX code delimiter |
| `~`  | 0x7E | Reserved |

### Common Talker IDs
| ID  | Source |
|-----|--------|
| `GP` | GPS only |
| `GN` | GNSS (multi-constellation) |
| `GL` | GLONASS |
| `GA` | Galileo |
| `GB` | BeiDou |
| `II` | Integrated instrumentation |
| `AI` | AIS |
| `HC` | Heading — magnetic compass |
| `WI` | Weather instruments |
| `P`  | Proprietary (followed by manufacturer ID) |

### Standard Sentence Types

#### Navigation / Position
| Sentence | Description |
|----------|-------------|
| `GGA`    | GPS Fix Data — lat/lon, altitude, fix quality, satellites, HDOP |
| `GLL`    | Geographic Position — lat/lon with timestamp |
| `RMC`    | Recommended Minimum Specific — lat/lon, speed, course, date/time |
| `VTG`    | Course and Speed Over Ground |
| `GNS`    | GNSS Fix Data (multi-constellation) |
| `GBS`    | Satellite Fault Detection |
| `GSA`    | DOP and Active Satellites |
| `GSV`    | Satellites in View (signal strength per satellite) |
| `GST`    | Position Error Statistics |
| `ZDA`    | Date and Time |

#### Depth / Environment
| Sentence | Description |
|----------|-------------|
| `DPT`    | Depth of Water |
| `DBK`    | Depth Below Keel |
| `DBT`    | Depth Below Transducer |
| `MTW`    | Mean Temperature of Water |
| `MDA`    | Meteorological Composite (wind, pressure, temp) |
| `MWV`    | Wind Speed and Angle |

#### Heading / Motion
| Sentence | Description |
|----------|-------------|
| `HDT`    | True Heading |
| `HDM`    | Magnetic Heading |
| `VHW`    | Water Speed and Heading |
| `ROT`    | Rate of Turn |

#### AIS (encapsulated with `!`)
| Sentence  | Description |
|-----------|-------------|
| `!AIVDM` | AIS VHF Data-link Message (from other vessels) |
| `!AIVDO` | AIS VHF Data-link Message Own-vessel |

#### Routing
| Sentence | Description |
|----------|-------------|
| `BOD`    | Bearing — Origin to Destination |
| `BWC`    | Bearing and Distance to Waypoint |
| `RMB`    | Recommended Minimum Navigation Information |
| `AAM`    | Waypoint Arrival Alarm |

#### Sample Sentence (GGA)
```
$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76
         ↑time    ↑lat       ↑lon        ↑q↑sv↑hdop ↑alt         ↑checksum
```

---

## 2. Rust Crates for NMEA Parsing

### Option A: `nmea` (Recommended for GNSS)

- **Version:** `0.7.0`
- **Link:** https://crates.io/crates/nmea / https://docs.rs/nmea
- **License:** Apache-2.0 ✅ (compatible with Apache 2.0)
- **Stable Rust:** ✅ MSRV 1.70.0
- **Async/Tokio:** ❌ Synchronous only — parse individual sentences. Use with tokio's `spawn_blocking` or a reader loop.
- **no_std:** ✅ (feature flag: disable `std` feature)
- **Sentences supported:**
  - AAM, ALM, APA, BOD, BWC, BWW, DBK, DPT, GBS, **GGA**, **GLL**, **GNS**, **GSA**, GST, **GSV**, HDT, MDA, MTW, MWV, **RMC**, TTM, VHW, **VTG**, WNC, ZDA, ZFO, ZTG, TXT
  - Vendor: PGRMZ (Garmin altitude)
- **Usage:**
  ```rust
  use nmea::Nmea;
  let mut parser = Nmea::default();
  parser.parse("$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76").unwrap();
  println!("{:?}", parser.latitude); // Some(53.36...)

  // Or stateless:
  use nmea::parse_str;
  let result = parse_str("$GPGGA,...").unwrap();
  ```
- **Gotchas:**
  - Does NOT support AIS sentences — use `ais` crate for those
  - Stateful `Nmea` struct accumulates fixes; stateless `parse_str` for one-shot use
  - serde support behind `serde` feature flag

---

### Option B: `nmea-parser` (Recommended for AIS + GNSS combined)

- **Version:** `0.11.0`
- **Link:** https://crates.io/crates/nmea-parser
- **License:** Apache-2.0 ✅
- **Stable Rust:** ✅ (2018 edition)
- **Async/Tokio:** ❌ Synchronous parser
- **no_std:** ✅ (feature flag)
- **Sentences supported:** Both AIS (class A + B) and GNSS sentences in one crate
- **Usage:**
  ```rust
  use nmea_parser::*;
  let mut parser = NmeaParser::new();
  match parser.parse_sentence("!AIVDM,1,1,,B,15M67N0P01G?Uf6E...")? {
      ParsedMessage::VesselDynamicData(vdd) => { /* AIS position */ }
      ParsedMessage::Gga(gga) => { /* GPS fix */ }
      _ => {}
  }
  ```
- **Gotchas:**
  - Last updated ~2023. Active but slower release cadence.
  - Handles multi-fragment AIS assembly internally (critical for type 8+ AIS msgs)
  - Good balance of AIS + GNSS if you need both in one place

---

### Option C: `nmea0183` (Byte-by-byte streaming)

- **Version:** ~`0.2.x`
- **Link:** https://crates.io/crates/nmea0183
- **License:** MIT ✅
- **Stable Rust:** ✅
- **Async/Tokio:** ❌ Push bytes one at a time, sentence detected on `\n`
- **Usage:**
  ```rust
  use nmea0183::{Parser, ParseResult};
  let mut parser = Parser::new();
  for byte in data.iter() {
      if let Some(result) = parser.parse_from_byte(*byte) {
          match result {
              Ok(ParseResult::GGA(Some(gga))) => { /* got fix */ }
              _ => {}
          }
      }
  }
  ```
- **Gotchas:**
  - Fewer sentence types than `nmea` crate
  - Useful for embedded/streaming byte sources; less useful for batch parsing

---

### Option D: `nmea0183-parser` (nom-based, newest)

- **Version:** `0.3.2`
- **Link:** https://crates.io/crates/nmea0183-parser
- **License:** MIT OR Apache-2.0 ✅
- **Stable Rust:** ✅ (2024 edition, 7 months old)
- **Async/Tokio:** ❌
- **Notes:** Uses `nom` parser combinators — clean, well-structured. Newer project, fewer sentence types currently.

---

### **Recommendation for ORP**

Use **`nmea` + `ais`** as separate crates:
- `nmea` for all GNSS/navigation sentences
- `ais` for AIS AIVDM decoding
- They're both Apache-2.0 and well-maintained
- OR use `nmea-parser` alone if you want one crate handling both

---

## 3. Serial Port Crates

### Option A: `serialport` (Blocking baseline)

- **Version:** `4.8.1`
- **Link:** https://crates.io/crates/serialport / https://docs.rs/serialport
- **License:** MPL-2.0 ⚠️ — **Note:** MPL-2.0 is file-level copyleft. You can link against it in an Apache-2.0 binary (it's compatible as a dependency), but modifications to serialport files themselves must be MPL-2.0. For ORP's use as a library dependency, this is **fine**.
- **Stable Rust:** ✅
- **Async/Tokio:** ❌ Blocking I/O only
- **Cross-platform:** ✅ Linux, macOS, Windows
- **Usage:**
  ```rust
  use serialport::SerialPort;
  let port = serialport::new("/dev/ttyUSB0", 4800)
      .timeout(std::time::Duration::from_millis(10))
      .open()?;
  let mut reader = std::io::BufReader::new(port);
  let mut line = String::new();
  reader.read_line(&mut line)?;
  ```
- **Gotchas:**
  - Blocking — wrap in `tokio::task::spawn_blocking` for async apps
  - Foundation for `tokio-serial` and `mio-serial`

---

### Option B: `tokio-serial` (Async — Recommended)

- **Version:** `5.4.5`
- **Link:** https://crates.io/crates/tokio-serial / https://docs.rs/tokio-serial
- **License:** MIT ✅
- **Stable Rust:** ✅ MSRV 1.46.0
- **Async/Tokio:** ✅ Full async, implements `AsyncRead` + `AsyncWrite`
- **Usage:**
  ```rust
  use tokio_serial::SerialPortBuilderExt;
  use tokio::io::AsyncReadExt;

  #[tokio::main]
  async fn main() {
      let mut port = tokio_serial::new("/dev/ttyUSB0", 4800)
          .open_native_async()
          .unwrap();
      let mut buf = [0u8; 1024];
      let n = port.read(&mut buf).await.unwrap();
      // Parse NMEA lines from buf[..n]
  }
  ```
- **Gotchas:**
  - Known issue: `readable().await` can return without data on some platforms (busy-loop risk). Use `AsyncBufReadExt::read_line` with a `BufReader` wrapper instead.
  - Wraps `serialport` under the hood (so same MPL-2.0 concern applies transitively via serialport)
  - Best pattern for ORP: `tokio::io::BufReader::new(port)` → `lines()` stream

---

### Option C: `serial2-tokio` (Newer alternative)

- **Version:** current (active as of 2026-02)
- **Link:** https://lib.rs/crates/serial2-tokio
- **License:** BSD-2-Clause ✅
- **Async/Tokio:** ✅ Implements `AsyncRead`/`AsyncWrite` with `&self` (allows concurrent read+write)
- **Gotchas:**
  - Less battle-tested than tokio-serial
  - The `&self` concurrent read/write is a useful advantage for bidirectional comms

---

### **Recommendation for ORP**

```toml
[dependencies]
tokio-serial = "5.4.5"
tokio = { version = "1", features = ["full"] }
```

Use pattern:
```rust
let port = tokio_serial::new("/dev/ttyUSB0", 4800).open_native_async()?;
let reader = tokio::io::BufReader::new(port);
let mut lines = reader.lines();
while let Some(line) = lines.next_line().await? {
    // parse NMEA line
}
```

---

## 4. NMEA 2000 / CAN Bus Crates

### `socketcan`

- **Version:** `3.5.x` (v3.4.0 released end of 2024)
- **Link:** https://crates.io/crates/socketcan / https://github.com/socketcan-rs/socketcan-rs
- **License:** MIT ✅
- **Stable Rust:** ✅ MSRV 1.70 (Rust 2021 edition)
- **Async/Tokio:** ✅ Optional feature `tokio` (also supports `async-std`, `smol`)
- **Platform:** Linux only (uses Linux SocketCAN kernel subsystem)
- **Features:**
  - Standard CAN frames + CAN FD (Flexible Data)
  - Netlink interface control (set bitrate, restart interface)
  - Async support merged from `tokio-socketcan`
  - Frame filtering, error frames
  - `candump` log format parsing
- **Usage:**
  ```rust
  // Async tokio example
  use socketcan::{tokio::CanSocket, Frame, CanFrame};
  let socket = CanSocket::open("can0").unwrap();
  let frame = socket.read_frame().await.unwrap();
  println!("CAN ID: {:?}, data: {:?}", frame.raw_id(), frame.data());
  ```
- **Gotchas:**
  - Linux **only** — no macOS/Windows support (Raspberry Pi is fine)
  - NMEA 2000 decodes from CAN frames require additional decoding of PGN (Parameter Group Number) format
  - Recommend pairing with `canboat-rs` for NMEA 2000 PGN decoding

---

### `canboat-rs`

- **Version:** Latest on crates.io
- **Link:** https://crates.io/crates/canboat-rs
- **License:** Check repo (based on canboat — Apache-2.0 likely)
- **What it does:** Reads NMEA 2000 data using PGN definitions auto-generated from the canboat project database
- **Gotchas:**
  - Relatively niche crate — evaluate stability before relying on it
  - Alternative: implement PGN decoding manually using the open canboat PGN database (JSON)

### NMEA 2000 Architecture Note

NMEA 2000 runs on a CAN bus at 250 kbit/s. The data model:
- **PGN** (Parameter Group Number) — identifies message type (like sentence type in 0183)
- **Source/Destination** — 8-bit address
- Fast-packet protocol for multi-frame messages > 8 bytes

For ORP on Raspberry Pi, options:
1. **Actisense NGT-1** — USB-to-NMEA2000 adapter that outputs NMEA 0183 over serial → use `tokio-serial` + convert
2. **PiCAN 2** — HAT for Raspberry Pi that provides a native CAN interface → use `socketcan`
3. **YDNU-02** — Yacht Devices USB adapter → serial-based, similar to NGT-1

---

## 5. AIS Decoding

### `ais` (Primary Recommendation)

- **Version:** `0.18.x` (active as of 2024)
- **Link:** https://crates.io/crates/ais / https://docs.rs/ais
- **License:** MIT ✅ (compatible with Apache 2.0)
- **Stable Rust:** ✅
- **Async/Tokio:** ❌ Pure parser — pair with tokio-serial for async ingestion
- **Supports:** `!AIVDM` and `!AIVDO` sentences, handles multi-fragment assembly
- **AIS Message Types Supported:**
  - Type 1/2/3 — Position Report Class A
  - Type 4 — Base Station Report
  - Type 5 — Static and Voyage Related Data
  - Type 8 — Binary Broadcast Message
  - Type 9 — Standard SAR Aircraft Position Report
  - Type 14 — Safety Related Broadcast Message
  - Type 18 — Standard Class B CS Position Report
  - Type 21 — Aid-to-Navigation Report
  - Type 24 — Class B CS Static Data Report
  - And more...
- **Usage:**
  ```rust
  use ais::{AisFragments, AisParser};
  use ais::messages::AisMessage;

  let line = b"!AIVDM,1,1,,B,E>kb9O9aS@7PUh10dh19@;0Tah2cWrfP:l?M`00003vP100,0*01";
  let mut parser = AisParser::new();
  if let AisFragments::Complete(msg) = parser.parse(line, true)? {
      match msg.message {
          AisMessage::PositionReport(pos) => {
              println!("MMSI: {}, lat: {:?}, lon: {:?}", pos.mmsi, pos.latitude, pos.longitude);
          }
          _ => {}
      }
  }
  ```
- **Gotchas:**
  - Multi-fragment AIS messages (split over multiple sentences) require holding fragments — the parser handles this internally but requires feeding lines sequentially
  - AIS encoding uses 6-bit ASCII armoring — the crate handles de-armoring automatically

---

### `ship162` (Alternative — newer, complete)

- **Version:** `0.1.0`
- **Link:** https://crates.io/crates/ship162
- **License:** Check repo
- **What it does:** All 27 AIS message types, uses `deku` for bit-level parsing
- **Gotchas:** Very new (0.1.0), limited production use. Interesting for future.

---

## 6. Raspberry Pi Cross-Compilation (from macOS Apple Silicon)

### Target Triples

| Pi Model | OS          | Target Triple |
|----------|-------------|---------------|
| Pi 3/4/5 (64-bit OS) | Debian/Ubuntu arm64 | `aarch64-unknown-linux-gnu` |
| Pi 3/4 (32-bit OS) | Raspberry Pi OS | `armv7-unknown-linux-gnueabihf` |
| Pi Zero / Pi 1 | Any | `arm-unknown-linux-gnueabihf` |

**For modern Pi (3/4/5) running 64-bit Raspberry Pi OS: use `aarch64-unknown-linux-gnu`**

---

### Method 1: Direct Cross-Compile (No Docker) — macOS → aarch64 Linux

```bash
# 1. Add target
rustup target add aarch64-unknown-linux-gnu

# 2. Install cross-linker via Homebrew
brew tap messense/macos-cross-toolchains
brew install aarch64-unknown-linux-gnu

# 3. Configure Cargo linker
# .cargo/config.toml in ORP project:
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-unknown-linux-gnu-gcc"

# 4. Build
cargo build --target aarch64-unknown-linux-gnu --release

# Output: target/aarch64-unknown-linux-gnu/release/orp-edge
```

**Gotcha:** If any dependency needs system libs (openssl, libudev for serialport), cross-linking becomes complex. Prefer:
- `rustls` over OpenSSL
- `serialport` with `libudev` disabled (Linux udev feature)

---

### Method 2: `cross` (Docker-based — Easiest)

```bash
# Install cross
cargo install cross

# Build (Docker must be running)
cross build --target aarch64-unknown-linux-gnu --release
```

`cross` uses a Docker image pre-configured with the correct cross-linker and sysroot. Handles system library cross-compilation automatically.

**Gotcha:** `cross` historically had issues with aarch64 (Apple Silicon) **hosts**. As of 2024, this is largely resolved if Docker Desktop is used with `linux/amd64` emulation. Use:
```bash
cross build --target aarch64-unknown-linux-gnu --release
```

If `cross` fails on M-series Mac:
```bash
# Override Docker platform
CROSS_CONTAINER_OPTS="--platform linux/amd64" cross build --target aarch64-unknown-linux-gnu --release
```

---

### Method 3: Static musl Build (Most Portable)

For a fully static binary with no system lib dependencies:

```bash
rustup target add aarch64-unknown-linux-musl

# Install musl cross toolchain
brew install FiloSottile/musl-cross/musl-cross
# or: cargo install cross and use musl target

# .cargo/config.toml
[target.aarch64-unknown-linux-musl]
linker = "aarch64-linux-musl-gcc"

cargo build --target aarch64-unknown-linux-musl --release
```

**Advantage:** Binary runs on any aarch64 Linux regardless of glibc version — great for OTA deployment to field devices.

**Gotcha:** `serialport` depends on `libudev` on Linux — this breaks musl static builds. Solution:
```toml
[dependencies]
serialport = { version = "4", default-features = false }
# or use tokio-serial with udev disabled
```

---

### Recommended CI/CD Pipeline for ORP

```yaml
# GitHub Actions
jobs:
  build-arm:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-unknown-linux-gnu
      - uses: taiki-e/setup-cross-toolchain-action@v1
        with:
          target: aarch64-unknown-linux-gnu
      - run: cargo build --target aarch64-unknown-linux-gnu --release
      - uses: actions/upload-artifact@v4
        with:
          name: orp-edge-aarch64
          path: target/aarch64-unknown-linux-gnu/release/orp-edge
```

---

## 7. Recommended Cargo.toml for ORP NMEA Connector

```toml
[package]
name = "orp-edge"
version = "0.1.0"
edition = "2021"

[dependencies]
# NMEA GNSS parsing
nmea = { version = "0.7", features = ["serde"] }

# AIS decoding (!AIVDM)
ais = "0.18"

# Async serial port reading
tokio-serial = "5.4"
tokio = { version = "1", features = ["full"] }

# CAN bus (NMEA 2000) — Linux only, optional
[target.'cfg(target_os = "linux")'.dependencies]
socketcan = { version = "3", features = ["tokio"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# Error handling
thiserror = "1"
anyhow = "1"

[profile.release]
opt-level = "z"     # Optimize for size (edge device)
lto = true
strip = true
```

---

## 8. Key Architecture Decisions for ORP Edge Connector

### Input Sources
```
NMEA 0183 over RS-422/RS-232/USB → tokio-serial → nmea crate → normalized events
NMEA 0183 over UDP (port 10110 is de facto standard) → tokio UDP socket → same parser
AIS !AIVDM sentences → ais crate → vessel events
NMEA 2000 via CAN bus (Raspberry Pi + PiCAN HAT) → socketcan → PGN decode
NMEA 2000 via Actisense NGT-1 (USB serial) → tokio-serial → proprietary frame decode
```

### NMEA Multiplexer / Combiner
Multiple instruments speak simultaneously on separate talkers. Common setup:
- GPS talker: `$GPGGA`, `$GPRMC` at 1Hz
- AIS transponder: `!AIVDM` at irregular intervals, 38400 baud
- Wind/depth: `$IIMWV`, `$IIDPT` at 1-2Hz
- Compass: `$HCHDT` at 10Hz

**Solution:** Open a separate tokio task per serial port, merge via mpsc channel.

### Baud Rate Selection
- GPS: 4800 baud (standard), some modern units default to 9600 or 115200
- AIS: 38400 baud (NMEA-0183HS)
- All others: 4800 baud

---

## 9. Quick Reference Links

| Resource | URL |
|----------|-----|
| `nmea` crate docs | https://docs.rs/nmea |
| `nmea-parser` crate | https://crates.io/crates/nmea-parser |
| `ais` crate docs | https://docs.rs/ais |
| `tokio-serial` crate | https://crates.io/crates/tokio-serial |
| `serialport` crate | https://crates.io/crates/serialport |
| `socketcan` crate | https://crates.io/crates/socketcan |
| `socketcan` GitHub | https://github.com/socketcan-rs/socketcan-rs |
| `canboat-rs` | https://crates.io/crates/canboat-rs |
| canboat PGN database | https://github.com/canboat/canboat |
| cross-rs | https://github.com/cross-rs/cross |
| NMEA 0183 Wikipedia | https://en.wikipedia.org/wiki/NMEA_0183 |
| NMEA 2000 Wikipedia | https://en.wikipedia.org/wiki/NMEA_2000 |
| macOS → Pi cross-compile guide | https://sebi.io/posts/2024-05-02-guide-cross-compiling-rust-from-macos-to-raspberry-pi-2024-apple-silicon/ |

---

*Research complete. This document covers everything needed to implement ORP's NMEA edge connector.*
