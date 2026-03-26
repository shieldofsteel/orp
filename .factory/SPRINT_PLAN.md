# Sprint: Universal Protocol Parsers

## Goal
Make ORP understand data from ANY domain — military, aviation, cyber, industrial, emergency. Build parsers for the most critical protocols that don't have existing connectors.

## Priority Order (build in this sequence)

### P1: Cursor on Target (CoT) — Military Integration
- XML-based format used by TAK/ATAK
- Every US/NATO military unit uses this
- Parse `<event>` elements with `<point>`, `<detail>` children
- Entity mapping: CoT type → ORP entity type (a-f-G → friendly ground, a-h-A → hostile aircraft, etc.)
- Bidirectional: ORP should also EMIT CoT so TAK clients can subscribe
- File: `crates/orp-connector/src/adapters/cot.rs`

### P2: ASTERIX — Aviation Radar
- Binary protocol from Eurocontrol
- Used by every ATC radar, military radar worldwide
- Category 048 (monoradar target reports) and Category 062 (system track data) are most important
- Parse UAP (User Application Profile) field structure
- File: `crates/orp-connector/src/adapters/asterix.rs`

### P3: STIX/TAXII — Cyber Threat Intelligence
- JSON-based (STIX 2.1) threat intel sharing
- TAXII is the transport protocol (REST API polling)
- Parse: indicators, malware, threat actors, attack patterns, vulnerabilities
- Map to ORP entities: indicator→threat, malware→threat, vulnerability→vulnerability
- File: `crates/orp-connector/src/adapters/stix.rs`

### P4: OPC-UA — Industrial/SCADA
- Industrial automation standard
- Factories, power plants, refineries, water treatment
- Client that subscribes to OPC-UA server nodes
- Map: each monitored node → ORP sensor entity
- File: `crates/orp-connector/src/adapters/opcua.rs`

### P5: CAP — Emergency Alerts
- Common Alerting Protocol (XML)
- FEMA, weather services, earthquake warnings worldwide
- Parse `<alert>` with `<info>`, `<area>` (polygon/circle)
- Map to ORP weather/emergency entities with geofence
- File: `crates/orp-connector/src/adapters/cap.rs`

### P6: Modbus — Legacy Industrial
- Serial/TCP protocol for PLCs, sensors, actuators
- Still used in 80% of industrial installations
- Read holding registers, input registers
- Map register values to ORP sensor properties
- File: `crates/orp-connector/src/adapters/modbus.rs`

## Rules
- Each parser must have 15+ unit tests with real protocol examples
- Each must implement the Connector trait
- Each must auto-detect entity types from the protocol data
- Read `specs/PROTOCOL_UNIVERSE.md` if it exists for Rust crate guidance
- Use existing crates where available (don't reinvent XML/binary parsers)
- `cargo test` + `cargo clippy` after each parser
- Commit after each parser with conventional commits
