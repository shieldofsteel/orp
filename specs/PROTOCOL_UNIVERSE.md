# PROTOCOL UNIVERSE
### ORP Multi-Domain Sensor & Data Protocol Reference

> **Purpose:** Defines every major protocol ORP must understand to be truly universal.
> Each entry specifies domain, format, Rust support, implementation difficulty, and strategic importance.
>
> **Last updated:** 2026-03-26
> **Researcher:** Sentinel (subagent)

---

## Table of Contents

1. [Maritime](#1-maritime)
2. [Aviation](#2-aviation)
3. [Military / Defense](#3-military--defense)
4. [IoT / Industrial](#4-iot--industrial)
5. [Cyber / Network](#5-cyber--network)
6. [Automotive / Transport](#6-automotive--transport)
7. [Weather / Environment](#7-weather--environment)
8. [Emergency / Public Safety](#8-emergency--public-safety)
9. [Implementation Roadmap](#implementation-roadmap)
10. [Rust Crate Registry](#rust-crate-registry)

---

## 1. Maritime

### NMEA 0183
| Field | Value |
|-------|-------|
| **Domain** | Maritime — GPS receivers, chart plotters, AIS transponders, depth sounders |
| **Format** | ASCII text, comma-delimited sentences starting with `$` or `!` |
| **Standard** | IEC 61162-1 / NMEA 0183 v4.11 |
| **Rust Crates** | `nmea` (simple GNSS), `nmea-parser` (GNSS + AIS), `nmea0183-parser` (nom-based) |
| **Difficulty** | Easy — well-documented, text-based, checksum-validated |
| **ORP Importance** | **CRITICAL** — primary marine sensor protocol worldwide |

**Key sentence types:** GGA (GPS fix), RMC (recommended minimum), VTG (track/speed), GLL, ZDA (time), VHW (water speed), DBT (depth), HDG (heading), MWV (wind), VDM/VDO (AIS encapsulation)

**Notes:** Sentences are max 82 chars. Fields separated by commas. CRLF terminated. Checksum = XOR of all chars between `$` and `*`.

---

### AIS (Automatic Identification System)
| Field | Value |
|-------|-------|
| **Domain** | Maritime — vessel tracking, collision avoidance, port management |
| **Standard** | ITU-R M.1371-5 (radio), IEC 62287 (receiver), SOLAS Chapter V |
| **Format** | Binary payload encoded as 6-bit ASCII, wrapped in NMEA VDM/VDO sentences |
| **Rust Crates** | `nmea-parser` (AIS + GNSS), `ais` crate, `rs162` / `ship162` (full AIS + MQTT) |
| **Difficulty** | Medium — bit-level parsing, 27+ message types, multi-sentence assembly |
| **ORP Importance** | **CRITICAL** — required for all maritime situational awareness |

**Key message types:**
- Type 1/2/3: Class A position report (CNB)
- Type 4: Base station report
- Type 5: Class A voyage data
- Type 14: Safety broadcast
- Type 18/19: Class B position
- Type 21: Aid to navigation
- Type 24: Class B static data

**Transport:** VHF 161.975 MHz (Ch 87B) and 162.025 MHz (Ch 88B) at 9600 bps GMSK

---

### NMEA 2000 (N2K)
| Field | Value |
|-------|-------|
| **Domain** | Maritime — modern vessel networks (engine, navigation, bilge, instruments) |
| **Standard** | IEC 61162-3, based on CAN bus (ISO 11898) |
| **Format** | Binary CAN frames, PGN (Parameter Group Number) based, 250 kbps |
| **Rust Crates** | No dedicated crate; use `socketcan` + manual PGN decoding; `canparse` for generic CAN |
| **Difficulty** | Hard — proprietary PGN definitions, closed standard (license required), CAN framing |
| **ORP Importance** | **Critical** — dominant onboard vessel bus for modern ships |

**Key PGNs:** 129025 (position rapid), 129026 (COG/SOG rapid), 129029 (GNSS fix), 127245 (rudder), 127488 (engine), 128267 (water depth)

**Notes:** Canboat project (C) has most complete open PGN database. OpenCPN uses it. Consider wrapping canboat via FFI or subprocess.

---

### IEC 61162 / IEC 61162-450
| Field | Value |
|-------|-------|
| **Domain** | Maritime — bridge systems, ECDIS, RADAR integration |
| **Standard** | IEC 61162-1 (serial/NMEA 0183 successor), IEC 61162-450 (Ethernet/UDP multicast) |
| **Format** | IEC 61162-450: UDP/XML wrapping NMEA 0183 sentences; adds source tagging |
| **Rust Crates** | No dedicated crate; parse as NMEA 0183 + UDP socket handling |
| **Difficulty** | Medium — extends NMEA 0183 over UDP with XML envelope |
| **ORP Importance** | **Useful** — required for modern integrated bridge systems (IBS) |

---

### ECDIS (Electronic Chart Display)
| Field | Value |
|-------|-------|
| **Domain** | Maritime — electronic navigation charts |
| **Standard** | IHO S-57 (data), IHO S-100 (framework), IHO S-52 (display) |
| **Format** | S-57: ISO 8211 binary (feature objects, spatial objects); S-100: GML/XML + HDF5 |
| **Rust Crates** | No Rust crates; Python: `pyshp`, `gdal`; consider FFI to GDAL |
| **Difficulty** | Hard — complex object model, ISO 8211 binary, proprietary chart cells |
| **ORP Importance** | **Useful** — chart overlay for situational awareness, not real-time sensor |

---

### ARPA Radar
| Field | Value |
|-------|-------|
| **Domain** | Maritime — collision avoidance radar tracking |
| **Standard** | Output via NMEA 0183 TTM sentences (Tracked Target Message) |
| **Format** | ASCII NMEA sentences: TTM, TLL, RSD, OSD |
| **Rust Crates** | Handled by NMEA 0183 parsers (TTM sentence support varies) |
| **Difficulty** | Easy — subset of NMEA 0183 |
| **ORP Importance** | **Useful** — maritime traffic / CPA/TCPA calculations |

---

## 2. Aviation

### ADS-B / Mode S
| Field | Value |
|-------|-------|
| **Domain** | Aviation — aircraft surveillance, traffic awareness (TCAS), FlightAware |
| **Standard** | ICAO Annex 10, DO-260B (ADS-B), DO-181F (Mode S) |
| **Format** | Binary — 56-bit (short) or 112-bit (long) frames at 1090 MHz |
| **Rust Crates** | `adsb` (Mode S DF parsing), `adsb_deku` (deku-based, comprehensive) |
| **Difficulty** | Medium — bit-level, CPR position decoding (complex math), multiple message types |
| **ORP Importance** | **CRITICAL** — primary aviation surveillance worldwide |

**Key message types (DF):**
- DF 17: Extended Squitter (ADS-B) — position, velocity, ID
- DF 18: Non-transponder ADS-B
- DF 11: All-Call reply
- DF 20/21: Surveillance altitude/identity

**Key Extended Squitter type codes:**
- TC 1-4: Aircraft ID
- TC 9-18: Airborne position (CPR encoded lat/lon)
- TC 19: Velocity (ground speed or airspeed)

**Receiving:** RTL-SDR dongle + `dump1090` → Beast format or raw frames. ORP can ingest Beast TCP or raw frames.

---

### ASTERIX (All Purpose Structured Eurocontrol Surveillance Information Exchange)
| Field | Value |
|-------|-------|
| **Domain** | Aviation — ATC radar data exchange between systems |
| **Standard** | Eurocontrol ASTERIX (open, free documentation) |
| **Format** | Binary — category-based TLV structure (Category 1, 10, 21, 48, 62, etc.) |
| **Rust Crates** | `asterix` (deku-based encode/decode), `asterix_parser` |
| **Difficulty** | Medium-Hard — many categories, FSPEC bitmask field presence, UAP tables |
| **ORP Importance** | **Critical** — European ATC standard, used in MLAT, SMGCS |

**Key categories:**
- Cat 001: Monoradar (SSR/PSR) plots & tracks
- Cat 010: MLAT/WAM target reports
- Cat 021: ADS-B target reports
- Cat 048: Monoradar target reports (most common)
- Cat 062: SDPS (System Track) data

---

### ACARS (Aircraft Communications Addressing and Reporting System)
| Field | Value |
|-------|-------|
| **Domain** | Aviation — airline operational messaging (VHF/SATCOM) |
| **Standard** | ARINC 618, 619, 620, 622, 633 |
| **Format** | ASCII text with structured header (mode, reg, flight, label, sublabel) |
| **Rust Crates** | No Rust crate; `libacars` (C library) — wrap via FFI |
| **Difficulty** | Medium — header parsing easy; payload (FANS, ATC, AOC) is application-specific |
| **ORP Importance** | **Useful** — flight operations data, weather uplinks, position reports |

**Receiving:** VHF 129.125 MHz (primary) + SATCOM (Inmarsat/Iridium). SDR + `acarsdec`.

---

### ARINC 429
| Field | Value |
|-------|-------|
| **Domain** | Aviation — avionics bus (FMS, ADC, IRS, autopilot, displays) |
| **Standard** | ARINC 429 (Aeronautical Radio Inc.) |
| **Format** | Binary — 32-bit words on differential twisted pair, 12.5/100 kbps |
| **Rust Crates** | No Rust crates; hardware interface required (ARINC 429 cards) |
| **Difficulty** | Hard — proprietary hardware interface, equipment labels, BNR/BCD encoding |
| **ORP Importance** | **Niche** — avionics only, not externally accessible without special hardware |

---

### MIL-STD-1553
| Field | Value |
|-------|-------|
| **Domain** | Military Aviation / Defense — avionics/weapons systems data bus |
| **Standard** | MIL-STD-1553B |
| **Format** | Binary — 1 MHz Manchester II encoded, command/response protocol |
| **Rust Crates** | No Rust crates; requires specialized hardware interface cards |
| **Difficulty** | Hard — military hardware, requires licensed interface hardware |
| **ORP Importance** | **Niche** — military avionics only, very restricted access |

---

### SWIM (System Wide Information Management)
| Field | Value |
|-------|-------|
| **Domain** | Aviation — FAA/EUROCONTROL operational data sharing (flights, restrictions, weather) |
| **Standard** | SWIM (ICAO Doc 10039), FIXM (flight info), AIXM (airspace), WXXM/IWXXM (weather) |
| **Format** | XML/GML over AMQP or JMS messaging |
| **Rust Crates** | No dedicated crates; `quick-xml` for parsing, `lapin` for AMQP |
| **Difficulty** | Hard — large XML schemas, subscription-based, requires FAA/EUROCONTROL credentials |
| **ORP Importance** | **Useful** — strategic flight data, not real-time sensor |

---

## 3. Military / Defense

### Link 16 / TADIL-J
| Field | Value |
|-------|-------|
| **Domain** | Military — NATO tactical data link (air, land, sea platforms) |
| **Standard** | MIL-STD-6016, STANAG 5516 |
| **Format** | Binary — TDMA time-slotted, J-series messages (J2.0 tracks, J3.0 Air tracks, etc.) |
| **Rust Crates** | None — classified implementation, NSA Type 1 encryption |
| **Difficulty** | **Hard/Impossible** — classified, requires COMSEC equipment (KIV-77 crypto) |
| **ORP Importance** | **Niche** — military-only, requires government clearance + certified hardware |

**Notes:** JREAP-C (IP tunneling of Link 16) may be more accessible for simulation/testing environments.

---

### CoT (Cursor on Target)
| Field | Value |
|-------|-------|
| **Domain** | Military/Public Safety — TAK (Team Awareness Kit), ATAK, WinTAK, iTAK |
| **Standard** | Informal (MITRE Corp origin), now de facto standard for SA tools |
| **Format** | XML (CoT XML), also Protobuf (proto-CoT for bandwidth efficiency) |
| **Rust Crates** | No dedicated crate; `quick-xml` + custom schema; `prost` for proto-CoT |
| **Difficulty** | Easy-Medium — XML schema is well-documented, free spec available |
| **ORP Importance** | **CRITICAL** — dominant first responder & military SA protocol, huge ATAK ecosystem |

**Schema:** `<event>` root with `<point>`, `<detail>` children. UID, type (a-f-G-U-C = friendly ground unit), how, time, stale, start attributes.

**Transports:** UDP multicast (239.2.3.1:6969), TCP unicast, TAK Server (SSL), FreeTAKServer.

---

### VMF (Variable Message Format)
| Field | Value |
|-------|-------|
| **Domain** | Military — US Army digital comms (SINCGARS, EPLRS, FBCB2/BFT) |
| **Standard** | MIL-STD-47001 |
| **Format** | Binary — variable-length fields, message type headers |
| **Rust Crates** | None |
| **Difficulty** | Hard — military standard, limited public documentation |
| **ORP Importance** | **Niche** — US Army specific |

---

### NFFI (NATO Friendly Force Information)
| Field | Value |
|-------|-------|
| **Domain** | Military — NATO land forces situational awareness |
| **Standard** | STANAG 5527 |
| **Format** | XML over IP |
| **Rust Crates** | None |
| **Difficulty** | Hard — NATO classified portions, requires STANAG access |
| **ORP Importance** | **Niche** — NATO military only |

---

### OTH-Gold / JREAP
| Field | Value |
|-------|-------|
| **Domain** | Military — over-the-horizon sensor data, track relay |
| **Standard** | JREAP (Joint Range Extension Application Protocol) — MIL-STD-3011 |
| **Format** | Binary — JREAP-A (serial), JREAP-C (TCP/IP tunneling Link 16 J-messages) |
| **Rust Crates** | None |
| **Difficulty** | Hard — military standard |
| **ORP Importance** | **Niche** — military coalition networks only |

---

## 4. IoT / Industrial

### MQTT
| Field | Value |
|-------|-------|
| **Domain** | IoT — telemetry from billions of sensors (weather, energy, industrial, consumer) |
| **Standard** | OASIS MQTT 3.1.1 / 5.0 |
| **Format** | Binary framing (control header + variable header + payload); payload is application-defined (often JSON) |
| **Rust Crates** | `rumqttc` (async client, most popular), `mqtt-async-client`, `paho-mqtt` (FFI) |
| **Difficulty** | Easy — well-documented, many examples, broker handles routing |
| **ORP Importance** | **CRITICAL** — dominant IoT protocol; ORP should be an MQTT subscriber |

**Notes:** ORP should subscribe to configurable MQTT topics and ingest payloads as sensor events. Support SparkplugB payload format (Protobuf over MQTT, industrial standard).

---

### OPC-UA (OPC Unified Architecture)
| Field | Value |
|-------|-------|
| **Domain** | Industrial — SCADA, PLCs, manufacturing, energy, process control |
| **Standard** | IEC 62541, OPC Foundation |
| **Format** | Binary (UA Binary) or XML, over TCP (opc.tcp://) or HTTPS |
| **Rust Crates** | `opcua` (comprehensive OPC-UA client/server), `open62541-sys` (FFI to C lib) |
| **Difficulty** | Hard — complex security model (certificates), large spec, node address space |
| **ORP Importance** | **Critical** — industrial automation standard, energy sector, smart factories |

---

### Modbus
| Field | Value |
|-------|-------|
| **Domain** | Industrial — PLCs, sensors, SCADA (oldest, most deployed ICS protocol) |
| **Standard** | Modbus Organization (open) |
| **Format** | Binary — RTU (serial, compact) or TCP (port 502, with MBAP header) |
| **Rust Crates** | `tokio-modbus` (async TCP/RTU), `modbus` (basic), `modbus-mqtt` (bridge) |
| **Difficulty** | Easy — simple register/coil model, tiny protocol |
| **ORP Importance** | **Critical** — deployed in millions of industrial devices worldwide |

**Key operations:** Read Coils (FC1), Read Discrete Inputs (FC2), Read Holding Registers (FC3), Read Input Registers (FC4), Write Single Coil (FC5), Write Single Register (FC6)

---

### BACnet
| Field | Value |
|-------|-------|
| **Domain** | Building automation — HVAC, fire safety, access control, lighting |
| **Standard** | ASHRAE 135, ISO 16484-5 |
| **Format** | Binary — BACnet/IP (UDP port 47808), BACnet MS/TP (serial), BACnet/SC (WebSocket) |
| **Rust Crates** | `bacnet-rs` (in development), `bacnet` (partial); more mature options in C (`bacnet-stack`) |
| **Difficulty** | Medium-Hard — complex object model, many optional services |
| **ORP Importance** | **Useful** — building management integration, smart campus/facilities |

---

### DNP3 / IEC 60870-5
| Field | Value |
|-------|-------|
| **Domain** | Power / SCADA — utilities, substations, water treatment, pipeline control |
| **Standard** | DNP3 (IEEE 1815), IEC 60870-5-101/104 |
| **Format** | Binary — serial or TCP; data objects with variation codes |
| **Rust Crates** | `dnp3` (rodbus-adjacent, OpenDNP3 quality), `dnp3-rs` |
| **Difficulty** | Hard — time synchronization, CRC, complex data object model |
| **ORP Importance** | **Critical** — utility/energy sector SCADA, cybersecurity monitoring |

---

### Zigbee / Z-Wave
| Field | Value |
|-------|-------|
| **Domain** | IoT — smart home, building automation, mesh sensor networks |
| **Standard** | IEEE 802.15.4 (PHY/MAC), Zigbee Alliance |
| **Format** | Binary frames over 2.4 GHz mesh radio |
| **Rust Crates** | `zigbee-rs` (partial); typically accessed via Zigbee2MQTT (exposes as MQTT) |
| **Difficulty** | Hard at radio level; Easy via Zigbee2MQTT bridge |
| **ORP Importance** | **Useful** — ingest via MQTT bridge rather than raw radio |

---

### LoRaWAN
| Field | Value |
|-------|-------|
| **Domain** | IoT — long-range, low-power sensors (agriculture, asset tracking, smart city) |
| **Standard** | LoRa Alliance LoRaWAN Specification |
| **Format** | Binary — PHYPayload with MAC header + encrypted payload; delivered via Semtech UDP or LNS API |
| **Rust Crates** | `lorawan` (LoRaWAN frame encoding/decoding), `lorawan-device` |
| **Difficulty** | Medium — crypto (AES-128), uplink/downlink framing, gateway protocol |
| **ORP Importance** | **Useful** — long-range IoT sensors for perimeter, environmental monitoring |

---

## 5. Cyber / Network

### Syslog (RFC 5424 / RFC 3164)
| Field | Value |
|-------|-------|
| **Domain** | Cyber / IT — system and network event logging (Linux, routers, firewalls, apps) |
| **Standard** | RFC 5424 (structured), RFC 3164 (legacy BSD syslog) |
| **Format** | ASCII text — `<PRI>VERSION TIMESTAMP HOSTNAME APP-NAME PROCID MSGID [STRUCTURED-DATA] MSG` |
| **Rust Crates** | `syslog-rfc5424` (parser), `syslog` (client/sender), `syslogparser` |
| **Difficulty** | Easy — text-based, well-documented RFC |
| **ORP Importance** | **CRITICAL** — ubiquitous in all IT/security environments |

---

### CEF (Common Event Format)
| Field | Value |
|-------|-------|
| **Domain** | Cyber / SIEM — ArcSight, Splunk, IBM QRadar event format |
| **Standard** | Micro Focus / ArcSight CEF Specification |
| **Format** | Text — `CEF:Version|Device Vendor|Device Product|Device Version|Signature ID|Name|Severity|Extensions` |
| **Rust Crates** | No dedicated crate; parse with regex/nom; `cef-parser` (unpublished) |
| **Difficulty** | Easy — structured text, simple delimiter format |
| **ORP Importance** | **Critical** — dominant SIEM event format |

---

### LEEF (Log Event Extended Format)
| Field | Value |
|-------|-------|
| **Domain** | Cyber / SIEM — IBM QRadar native format |
| **Standard** | IBM LEEF 1.0 / 2.0 |
| **Format** | Text — tab-delimited key-value pairs |
| **Rust Crates** | None; trivial to implement |
| **Difficulty** | Easy |
| **ORP Importance** | **Useful** — QRadar environments |

---

### STIX / TAXII (Structured Threat Information Expression)
| Field | Value |
|-------|-------|
| **Domain** | Cyber — threat intelligence sharing (IOCs, TTPs, campaigns, actors) |
| **Standard** | OASIS CTI TC — STIX 2.1, TAXII 2.1 |
| **Format** | JSON (STIX objects over TAXII REST API) |
| **Rust Crates** | No dedicated crate; `reqwest` + `serde_json` for TAXII client |
| **Difficulty** | Medium — large STIX object model, TAXII pagination |
| **ORP Importance** | **Critical** — threat intelligence feeds for cyber ops |

---

### NetFlow / IPFIX
| Field | Value |
|-------|-------|
| **Domain** | Network — traffic flow analysis, network monitoring, DDoS detection |
| **Standard** | Cisco NetFlow v5/v9, IETF IPFIX (RFC 7011) |
| **Format** | Binary UDP datagrams — header + flow records |
| **Rust Crates** | `netflow_parser` (v5/v7/v9/IPFIX), `ipfix` |
| **Difficulty** | Medium — v5 easy, v9/IPFIX requires template tracking |
| **ORP Importance** | **Critical** — network traffic visibility, SOC analytics |

---

### PCAP / PCAPNG
| Field | Value |
|-------|-------|
| **Domain** | Network — packet capture files (Wireshark, tcpdump, Zeek) |
| **Standard** | libpcap format, IETF PCAPNG (RFC 8252) |
| **Format** | Binary — global header + packet records with timestamps |
| **Rust Crates** | `pcap-parser` (streaming, nom-based), `pcap-file` (read/write), `pcap` (live capture via libpcap) |
| **Difficulty** | Easy (file format); Medium (live capture + protocol decoding) |
| **ORP Importance** | **Critical** — forensics, incident response, offline analysis |

---

### Zeek Logs
| Field | Value |
|-------|-------|
| **Domain** | Cyber — network security monitor logs (conn.log, dns.log, http.log, ssl.log, etc.) |
| **Standard** | Zeek (formerly Bro) project |
| **Format** | TSV (tab-separated) or JSON; structured with header metadata |
| **Rust Crates** | No dedicated crate; parse as TSV/JSON with `csv` or `serde_json` |
| **Difficulty** | Easy — TSV/JSON, well-documented field names |
| **ORP Importance** | **Critical** — Zeek is backbone of many SOC/NSM deployments |

---

### SNMP (Simple Network Management Protocol)
| Field | Value |
|-------|-------|
| **Domain** | Network — device monitoring (routers, switches, servers) |
| **Standard** | RFC 1157 (v1), RFC 1901 (v2c), RFC 3411 (v3) |
| **Format** | BER-encoded ASN.1 binary over UDP 161/162 |
| **Rust Crates** | `snmp` (v2c client), `snmp2` |
| **Difficulty** | Medium — ASN.1/BER encoding, MIB resolution |
| **ORP Importance** | **Useful** — infrastructure monitoring |

---

## 6. Automotive / Transport

### CAN Bus (Controller Area Network)
| Field | Value |
|-------|-------|
| **Domain** | Automotive / Industrial — vehicle ECU communication, robotics, automation |
| **Standard** | ISO 11898 (CAN), ISO 15765 (CAN-TP), CAN FD (ISO 11898-7) |
| **Format** | Binary — 11-bit or 29-bit arbitration ID + up to 8 bytes data (64 bytes CAN FD) |
| **Rust Crates** | `socketcan` (Linux SocketCAN interface), `can-dbc` (DBC file parser for signal decoding), `canparse` |
| **Difficulty** | Medium — raw frames easy; signal decoding requires DBC files |
| **ORP Importance** | **Useful** — vehicle telematics, OBD-II, fleet management |

---

### SAE J1939
| Field | Value |
|-------|-------|
| **Domain** | Automotive — heavy-duty vehicles (trucks, buses, agriculture, construction) |
| **Standard** | SAE J1939 (set of standards over CAN) |
| **Format** | Binary — CAN 29-bit extended frames; PGN-based parameter groups |
| **Rust Crates** | `j1939` (PGN parsing), `socketcan` (for CAN interface) |
| **Difficulty** | Medium — PGN lookup tables, SPN (Suspect Parameter Number) decoding |
| **ORP Importance** | **Useful** — fleet/logistics, heavy machinery monitoring |

---

### GTFS (General Transit Feed Specification)
| Field | Value |
|-------|-------|
| **Domain** | Transport — public transit schedules and real-time vehicle positions |
| **Standard** | Google/MobilityData GTFS Static, GTFS-Realtime |
| **Format** | Static: CSV ZIP archive; Realtime: Protobuf (FeedMessage) over HTTP |
| **Rust Crates** | No dedicated crate; `prost` for protobuf decoding of GTFS-RT |
| **Difficulty** | Easy (static CSV), Medium (realtime protobuf + trip matching) |
| **ORP Importance** | **Useful** — public safety, urban mobility, emergency routing |

---

### AIS (Inland Waterways — IALA)
| Field | Value |
|-------|-------|
| **Domain** | Inland waterways — river/canal vessel tracking (European RAINIER project) |
| **Standard** | IALA Recommendation on AIS, UN ECE Resolution 57 |
| **Format** | Same as maritime AIS (NMEA VDM/VDO) with extended inland-specific fields |
| **Rust Crates** | Same as maritime AIS (`nmea-parser`, `ais`) |
| **Difficulty** | Medium — superset of maritime AIS |
| **ORP Importance** | **Useful** — inland navigation, port logistics |

---

## 7. Weather / Environment

### METAR / TAF
| Field | Value |
|-------|-------|
| **Domain** | Aviation / Weather — aviation weather observations and forecasts |
| **Standard** | ICAO Annex 3, WMO No. 306 |
| **Format** | ASCII text — coded abbreviation format (e.g., `METAR WSSS 260830Z 22006KT ...`) |
| **Rust Crates** | `metar` (METAR parser), `avwx` (wrapper around AVWX API) |
| **Difficulty** | Easy-Medium — text format but many abbreviation codes |
| **ORP Importance** | **Critical** — aviation ops, maritime routing, emergency operations |

---

### BUFR (Binary Universal Form for Representation)
| Field | Value |
|-------|-------|
| **Domain** | Meteorology — WMO standard for synoptic observations (radiosondes, satellites, ships) |
| **Standard** | WMO FM 94 BUFR |
| **Format** | Binary — section-based with descriptor tables (BUFR tables B, C, D) |
| **Rust Crates** | No Rust crates; Python: `eccodes` (ECMWF), `bufr4all` |
| **Difficulty** | Hard — complex descriptor tables, self-referential, WMO tables required |
| **ORP Importance** | **Useful** — raw weather data ingest from NWP systems |

---

### GRIB (GRIdded Binary)
| Field | Value |
|-------|-------|
| **Domain** | Meteorology — numerical weather prediction model output (ECMWF, GFS, NAM) |
| **Standard** | WMO FM 92 GRIB (Edition 1 and 2) |
| **Format** | Binary — gridded field data with packing compression |
| **Rust Crates** | No Rust crates; Python: `cfgrib`, `pygrib`; C: `eccodes` |
| **Difficulty** | Hard — packing schemes, grid definitions, parameter tables |
| **ORP Importance** | **Useful** — weather routing, hurricane tracking overlays |

---

### GeoJSON
| Field | Value |
|-------|-------|
| **Domain** | Universal — geospatial data interchange (web maps, GIS, sensors) |
| **Standard** | RFC 7946 |
| **Format** | JSON — Feature, FeatureCollection, Point, LineString, Polygon geometries |
| **Rust Crates** | `geojson` (comprehensive), `geo` (geometry types), `geo-types` |
| **Difficulty** | Easy — standard JSON |
| **ORP Importance** | **CRITICAL** — primary output format for ORP geospatial data |

---

### OGC WFS / WMS / WCS
| Field | Value |
|-------|-------|
| **Domain** | GIS — geospatial web services (layers, features, coverages) |
| **Standard** | OGC (Open Geospatial Consortium) Web Feature/Map/Coverage Service |
| **Format** | XML/GML (WFS features), PNG/JPEG tiles (WMS), GeoTIFF (WCS) |
| **Rust Crates** | No dedicated crates; use `reqwest` + `quick-xml` |
| **Difficulty** | Medium — XML/GML parsing, OGC filter encoding |
| **ORP Importance** | **Useful** — map layer services, geospatial context |

---

### IWXXM / SIGMET / PIREP
| Field | Value |
|-------|-------|
| **Domain** | Aviation Weather — significant meteorological events, pilot reports |
| **Standard** | ICAO Doc 8896, IWXXM (WMO XML for aviation) |
| **Format** | XML (IWXXM), coded text (SIGMET/PIREP) |
| **Rust Crates** | No dedicated crates |
| **Difficulty** | Medium |
| **ORP Importance** | **Useful** — aviation situational awareness |

---

## 8. Emergency / Public Safety

### CAP (Common Alerting Protocol)
| Field | Value |
|-------|-------|
| **Domain** | Emergency Management — public alerts (AMBER, weather, tsunami, civil emergency) |
| **Standard** | OASIS CAP 1.2, ITU-T X.1303 |
| **Format** | XML — `<alert>` with `<info>`, `<area>`, `<resource>` elements |
| **Rust Crates** | No dedicated crate; `quick-xml` + custom structs; trivial to implement |
| **Difficulty** | Easy — simple, well-documented XML schema |
| **ORP Importance** | **CRITICAL** — alerts from NWS, FEMA IPAWS, international agencies |

**Distribution:** ATOM/RSS feeds, XMPP, HTTPS push, EDXL-DE envelope

---

### EDXL (Emergency Data Exchange Language)
| Field | Value |
|-------|-------|
| **Domain** | Emergency Management — interoperability between dispatch, EOC, hospitals |
| **Standard** | OASIS EDXL-DE (Distribution Element), EDXL-HAVE (hospital availability), EDXL-SITREP |
| **Format** | XML — envelope for routing, targeting, and packaging CAP or other payloads |
| **Rust Crates** | None |
| **Difficulty** | Medium — XML envelope, routing expressions |
| **ORP Importance** | **Useful** — emergency interoperability, mass casualty, EOC integration |

---

### P25 (APCO Project 25)
| Field | Value |
|-------|-------|
| **Domain** | Public Safety — police, fire, EMS digital radio (LMR replacement) |
| **Standard** | APCO/TIA-102 |
| **Format** | Binary — IMBE/AMBE voice codec + digital control channel (TSBK, PDU) |
| **Rust Crates** | None; `op25` (GNU Radio Python) for SDR decoding |
| **Difficulty** | Hard — proprietary vocoder, radio framing, encryption (AES-256 optional) |
| **ORP Importance** | **Useful** — public safety radio monitoring, dispatch integration |

**Notes:** For ORP, consider ingesting from P25 gateways or CAD/dispatch systems rather than raw radio decoding.

---

### NIEM (National Information Exchange Model)
| Field | Value |
|-------|-------|
| **Domain** | Public Safety / Government — US data exchange standard (law enforcement, justice, emergency) |
| **Standard** | NIEM 5.0 (US DOJ/DHS) |
| **Format** | XML with strict schema conformance |
| **Rust Crates** | None; handle with `quick-xml` |
| **Difficulty** | Hard — massive schema library |
| **ORP Importance** | **Useful** — US government integrations |

---

## Implementation Roadmap

### Phase 1 — CRITICAL (Build First)

| Priority | Protocol | Effort | Notes |
|----------|----------|--------|-------|
| 1 | NMEA 0183 | Low | Use `nmea-parser` crate |
| 2 | AIS (maritime) | Low | Use `nmea-parser` or `ais` crate |
| 3 | ADS-B / Mode S | Medium | Use `adsb_deku` |
| 4 | MQTT | Low | Use `rumqttc`; ingest JSON/SparkplugB payloads |
| 5 | CoT (Cursor on Target) | Medium | Implement XML + proto-CoT |
| 6 | GeoJSON | Trivial | Use `geojson` crate |
| 7 | Syslog RFC 5424 | Low | Use `syslog-rfc5424` |
| 8 | NetFlow / IPFIX | Medium | Use `netflow_parser` |
| 9 | METAR / TAF | Medium | Use `metar` crate |
| 10 | CAP (Common Alerting) | Low | Implement XML parser |

### Phase 2 — CRITICAL Infrastructure

| Priority | Protocol | Effort | Notes |
|----------|----------|--------|-------|
| 11 | Modbus TCP | Low | Use `tokio-modbus` |
| 12 | ASTERIX | Medium | Use `asterix` crate (deku-based) |
| 13 | STIX 2.1 / TAXII | Medium | JSON + REST client |
| 14 | PCAP | Low | Use `pcap-parser` |
| 15 | CEF | Low | Regex/nom parser |
| 16 | Zeek Logs | Low | TSV/JSON parsing |

### Phase 3 — USEFUL Integrations

| Priority | Protocol | Effort | Notes |
|----------|----------|--------|-------|
| 17 | OPC-UA | High | Use `opcua` crate |
| 18 | NMEA 2000 | High | Canboat FFI or subprocess |
| 19 | ACARS | High | `libacars` FFI |
| 20 | CAN / J1939 | Medium | `socketcan` + DBC files |
| 21 | GTFS-Realtime | Medium | Protobuf (`prost`) |
| 22 | LoRaWAN | Medium | Use `lorawan` crate |
| 23 | DNP3 | High | Use `dnp3` crate |
| 24 | BACnet | High | Partial Rust support |
| 25 | BUFR / GRIB | High | FFI to `eccodes` |

### Phase 4 — NICHE / Specialized

| Priority | Protocol | Effort | Notes |
|----------|----------|--------|-------|
| 26 | Link 16 / TADIL-J | Infeasible | Classified, skip |
| 27 | MIL-STD-1553 | Infeasible | Hardware-only, skip |
| 28 | ARINC 429 | Infeasible | Hardware-only, skip |
| 29 | P25 | Hard | Gateway approach |
| 30 | SWIM | Hard | Credentialed access only |
| 31 | NIEM | Hard | Government only |
| 32 | VMF | Hard | Military only |
| 33 | NFFI | Hard | NATO classified portions |

---

## Rust Crate Registry

| Protocol | Crate | Quality | Notes |
|----------|-------|---------|-------|
| NMEA 0183 | `nmea` | ⭐⭐⭐ | Simple GNSS sentences |
| NMEA 0183 + AIS | `nmea-parser` | ⭐⭐⭐⭐ | Best all-around |
| NMEA 0183 | `nmea0183-parser` | ⭐⭐⭐ | nom-based, fast |
| AIS | `ais` | ⭐⭐⭐ | Standalone AIS |
| AIS + MQTT | `rs162` / `ship162` | ⭐⭐ | SDR→MQTT pipeline |
| ADS-B / Mode S | `adsb` | ⭐⭐⭐ | Good DF coverage |
| ADS-B / Mode S | `adsb_deku` | ⭐⭐⭐⭐ | deku-based, comprehensive |
| ASTERIX | `asterix` | ⭐⭐⭐ | deku encode/decode |
| ASTERIX | `asterix_parser` | ⭐⭐ | Parser only |
| MQTT | `rumqttc` | ⭐⭐⭐⭐⭐ | Most popular async MQTT |
| OPC-UA | `opcua` | ⭐⭐⭐⭐ | Comprehensive |
| Modbus | `tokio-modbus` | ⭐⭐⭐⭐ | Async, TCP + RTU |
| DNP3 | `dnp3` | ⭐⭐⭐ | Async, robust |
| LoRaWAN | `lorawan` | ⭐⭐⭐ | Frame codec |
| CAN bus | `socketcan` | ⭐⭐⭐⭐ | Linux SocketCAN |
| CAN DBC | `can-dbc` | ⭐⭐⭐ | DBC file parsing |
| J1939 | `j1939` | ⭐⭐ | PGN parsing |
| NetFlow/IPFIX | `netflow_parser` | ⭐⭐⭐⭐ | v5/v7/v9/IPFIX |
| PCAP | `pcap-parser` | ⭐⭐⭐⭐ | Streaming, nom |
| PCAP | `pcap-file` | ⭐⭐⭐⭐ | Read/write PCAP/NG |
| PCAP live | `pcap` | ⭐⭐⭐⭐ | libpcap FFI |
| Syslog | `syslog-rfc5424` | ⭐⭐⭐ | RFC 5424 parser |
| GeoJSON | `geojson` | ⭐⭐⭐⭐⭐ | Comprehensive |
| METAR | `metar` | ⭐⭐⭐ | METAR decoder |
| Protobuf (GTFS/CoT) | `prost` | ⭐⭐⭐⭐⭐ | Universal protobuf |
| XML (CAP/CoT/EDXL) | `quick-xml` | ⭐⭐⭐⭐⭐ | Fast XML |
| SNMP | `snmp` | ⭐⭐⭐ | v2c client |

---

## Protocol Complexity Matrix

```
COMPLEXITY vs IMPORTANCE

                    LOW COMPLEXITY    MEDIUM COMPLEXITY    HIGH COMPLEXITY
HIGH IMPORTANCE  │ NMEA 0183         AIS                  ADS-B
                 │ GeoJSON           CoT (XML)            ASTERIX
                 │ MQTT              NetFlow/IPFIX        OPC-UA
                 │ Syslog            METAR                NMEA 2000
                 │ CEF               STIX/TAXII           DNP3
                 │ CAP               PCAP analysis        ACARS
─────────────────┤──────────────────────────────────────────────────────
MEDIUM IMPORTANCE│ Zeek Logs         CAN/J1939            BACnet
                 │ LEEF              GTFS-RT              BUFR/GRIB
                 │ SNMP              LoRaWAN              SWIM
                 │                   P25 (gateway)        NIEM
─────────────────┤──────────────────────────────────────────────────────
LOW IMPORTANCE   │                   ARINC 429 (HW)       Link 16 (classified)
(niche/locked)   │                   MIL-STD-1553 (HW)   VMF
                 │                   NFFI                 OTH-Gold/JREAP
```

---

## Universal Data Model for ORP

All protocols, regardless of source, should normalize into ORP's universal event schema:

```json
{
  "id": "uuid-v4",
  "source_protocol": "NMEA_0183 | AIS | ADSB | MQTT | COT | ...",
  "source_adapter": "serial://ttyUSB0 | udp://239.2.3.1:6969 | mqtt://broker:1883",
  "timestamp": "ISO-8601 UTC",
  "entity_type": "vessel | aircraft | vehicle | sensor | alert | track | event",
  "identity": {
    "uid": "MMSI / ICAO / call_sign / hostname / ...",
    "name": "human-readable name",
    "classification": "friendly | neutral | unknown | hostile | assumed_friendly"
  },
  "position": {
    "lat": 0.0,
    "lon": 0.0,
    "alt_m": null,
    "accuracy_m": null,
    "datum": "WGS84"
  },
  "motion": {
    "course_deg": null,
    "speed_knots": null,
    "speed_mps": null,
    "heading_deg": null,
    "vertical_rate_fpm": null
  },
  "raw": {
    "format": "NMEA | AIS | BEAST | ...",
    "payload": "original raw message (base64 if binary)"
  },
  "metadata": {},
  "tags": []
}
```

---

*This document is the authoritative protocol reference for ORP. Update as new protocols are implemented.*
