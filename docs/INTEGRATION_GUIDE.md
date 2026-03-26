# ORP Integration Guide

> Connect **any system** to ORP in minutes. No schema. No SDK required.

---

## Table of Contents

1. [Universal Ingest — Just POST JSON](#1-universal-ingest--just-post-json)
2. [Examples by Entity Type](#2-examples-by-entity-type)
3. [Python — 5 Lines to Connect](#3-python--5-lines-to-connect)
4. [JavaScript — 5 Lines to Connect](#4-javascript--5-lines-to-connect)
5. [MQTT Bridge](#5-mqtt-bridge)
6. [Webhook Receiver](#6-webhook-receiver)
7. [Federation — Connecting Two ORP Instances](#7-federation--connecting-two-orp-instances)
8. [Raspberry Pi Deployment](#8-raspberry-pi-deployment)
9. [Docker One-liner](#9-docker-one-liner)

---

## 1. Universal Ingest — Just POST JSON

ORP's universal ingest endpoint accepts **any JSON payload**. It auto-detects
the entity type, generates a stable ID, and stores it — no pre-configuration
required.

```
POST /api/v1/ingest
Content-Type: application/json
Authorization: Bearer <your_api_key>
```

ORP detects the entity type by inspecting fields in priority order:

| Fields present                          | Assigned type |
|-----------------------------------------|---------------|
| `mmsi` or `imo`                         | `ship`        |
| `icao` or (`callsign` + `altitude`)     | `aircraft`    |
| `ip` or `hostname`                      | `host`        |
| `cve` or `vulnerability`                | `threat`      |
| `temperature` or `humidity`             | `sensor`      |
| `plate` or `vin`                        | `vehicle`     |
| `lat` + `lon` (nothing else matched)    | `point`       |
| Anything else                           | `generic`     |

Entity IDs are **auto-generated** from identifying fields (e.g. `mmsi`, `icao`,
`vin`). Re-ingesting the same physical entity updates rather than duplicates it.

### Batch ingest

```
POST /api/v1/ingest/batch
Content-Type: application/json
```

Body: a JSON array of up to 1,000 objects. Partial failures are reported
without aborting the rest of the batch.

```json
[
  {"mmsi": "123456789", "lat": 51.92, "lon": 4.47},
  {"icao": "A12345", "altitude": 35000},
  {"temperature": 22.5, "humidity": 61.2, "device_id": "sensor-001"}
]
```

---

## 2. Examples by Entity Type

### Ship (AIS data)

```bash
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "mmsi": "123456789",
    "name": "MV Ever Given",
    "lat": 30.6936,
    "lon": 32.3000,
    "speed": 9.2,
    "heading": 180,
    "course": 179.5,
    "draught": 14.5,
    "destination": "NLRTM"
  }'
```

### Aircraft (ADS-B data)

```bash
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "icao": "A1B2C3",
    "callsign": "SQ321",
    "altitude": 38000,
    "lat": 1.3521,
    "lon": 103.8198,
    "speed": 512,
    "heading": 45,
    "vertical_rate": 0,
    "squawk": "1234"
  }'
```

### IoT Sensor

```bash
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "device_id": "sensor-warehouse-01",
    "temperature": 22.5,
    "humidity": 61.2,
    "pressure": 1013.25,
    "lat": 51.9225,
    "lon": 4.4792,
    "battery_pct": 87
  }'
```

### Cyber Threat (CVE / vulnerability)

```bash
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "cve": "CVE-2024-12345",
    "vulnerability": "Remote code execution in nginx < 1.25.4",
    "severity": "critical",
    "cvss": 9.8,
    "affected_system": "nginx",
    "affected_versions": ["< 1.25.4"],
    "patch_available": true
  }'
```

### Vehicle

```bash
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "vin": "1HGCM82633A004352",
    "plate": "ABC-1234",
    "make": "Honda",
    "model": "Accord",
    "year": 2023,
    "lat": 37.7749,
    "lon": -122.4194,
    "speed_kmh": 0
  }'
```

---

## 3. Python — 5 Lines to Connect

```python
import requests, os

ORP = os.getenv("ORP_URL", "http://localhost:8080")
KEY = os.getenv("ORP_API_KEY")

def ingest(payload: dict) -> dict:
    return requests.post(f"{ORP}/api/v1/ingest", json=payload,
                         headers={"Authorization": f"Bearer {KEY}"}).json()
```

**Usage:**

```python
# Ship
ingest({"mmsi": "123456789", "lat": 51.92, "lon": 4.47, "speed": 12.3})

# Batch
r = requests.post(f"{ORP}/api/v1/ingest/batch",
    json=[{"temperature": 22.5, "device_id": "s01"},
          {"cve": "CVE-2024-0001", "severity": "high"}],
    headers={"Authorization": f"Bearer {KEY}"})
print(r.json()["summary"])
```

---

## 4. JavaScript — 5 Lines to Connect

```javascript
const ORP = process.env.ORP_URL ?? 'http://localhost:8080';
const KEY = process.env.ORP_API_KEY;

const ingest = async (payload) =>
  fetch(`${ORP}/api/v1/ingest`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'Authorization': `Bearer ${KEY}` },
    body: JSON.stringify(payload),
  }).then(r => r.json());
```

**Usage:**

```javascript
// From a browser/Node.js app
await ingest({ icao: 'A1B2C3', callsign: 'SQ321', altitude: 38000, lat: 1.35, lon: 103.82 });

// Batch
const res = await fetch(`${ORP}/api/v1/ingest/batch`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json', 'Authorization': `Bearer ${KEY}` },
  body: JSON.stringify([
    { mmsi: '123456789', lat: 51.92, lon: 4.47 },
    { ip: '10.0.1.5', hostname: 'prod-server-01', os: 'Ubuntu 22.04' },
  ]),
});
console.log(await res.json());
```

---

## 5. MQTT Bridge

Subscribe to an MQTT topic and forward every message to ORP's ingest endpoint.

### Python bridge (production-ready)

```python
#!/usr/bin/env python3
"""
mqtt_bridge.py — subscribe to MQTT, forward to ORP ingest.
Usage: ORP_URL=http://localhost:8080 ORP_API_KEY=<key> \
       MQTT_HOST=broker.example.com python mqtt_bridge.py
"""
import json, os, logging
import paho.mqtt.client as mqtt
import requests

logging.basicConfig(level=logging.INFO)
log = logging.getLogger("orp-mqtt-bridge")

ORP_INGEST = f"{os.getenv('ORP_URL', 'http://localhost:8080')}/api/v1/ingest"
HEADERS    = {"Authorization": f"Bearer {os.getenv('ORP_API_KEY', '')}",
              "Content-Type": "application/json"}
TOPIC      = os.getenv("MQTT_TOPIC", "#")

def on_message(client, userdata, msg):
    try:
        payload = json.loads(msg.payload.decode())
        # Inject the MQTT topic as metadata so ORP can correlate sources
        payload.setdefault("mqtt_topic", msg.topic)
        r = requests.post(ORP_INGEST, json=payload, headers=HEADERS, timeout=5)
        r.raise_for_status()
        log.info("Ingested entity id=%s type=%s", r.json().get("id"), r.json().get("type"))
    except Exception as e:
        log.warning("Ingest failed: %s", e)

client = mqtt.Client()
client.on_message = on_message
client.connect(os.getenv("MQTT_HOST", "localhost"), int(os.getenv("MQTT_PORT", 1883)))
client.subscribe(TOPIC)
log.info("Bridging MQTT %s → ORP", TOPIC)
client.loop_forever()
```

Install: `pip install paho-mqtt requests`

---

## 6. Webhook Receiver

Expose a webhook endpoint that feeds data into ORP from any third-party service
(GitHub, PagerDuty, Shodan alerts, etc.).

### Minimal Flask webhook

```python
#!/usr/bin/env python3
"""
webhook_receiver.py — receive webhooks, forward to ORP.
"""
from flask import Flask, request
import requests, os, logging

app = Flask(__name__)
ORP_INGEST = f"{os.getenv('ORP_URL', 'http://localhost:8080')}/api/v1/ingest"
HEADERS    = {"Authorization": f"Bearer {os.getenv('ORP_API_KEY', '')}"}

@app.route("/webhook", methods=["POST"])
def webhook():
    payload = request.get_json(force=True) or {}
    # Add source metadata
    payload["webhook_source"] = request.headers.get("X-Webhook-Source", "unknown")
    r = requests.post(ORP_INGEST, json=payload, headers=HEADERS, timeout=5)
    return {"status": "ok", "entity_id": r.json().get("id")}, 200

if __name__ == "__main__":
    app.run(host="0.0.0.0", port=int(os.getenv("PORT", 5000)))
```

Install: `pip install flask requests`

**Deploy behind nginx:**

```nginx
location /webhook {
    proxy_pass http://127.0.0.1:5000;
    proxy_set_header Host $host;
    proxy_set_header X-Webhook-Source $http_x_source;
}
```

---

## 7. Federation — Connecting Two ORP Instances

Federation allows two (or more) ORP nodes to automatically share entity data.
Entities are tagged with `source: "peer:<peer_id>"` so you always know their
origin.

### Register a peer

```bash
# On ORP node A — tell it about node B
curl -X POST https://orp-node-a.example.com/api/v1/peers \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "node-b",
    "host": "orp-node-b.example.com",
    "port": 8080,
    "shared_entity_types": ["ship", "aircraft", "threat"],
    "sync_enabled": true
  }'
```

Node A will now pull from node B every 30 seconds automatically.

### List peers

```bash
curl https://orp-node-a.example.com/api/v1/peers \
  -H "Authorization: Bearer $ORP_API_KEY"
```

### Trigger immediate sync

```bash
curl -X POST https://orp-node-a.example.com/api/v1/peers/node-b/sync \
  -H "Authorization: Bearer $ORP_API_KEY"
# → {"status":"ok","peer_id":"node-b","entities_synced":42,"synced_at":"..."}
```

### Remove a peer

```bash
curl -X DELETE https://orp-node-a.example.com/api/v1/peers/node-b \
  -H "Authorization: Bearer $ORP_API_KEY"
```

### Conflict resolution

When both nodes report the same `entity_id`, **the copy with the highest
`confidence` value wins**. Lower-confidence remote data is silently discarded.
This means you can always trust local high-confidence ground truth over
federated estimates.

### Hub-and-spoke topology (3+ nodes)

```
Node C ──→ Node A (hub) ←── Node B
                │
                ↓
            Node D
```

Register A as a peer on B, C, and D. The hub pulls from all spokes every 30 s.
For full mesh, register every node as a peer of every other node.

---

## 8. Raspberry Pi Deployment

ORP is designed to run on constrained hardware. A Raspberry Pi 4 (4 GB RAM)
can handle thousands of entities with sub-millisecond query latency.

### Prerequisites

```bash
# Install Rust (ARM64)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Install system deps
sudo apt-get update && sudo apt-get install -y pkg-config libssl-dev
```

### Build (cross-compile on your Mac, or build natively on the Pi)

```bash
# On the Raspberry Pi:
git clone https://github.com/your-org/orp.git
cd orp
cargo build --release -p orp-core
```

First build takes ~15 min on a Pi 4 (ARM64). Subsequent builds are cached.

### Run

```bash
# Minimal headless mode — no frontend, API + WebSocket only
./target/release/orp serve \
  --port 8080 \
  --headless \
  --dev    # removes auth for local testing; remove in production
```

### Run as a systemd service

```ini
# /etc/systemd/system/orp.service
[Unit]
Description=ORP Entity Intelligence Engine
After=network.target

[Service]
Type=simple
User=pi
WorkingDirectory=/home/pi/orp
ExecStart=/home/pi/orp/target/release/orp serve --port 8080 --headless
Restart=always
RestartSec=5
Environment=RUST_LOG=info
Environment=ORP_CORS_ORIGINS=http://localhost:3000

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable orp
sudo systemctl start orp
```

### Performance on Pi 4 (4 GB)

| Metric                   | Value                |
|--------------------------|----------------------|
| Entities stored          | 500 K+ (DuckDB)      |
| Ingest throughput        | ~2,000 msg/s         |
| Query latency (p99)      | < 5 ms               |
| RAM footprint            | ~120 MB              |
| Storage (500 K entities) | ~400 MB on SD card   |

---

## 9. Docker One-liner

```bash
docker run -d \
  --name orp \
  -p 8080:8080 \
  -v orp-data:/data \
  -e RUST_LOG=info \
  -e ORP_CORS_ORIGINS=http://localhost:3000 \
  ghcr.io/your-org/orp:latest \
  serve --port 8080 --headless
```

### docker-compose (with federation peer)

```yaml
# docker-compose.yml
version: "3.9"
services:
  orp-a:
    image: ghcr.io/your-org/orp:latest
    command: serve --port 8080 --headless
    ports:
      - "8080:8080"
    volumes:
      - orp-a-data:/data
    environment:
      RUST_LOG: info
      ORP_CORS_ORIGINS: "http://localhost:3000"

  orp-b:
    image: ghcr.io/your-org/orp:latest
    command: serve --port 8080 --headless
    ports:
      - "8081:8080"
    volumes:
      - orp-b-data:/data
    environment:
      RUST_LOG: info

volumes:
  orp-a-data:
  orp-b-data:
```

After starting, register node-b as a peer of node-a:

```bash
curl -X POST http://localhost:8080/api/v1/peers \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"id":"node-b","host":"orp-b","port":8080,"shared_entity_types":["ship","aircraft"]}'
```

### Build your own image

```dockerfile
# Dockerfile
FROM rust:1.78-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p orp-core

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/orp /usr/local/bin/orp
VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["orp"]
CMD ["serve", "--port", "8080", "--headless"]
```

```bash
docker build -t orp:local .
docker run -d -p 8080:8080 orp:local
```

---

## 10. New Protocol Examples

### ACARS (Aircraft Data Link)

```bash
# ACARS messages ingested from VHF ground station or Aero satellite feed
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "acars_label": "H1",
    "registration": "9V-SMF",
    "flight": "SQ321",
    "message_text": "/POSREP .SQ321 1432 0133N 10353E 390 503 0139 0143N 10400E",
    "freq_mhz": 129.125,
    "entity_type": "aircraft"
  }'
```

### BACnet (Building Automation)

```bash
# BACnet/IP device readings via ORP's BACnet adapter
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "bacnet_device_id": 102,
    "bacnet_object": "analog-input:1",
    "description": "Zone Temperature",
    "value": 22.4,
    "units": "degrees-celsius",
    "building": "HQ-Block-A",
    "floor": 3,
    "lat": 1.3521,
    "lon": 103.8198,
    "entity_type": "sensor"
  }'
```

### GRIB (Weather Model Data)

```bash
# GRIB-derived weather grid entity (parsed by ORP GRIB adapter)
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "grib_model": "GFS",
    "grib_valid_time": "2026-03-27T06:00:00Z",
    "grib_parameter": "wind_speed_10m",
    "value": 18.5,
    "units": "knots",
    "lat": 1.35,
    "lon": 103.82,
    "entity_type": "weather_grid"
  }'
```

### CEF (Security Events)

```bash
# Common Event Format security event
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "cef_version": 0,
    "device_vendor": "Palo Alto Networks",
    "device_product": "PAN-OS",
    "device_version": "11.0",
    "signature_id": "9999",
    "name": "Brute Force Login Attempt",
    "severity": 7,
    "src_ip": "203.0.113.42",
    "dst_ip": "10.0.1.5",
    "dst_port": 22,
    "entity_type": "threat"
  }'
```

### LoRaWAN (IoT Sensors)

```bash
# LoRaWAN uplink from ChirpStack integration
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "dev_eui": "0102030405060708",
    "app_eui": "0807060504030201",
    "rssi": -85,
    "snr": 7.5,
    "frequency": 868.1,
    "sf": 7,
    "temperature": 28.3,
    "humidity": 72.1,
    "battery_mv": 3300,
    "lat": 1.2935,
    "lon": 103.8565,
    "entity_type": "sensor"
  }'
```

### NMEA 2000 / N2K (Marine CAN Bus)

```bash
# NMEA 2000 PGN decoded entity (from YDWG-02 or similar gateway)
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "n2k_pgn": 129025,
    "n2k_src": 3,
    "name": "MV Sentinel",
    "lat": 1.2678,
    "lon": 103.8527,
    "sog_knots": 8.4,
    "cog_deg": 225.0,
    "hdg_deg": 223.5,
    "depth_m": 18.2,
    "entity_type": "ship"
  }'
```

### NFFI (NATO Friendly Force Information)

```bash
# NATO NFFI track message
curl -X POST https://orp.example.com/api/v1/ingest \
  -H "Authorization: Bearer $ORP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "nffi_unit_id": "BRAVO-2-3",
    "sidc": "SFGPUCII-------",
    "affiliation": "friendly",
    "echelon": "squad",
    "lat": 48.8566,
    "lon": 2.3522,
    "speed_kmh": 12,
    "heading_deg": 090,
    "operational_status": "fully_capable",
    "entity_type": "military_unit"
  }'
```

---

## Quick Reference

| Endpoint                         | Method | Purpose                              |
|----------------------------------|--------|--------------------------------------|
| `/api/v1/ingest`                 | POST   | Ingest any JSON payload              |
| `/api/v1/ingest/batch`           | POST   | Ingest up to 1,000 payloads at once  |
| `/api/v1/entities`               | GET    | List entities (with type filter)     |
| `/api/v1/entities/{id}`          | GET    | Get a single entity                  |
| `/api/v1/entities/search`        | GET    | Text + geospatial search             |
| `/api/v1/peers`                  | POST   | Register a federation peer           |
| `/api/v1/peers`                  | GET    | List registered peers                |
| `/api/v1/peers/{id}`             | DELETE | Remove a peer                        |
| `/api/v1/peers/{id}/sync`        | POST   | Trigger immediate sync with a peer   |
| `/ws/updates`                    | WS     | Real-time entity updates             |
| `/api/v1/health`                 | GET    | Health check (no auth required)      |

---

*ORP — Object-Reality Protocol. The intelligence layer for anything that moves, exists, or matters.*
