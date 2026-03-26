"""
ORP Type definitions — Python 3.8+ compatible TypedDicts.
"""
from __future__ import annotations

from typing import Any, Dict, List, Optional
try:
    from typing import TypedDict
except ImportError:  # Python 3.7 fallback
    from typing_extensions import TypedDict  # type: ignore


class Location(TypedDict, total=False):
    lat: float
    lon: float
    alt: float


class Entity(TypedDict, total=False):
    id: str
    type: str
    name: str
    location: Location
    properties: Dict[str, Any]
    created_at: str
    updated_at: str


class QueryResult(TypedDict, total=False):
    entities: List[Entity]
    count: int
    cursor: Optional[str]


class HealthStatus(TypedDict, total=False):
    status: str
    version: str
    uptime: float
    peers: int


class Connector(TypedDict, total=False):
    id: str
    name: str
    type: str
    status: str
    config: Dict[str, Any]


class Peer(TypedDict, total=False):
    id: str
    host: str
    port: int
    status: str
    latency_ms: float


class IngestResult(TypedDict, total=False):
    id: str
    status: str
    created: bool


class BatchIngestResult(TypedDict, total=False):
    inserted: int
    failed: int
    ids: List[str]
    errors: List[Dict[str, Any]]


class NearFilter(TypedDict, total=False):
    lat: float
    lon: float
    radius_m: float
