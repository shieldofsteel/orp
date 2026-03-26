"""
ORP Python SDK — Object Relationship Platform client.

Zero required dependencies. Python 3.8+.

Quick start:
    from orp import ORPClient
    client = ORPClient(host="localhost", port=9090, token="my-token")
    ships = client.entities(type="ship")
"""
from __future__ import annotations

import json
import threading
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Callable, Dict, List, Optional

from .types import (
    BatchIngestResult,
    Connector,
    Entity,
    HealthStatus,
    IngestResult,
    Peer,
    QueryResult,
)

__version__ = "0.1.0"
__all__ = ["ORPClient", "ORPError", "ORPAuthError", "ORPNotFoundError"]


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------

class ORPError(Exception):
    """Base ORP error."""
    def __init__(self, message: str, status_code: int = 0, body: Any = None):
        super().__init__(message)
        self.status_code = status_code
        self.body = body


class ORPAuthError(ORPError):
    """Raised on 401/403 responses."""


class ORPNotFoundError(ORPError):
    """Raised on 404 responses."""


# ---------------------------------------------------------------------------
# Client
# ---------------------------------------------------------------------------

class ORPClient:
    """
    Lightweight ORP HTTP client using only the standard library.

    Args:
        host:    ORP server hostname or IP (default: "localhost")
        port:    ORP server port (default: 9090)
        token:   Bearer token for authentication
        api_key: API key (sent as X-API-Key header)
        tls:     Use HTTPS instead of HTTP (default: False)
        timeout: Request timeout in seconds (default: 30)
    """

    def __init__(
        self,
        host: str = "localhost",
        port: int = 9090,
        token: Optional[str] = None,
        api_key: Optional[str] = None,
        tls: bool = False,
        timeout: int = 30,
    ) -> None:
        scheme = "https" if tls else "http"
        self._base_url = f"{scheme}://{host}:{port}"
        self._token = token
        self._api_key = api_key
        self._timeout = timeout

    # ------------------------------------------------------------------
    # Entities
    # ------------------------------------------------------------------

    def entities(
        self,
        type: Optional[str] = None,
        near: Optional[Dict[str, float]] = None,
        limit: int = 100,
    ) -> List[Entity]:
        """
        List entities, optionally filtered by type and/or proximity.

        Args:
            type:  Entity type filter (e.g. "ship", "vessel")
            near:  Dict with lat, lon, and optional radius_m
            limit: Maximum number of results (default: 100)

        Returns:
            List of Entity dicts.

        Example:
            ships = client.entities(type="ship", near={"lat": 1.3, "lon": 103.8, "radius_m": 5000})
        """
        params: Dict[str, Any] = {"limit": limit}
        if type:
            params["type"] = type
        if near:
            params["lat"] = near["lat"]
            params["lon"] = near["lon"]
            if "radius_m" in near:
                params["radius_m"] = near["radius_m"]
        data = self._get("/entities", params=params)
        # Accept both {"entities": [...]} and bare list
        if isinstance(data, list):
            return data
        return data.get("entities", data.get("data", []))

    def entity(self, id: str) -> Entity:
        """
        Fetch a single entity by ID.

        Args:
            id: The entity ID.

        Returns:
            Entity dict.
        """
        return self._get(f"/entities/{urllib.parse.quote(str(id), safe='')}")

    # ------------------------------------------------------------------
    # Search
    # ------------------------------------------------------------------

    def search(
        self,
        query: Optional[str] = None,
        type: Optional[str] = None,
        near: Optional[Dict[str, float]] = None,
    ) -> List[Entity]:
        """
        Full-text / semantic search across entities.

        Args:
            query: Search string
            type:  Restrict to entity type
            near:  Proximity filter (lat, lon, radius_m)

        Returns:
            List of matching Entity dicts.
        """
        params: Dict[str, Any] = {}
        if query:
            params["q"] = query
        if type:
            params["type"] = type
        if near:
            params["lat"] = near["lat"]
            params["lon"] = near["lon"]
            if "radius_m" in near:
                params["radius_m"] = near["radius_m"]
        data = self._get("/search", params=params)
        if isinstance(data, list):
            return data
        return data.get("results", data.get("entities", []))

    # ------------------------------------------------------------------
    # Query (ORPQL)
    # ------------------------------------------------------------------

    def query(self, orpql: str) -> List[Any]:
        """
        Execute an ORPQL query string.

        Args:
            orpql: The ORPQL query (e.g. "MATCH (s:ship) WHERE s.speed > 10 RETURN s")

        Returns:
            List of result rows.
        """
        data = self._post("/query", {"query": orpql})
        if isinstance(data, list):
            return data
        return data.get("results", data.get("rows", []))

    # ------------------------------------------------------------------
    # Ingest
    # ------------------------------------------------------------------

    def ingest(self, data: Dict[str, Any]) -> IngestResult:
        """
        Ingest a single entity record.

        Args:
            data: Entity dict to ingest (must include at minimum "type").

        Returns:
            IngestResult dict with id and status.
        """
        return self._post("/ingest", data)

    def ingest_batch(self, records: List[Dict[str, Any]]) -> BatchIngestResult:
        """
        Ingest multiple entity records in one request.

        Args:
            records: List of entity dicts.

        Returns:
            BatchIngestResult with inserted count, failed count, ids, and errors.
        """
        return self._post("/ingest/batch", {"records": records})

    # ------------------------------------------------------------------
    # System
    # ------------------------------------------------------------------

    def health(self) -> HealthStatus:
        """
        Check ORP server health.

        Returns:
            HealthStatus dict with status, version, uptime, peers.
        """
        return self._get("/health")

    def connectors(self) -> List[Connector]:
        """
        List configured data connectors.

        Returns:
            List of Connector dicts.
        """
        data = self._get("/connectors")
        if isinstance(data, list):
            return data
        return data.get("connectors", [])

    def peers(self) -> List[Peer]:
        """
        List connected ORP peers (federation/cluster).

        Returns:
            List of Peer dicts.
        """
        data = self._get("/peers")
        if isinstance(data, list):
            return data
        return data.get("peers", [])

    # ------------------------------------------------------------------
    # Real-time subscription (optional websocket-client dependency)
    # ------------------------------------------------------------------

    def subscribe(
        self,
        entity_type: str,
        callback: Callable[[Entity], None],
        on_error: Optional[Callable[[Exception], None]] = None,
    ) -> "Subscription":
        """
        Subscribe to real-time entity updates via WebSocket.

        Requires the `websocket-client` package::

            pip install websocket-client

        Args:
            entity_type: Entity type to subscribe to (e.g. "ship")
            callback:    Called with each incoming Entity dict
            on_error:    Optional error handler

        Returns:
            A Subscription object. Call .stop() to unsubscribe.

        Example:
            def on_ship(entity):
                print(entity["id"], entity.get("location"))

            sub = client.subscribe("ship", on_ship)
            # ... later ...
            sub.stop()
        """
        try:
            import websocket  # type: ignore
        except ImportError as exc:
            raise ImportError(
                "websocket-client is required for subscribe(). "
                "Install it with: pip install websocket-client"
            ) from exc

        scheme = "wss" if self._base_url.startswith("https") else "ws"
        host_part = self._base_url.split("://", 1)[1]
        ws_url = f"{scheme}://{host_part}/ws/subscribe/{urllib.parse.quote(entity_type, safe='')}"

        headers = self._auth_headers()

        def _on_message(ws: Any, message: str) -> None:
            try:
                entity = json.loads(message)
                callback(entity)
            except Exception as exc:
                if on_error:
                    on_error(exc)

        def _on_error(ws: Any, error: Exception) -> None:
            if on_error:
                on_error(error)

        ws = websocket.WebSocketApp(
            ws_url,
            header=headers,
            on_message=_on_message,
            on_error=_on_error,
        )

        thread = threading.Thread(
            target=ws.run_forever,
            kwargs={"ping_interval": 30, "ping_timeout": 10},
            daemon=True,
        )
        thread.start()

        return Subscription(ws, thread)

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _auth_headers(self) -> Dict[str, str]:
        headers: Dict[str, str] = {"Content-Type": "application/json"}
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
        if self._api_key:
            headers["X-API-Key"] = self._api_key
        return headers

    def _build_url(self, path: str, params: Optional[Dict[str, Any]] = None) -> str:
        url = self._base_url + path
        if params:
            query = urllib.parse.urlencode(
                {k: v for k, v in params.items() if v is not None}
            )
            url = f"{url}?{query}"
        return url

    def _get(self, path: str, params: Optional[Dict[str, Any]] = None) -> Any:
        url = self._build_url(path, params)
        req = urllib.request.Request(url, headers=self._auth_headers(), method="GET")
        return self._send(req)

    def _post(self, path: str, body: Any) -> Any:
        url = self._build_url(path)
        payload = json.dumps(body).encode("utf-8")
        req = urllib.request.Request(
            url, data=payload, headers=self._auth_headers(), method="POST"
        )
        return self._send(req)

    def _send(self, req: urllib.request.Request) -> Any:
        try:
            with urllib.request.urlopen(req, timeout=self._timeout) as resp:
                raw = resp.read()
                if not raw:
                    return {}
                return json.loads(raw)
        except urllib.error.HTTPError as exc:
            body = None
            try:
                body = json.loads(exc.read())
            except Exception:
                pass
            msg = f"ORP HTTP {exc.code}: {exc.reason}"
            if exc.code in (401, 403):
                raise ORPAuthError(msg, status_code=exc.code, body=body) from exc
            if exc.code == 404:
                raise ORPNotFoundError(msg, status_code=exc.code, body=body) from exc
            raise ORPError(msg, status_code=exc.code, body=body) from exc
        except urllib.error.URLError as exc:
            raise ORPError(f"ORP connection failed: {exc.reason}") from exc


# ---------------------------------------------------------------------------
# Subscription handle
# ---------------------------------------------------------------------------

class Subscription:
    """
    Returned by :meth:`ORPClient.subscribe`. Call :meth:`stop` to disconnect.
    """

    def __init__(self, ws: Any, thread: threading.Thread) -> None:
        self._ws = ws
        self._thread = thread

    def stop(self) -> None:
        """Close the WebSocket connection and stop the background thread."""
        self._ws.close()

    def is_alive(self) -> bool:
        """Return True if the background thread is still running."""
        return self._thread.is_alive()
