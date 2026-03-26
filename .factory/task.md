# ORP — Wire AISStream.io Real Data

DO NOT run `cargo add` or any npm commands. Dependencies are already added.

## Build
Write `crates/orp-connector/src/adapters/aisstream.rs`:
- Use `tokio_tungstenite` (already in Cargo.toml) + `futures_util` for WebSocket client
- Connect to `wss://stream.aisstream.io/v0/stream`
- Send subscription JSON: `{"APIKey":"...", "BoundingBoxes":[[[-90,-180],[90,180]]], "FilterMessageTypes":["PositionReport","ShipStaticData"]}`
- Parse incoming JSON messages: MessageType, Message.PositionReport (UserID, Latitude, Longitude, Sog, Cog, TrueHeading), MetaData (MMSI, ShipName)
- Map to SourceEvent: entity_type="ship", entity_id="mmsi:{UserID}", properties with speed/course/heading/name
- Auto-reconnect with backoff on disconnect
- Implements Connector trait
- Config: api_key (String), bounding_boxes (Vec), filters
- 10+ tests

Update `adapters/mod.rs` — add `pub mod aisstream;`
Update `lib.rs` — re-export AisStreamConnector

Update `crates/orp-core/src/cli/commands.rs` — in `run_start`:
- Check `std::env::var("AISSTREAM_API_KEY")`
- If set, spawn AisStreamConnector instead of demo AIS connector
- Log: "Connected to AISStream.io — receiving live global AIS data"

cargo test + cargo clippy. Commit + push.
