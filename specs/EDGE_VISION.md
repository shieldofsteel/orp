# ORP Edge — The Vision

## The Problem
A ship has 10+ NMEA devices on its bridge. Each generates data continuously. That data stays siloed in individual displays. Nobody fuses it. Nobody shares it.

## The Solution
A 43MB binary on a $50 Raspberry Pi plugged into the NMEA bus:

```
Ship NMEA Bus (serial/TCP)
    │
    ▼
ORP Edge (Raspberry Pi, --headless)
    │
    ├── Parses ALL NMEA 0183 sentences
    ├── Parses NMEA 2000 (CAN bus via gateway)
    ├── Fuses into entities (own ship, nearby ships, weather, depth)
    ├── Stores locally (works offline)
    ├── Serves API on local network (crew can view on any browser)
    │
    └── When internet available:
        ├── Syncs with peer ORPs (other vessels, shore stations)
        ├── Shares position + sensor data (opt-in, ABAC-controlled)
        └── Receives global AIS, weather, threat data from peers
```

## NMEA 0183 Sentences to Parse

### Navigation
- `$GPGGA` — GPS fix (lat, lon, altitude, satellites, HDOP)
- `$GPRMC` — Recommended minimum (lat, lon, speed, course, date)
- `$GPVTG` — Track/speed over ground
- `$GPGSA` — GPS DOP and active satellites
- `$GPGSV` — Satellites in view

### AIS
- `!AIVDM` — AIS VHF data link message (other ships)
- `!AIVDO` — Own ship AIS data
- Message types 1-3 (position), 5 (static data), 18-19 (Class B), 21 (aids to navigation)

### Instruments
- `$WIMWD` — Wind direction and speed (true)
- `$WIMWV` — Wind speed and angle (relative)
- `$SDDBT` — Depth below transducer
- `$SDDBS` — Depth below surface
- `$YXXDR` — Transducer measurements (temperature, pressure, humidity)
- `$HCHDG` — Heading (magnetic)
- `$HEROT` — Rate of turn

### Engine (via gateway)
- `$ERRPM` — Engine RPM
- `$ERXDR` — Engine parameters (fuel flow, oil pressure, coolant temp)

## NMEA 2000 (CAN Bus)
NMEA 2000 uses PGNs (Parameter Group Numbers). Common ones:
- PGN 127250 — Vessel heading
- PGN 128259 — Speed through water
- PGN 128267 — Water depth
- PGN 130306 — Wind data
- PGN 127488 — Engine parameters
- PGN 129025 — Position (lat/lon)
- PGN 129029 — GNSS position data
- PGN 129038/039 — AIS Class A/B position

## Entity Types from NMEA

| NMEA Source | ORP Entity Type | Key Properties |
|------------|-----------------|----------------|
| GPS ($GPGGA/RMC) | `own_vessel` | lat, lon, speed, course, satellites |
| AIS (!AIVDM) | `ship` | mmsi, name, lat, lon, speed, course, type |
| Depth ($SDDBT) | `depth_reading` | depth_m, transducer_offset |
| Wind ($WIMWD) | `wind_reading` | direction_true, speed_knots |
| Heading ($HCHDG) | `own_vessel` (merged) | heading_magnetic, deviation |
| Engine ($ERRPM) | `engine` | rpm, engine_number |
| Temperature ($YXXDR) | `sensor` | temperature_c, humidity, pressure |

## Architecture: Edge → Mesh → COP

```
Layer 1: EDGE (on-vessel/on-site)
├── Raspberry Pi / embedded Linux
├── ORP binary (43MB, --headless)
├── Reads NMEA via serial:///dev/ttyUSB0 or tcp://192.168.1.100:10110
├── No internet required
├── Local web UI on ship's LAN (optional)
└── Stores 30 days of data locally

Layer 2: MESH (vessel-to-vessel, vessel-to-shore)
├── When internet/radio available
├── ORP instances discover each other (mDNS on LAN, configured for WAN)
├── Peer sync every 30s
├── ABAC controls what data is shared
├── Conflict resolution: highest confidence wins
└── Bandwidth-efficient: only send deltas, not full state

Layer 3: COP (Command Center)
├── Shore-based ORP with full web UI
├── Receives from all Edge nodes
├── Fuses into single operational picture
├── Analytics: CPA, anomaly detection, threat scoring
├── Multi-user with RBAC
└── Historical replay, reporting, export
```

## Installation

```bash
# On a Raspberry Pi:
curl -fsSL https://orp.dev/install | sh
orp connect serial:///dev/ttyUSB0 --baud 38400
orp start --headless --port 9090

# On a shore command center:
orp peer add vessel-alpha.local:9090
orp peer add vessel-bravo.local:9090
orp start --port 9090
# Open browser → all vessel data fused on one map
```

## The Moat
This is the moat. Not the web UI. Not the query language. 

The moat is: **a $50 device that plugs into ANY vessel's NMEA bus and instantly joins a global data fusion network.**

Every device deployed makes the network more valuable. Every vessel connected adds data. The more ORPs in the network, the more complete the operational picture. This is the network effect that no competitor can replicate by cloning code.
