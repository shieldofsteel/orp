# ORP Python SDK

Lightweight Python client for [ORP — Open Reality Protocol](https://github.com/shieldofsteel/orp).

- ✅ Zero required dependencies (stdlib only)
- ✅ Python 3.8+
- ✅ Full type hints
- ✅ Optional real-time WebSocket subscriptions

---

## Install

```bash
# From source
pip install -e /path/to/orp/sdk/python

# With real-time support
pip install -e "/path/to/orp/sdk/python[realtime]"
```

---

## 5-Line Quick Start — Get Ships

```python
from orp import ORPClient

client = ORPClient(host="localhost", port=9090, token="my-token")
ships = client.entities(type="ship")
print(ships)
```

---

## Full Usage

### Connect

```python
from orp import ORPClient

# Token auth
client = ORPClient(host="orp.example.com", port=9090, token="eyJ...")

# API key auth
client = ORPClient(host="orp.example.com", port=9090, api_key="sk-...")

# TLS
client = ORPClient(host="orp.example.com", port=443, token="eyJ...", tls=True)

# Custom timeout
client = ORPClient(timeout=60)
```

---

### Entities

```python
# All entities
all_entities = client.entities()

# Filter by type
ships = client.entities(type="ship")

# Nearby entities (within 5 km of Singapore)
nearby = client.entities(
    type="ship",
    near={"lat": 1.3521, "lon": 103.8198, "radius_m": 5000},
    limit=50,
)

# Single entity by ID
vessel = client.entity("ent_abc123")
print(vessel["name"], vessel["location"])
```

---

### Search

```python
# Full-text search
results = client.search(query="cargo vessel")

# Typed search
results = client.search(query="Alpha", type="ship")

# Proximity search
results = client.search(
    query="tanker",
    near={"lat": 1.3521, "lon": 103.8198, "radius_m": 10000},
)
```

---

### ORPQL Queries

```python
# Execute raw ORPQL
fast_ships = client.query("MATCH (s:ship) WHERE s.speed > 15 RETURN s")

# Relationship traversal
crew_routes = client.query(
    "MATCH (c:crew)-[:ASSIGNED_TO]->(s:ship) WHERE s.id = 'ent_abc123' RETURN c"
)
```

---

### Ingest

```python
# Single entity
result = client.ingest({
    "type": "ship",
    "name": "MV Sentinel",
    "location": {"lat": 1.3521, "lon": 103.8198},
    "properties": {"mmsi": "563012345", "flag": "SG"},
})
print(result["id"])  # "ent_xyz789"

# Batch ingest
result = client.ingest_batch([
    {"type": "ship", "name": "Ship A", "location": {"lat": 1.2, "lon": 103.7}},
    {"type": "ship", "name": "Ship B", "location": {"lat": 1.4, "lon": 103.9}},
])
print(result["inserted"], result["failed"])
```

---

### System

```python
# Health check
health = client.health()
print(health["status"])   # "ok"
print(health["version"])  # "1.2.3"

# Connectors
for connector in client.connectors():
    print(connector["name"], connector["status"])

# Peer nodes
for peer in client.peers():
    print(peer["host"], peer["latency_ms"])
```

---

### Real-Time Subscriptions

Requires `websocket-client`:

```bash
pip install websocket-client
# or: pip install "orp-client[realtime]"
```

```python
def on_ship_update(entity):
    print(f"[UPDATE] {entity['id']} → {entity.get('location')}")

def on_error(exc):
    print(f"WebSocket error: {exc}")

# Subscribe (non-blocking, runs in background thread)
sub = client.subscribe("ship", on_ship_update, on_error=on_error)

# ... do other work ...

import time
time.sleep(30)

# Unsubscribe
sub.stop()
```

---

### Error Handling

```python
from orp import ORPClient, ORPError, ORPAuthError, ORPNotFoundError

client = ORPClient(token="my-token")

try:
    entity = client.entity("does-not-exist")
except ORPNotFoundError:
    print("Entity not found")
except ORPAuthError:
    print("Invalid credentials")
except ORPError as exc:
    print(f"ORP error {exc.status_code}: {exc}")
```

---

## Type Hints

All methods are fully typed using `TypedDict` from `orp.types`:

```python
from orp.types import Entity, HealthStatus, Connector, Peer, NearFilter

def process(ship: Entity) -> None:
    loc = ship.get("location", {})
    print(loc.get("lat"), loc.get("lon"))
```

---

## License

Apache-2.0 © Shield of Steel — see [LICENSE](../../LICENSE)
