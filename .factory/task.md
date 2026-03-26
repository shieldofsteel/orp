# ORP — Wire Analytics to Live Data + Python SDK

## 1. Wire threat/analytics to live feed
In `crates/orp-core/src/cli/commands.rs`, after processing each SourceEvent through the stream processor, also:
- Call analytics engine (CPA, anomaly scoring) on position updates
- Call threat engine (risk scoring) on every entity
- If threat level changes → emit alert via broadcast channel
- Store threat scores as entity properties (risk_score, threat_level)
The analytics.rs and threat.rs modules exist — just wire them into the event processing loop.

## 2. Fix rate limit for ingest
In `crates/orp-core/src/server/http.rs`: exempt /api/v1/ingest and /api/v1/ingest/batch from rate limiting (or set a much higher limit like 1000/sec). The bridge needs to send hundreds of events per second without being throttled.

## 3. Historical track storage
Entity position updates should be stored as events in the events table (they already are via the processor). Add an API endpoint: GET /api/v1/entities/{id}/track — returns last N positions as a GeoJSON LineString. The frontend already has the Track tab in EntityInspector.

## 4. Python SDK (simple, powerful)
Write `sdk/python/orp/__init__.py`:
- `ORPClient(host, port, token)` class
- `client.entities(type=None, near=None, limit=100)` → list
- `client.entity(id)` → dict
- `client.query(orpql)` → list
- `client.ingest(data)` → dict  
- `client.subscribe(entity_type, callback)` → WebSocket listener
- `client.health()` → dict
Also write `sdk/python/setup.py` for pip install.

cargo test + cargo clippy. Commit + push.
