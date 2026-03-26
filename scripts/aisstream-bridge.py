#!/usr/bin/env python3
"""AISStream.io → ORP bridge. Connects to live AIS feed and POSTs to ORP's ingest API."""
import asyncio
import websockets
import json
import urllib.request
import sys
import os

ORP_URL = os.environ.get("ORP_URL", "http://localhost:9091")
API_KEY = os.environ.get("AISSTREAM_API_KEY", "")

async def bridge():
    if not API_KEY:
        print("Set AISSTREAM_API_KEY env var")
        sys.exit(1)
    
    print(f"Connecting to AISStream.io → forwarding to {ORP_URL}/api/v1/ingest")
    
    async with websockets.connect("wss://stream.aisstream.io/v0/stream") as ws:
        sub = {
            "APIKey": API_KEY,
            "BoundingBoxes": [[[-90, -180], [90, 180]]],
            "FilterMessageTypes": ["PositionReport", "ShipStaticData"]
        }
        await ws.send(json.dumps(sub))
        
        count = 0
        async for msg in ws:
            try:
                data = json.loads(msg)
                if "error" in data:
                    print(f"AISStream error: {data['error']}")
                    continue
                
                msg_type = data.get("MessageType", "")
                meta = data.get("MetaData", {})
                message = data.get("Message", {})
                
                mmsi = str(meta.get("MMSI", ""))
                name = meta.get("ShipName", "").strip()
                
                if msg_type in ("PositionReport", "StandardClassBPositionReport"):
                    report = message.get(msg_type, {})
                    lat = report.get("Latitude", 0)
                    lon = report.get("Longitude", 0)
                    if abs(lat) > 90 or abs(lon) > 180:
                        continue
                    
                    entity = {
                        "mmsi": mmsi,
                        "name": name,
                        "lat": lat,
                        "lon": lon,
                        "speed": report.get("Sog", 0),
                        "course": report.get("Cog", 0),
                        "heading": report.get("TrueHeading", 511),
                    }
                    
                    req = urllib.request.Request(
                        f"{ORP_URL}/api/v1/ingest",
                        data=json.dumps(entity).encode(),
                        headers={"Content-Type": "application/json"},
                        method="POST"
                    )
                    try:
                        urllib.request.urlopen(req, timeout=2)
                        count += 1
                        if count % 50 == 0:
                            print(f"  Ingested {count} ships...")
                    except Exception as e:
                        pass  # ORP might be slow, skip
                        
            except Exception:
                continue

asyncio.run(bridge())
