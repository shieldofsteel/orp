/**
 * MapView — Military-grade Common Operating Picture (COP)
 * World-class Leaflet implementation with full situational awareness features.
 */

import React, {
  useMemo,
  useCallback,
  useEffect,
  useRef,
  useState,
} from 'react';
import L from 'leaflet';
import {
  MapContainer,
  TileLayer,
  CircleMarker,
  Polygon,
  Polyline,
  Tooltip,
  Marker,
  Rectangle,
  useMap,
  useMapEvents,
  ScaleControl,
} from 'react-leaflet';
import type { LeafletMouseEvent } from 'leaflet';
import 'leaflet/dist/leaflet.css';
import { useAppStore } from '../store/useAppStore';
import type { Entity } from '../types';
import {
  MapControls,
  type TileLayerType,
  type LayerVisibility,
} from './MapControls';

// ── Tile layer configs ────────────────────────────────────────────────────────

interface TileConfig {
  url: string;
  attribution: string;
  subdomains?: string;
  opacity: number;
  className?: string;
  maxZoom?: number;
  // Whether to apply a dark CSS filter for military aesthetic
  darkFilter: boolean;
}

const TILE_CONFIGS: Record<TileLayerType, TileConfig> = {
  osm: {
    url: 'https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png',
    attribution: '© OpenStreetMap contributors',
    subdomains: 'abc',
    opacity: 1,
    darkFilter: true,
    maxZoom: 19,
  },
  satellite: {
    url: 'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}',
    attribution: '© Esri, Maxar, Earthstar Geographics',
    opacity: 1,
    darkFilter: false,
    maxZoom: 19,
  },
  dark: {
    url: 'https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}{r}.png',
    attribution: '© OpenStreetMap contributors, © CARTO',
    subdomains: 'abcd',
    opacity: 1,
    darkFilter: false,
    maxZoom: 19,
  },
  topo: {
    url: 'https://server.arcgisonline.com/ArcGIS/rest/services/World_Topo_Map/MapServer/tile/{z}/{y}/{x}',
    attribution: '© Esri, HERE, Garmin, OpenStreetMap',
    opacity: 1,
    darkFilter: true,
    maxZoom: 19,
  },
};

// ── Color helpers ─────────────────────────────────────────────────────────────

function shipColor(speed: unknown, selected: boolean): string {
  if (selected) return '#ff3232';
  const s = typeof speed === 'number' && isFinite(speed) ? speed : 0;
  if (s > 20) return '#ff7800';
  if (s >= 10) return '#1e8cff';
  return '#3cc85a';
}

function weatherFill(severity: string): string {
  switch (severity?.toLowerCase()) {
    case 'extreme': return 'rgba(220,20,20,0.30)';
    case 'high':    return 'rgba(255,90,0,0.25)';
    case 'moderate':return 'rgba(255,200,0,0.20)';
    default:        return 'rgba(100,160,255,0.14)';
  }
}

function weatherStroke(severity: string): string {
  switch (severity?.toLowerCase()) {
    case 'extreme': return '#ff1e1e';
    case 'high':    return '#ff7800';
    case 'moderate':return '#ffdc1e';
    default:        return '#78b4ff';
  }
}

// ── Geometry helpers ──────────────────────────────────────────────────────────

function getLatLng(d: Entity): [number, number] | null {
  if (
    d.geometry?.type === 'Point' &&
    Array.isArray(d.geometry.coordinates) &&
    d.geometry.coordinates.length >= 2
  ) {
    const [lon, lat] = d.geometry.coordinates as number[];
    if (
      typeof lon === 'number' &&
      typeof lat === 'number' &&
      isFinite(lon) &&
      isFinite(lat) &&
      !(lon === 0 && lat === 0) &&
      lat >= -90 && lat <= 90 &&
      lon >= -180 && lon <= 180
    ) {
      return [lat, lon];
    }
  }
  return null;
}

function hasValidGeometry(e: Entity): boolean {
  if (!e.geometry) return false;
  if (e.geometry.type === 'Point') return getLatLng(e) !== null;
  if (e.geometry.type === 'Polygon' && Array.isArray(e.geometry.coordinates)) return true;
  return false;
}

function getWeatherLatLngs(d: Entity): [number, number][] {
  if (d.geometry?.type === 'Polygon' && Array.isArray(d.geometry.coordinates)) {
    const ring = (d.geometry.coordinates as number[][][])[0];
    if (Array.isArray(ring)) return ring.map(([lon, lat]) => [lat, lon] as [number, number]);
  }
  if (
    d.geometry?.type === 'Point' &&
    Array.isArray(d.geometry.coordinates) &&
    d.geometry.coordinates.length >= 2
  ) {
    const [lon, lat] = d.geometry.coordinates as number[];
    if (typeof lon === 'number' && typeof lat === 'number' && isFinite(lon) && isFinite(lat)) {
      const r = ((d.properties?.radius_km as number) ?? 100) / 111;
      return Array.from({ length: 33 }, (_, i) => {
        const angle = (i / 32) * 2 * Math.PI;
        return [lat + r * 0.6 * Math.sin(angle), lon + r * Math.cos(angle)] as [number, number];
      });
    }
  }
  return [];
}

// ── Course vector projection ──────────────────────────────────────────────────

/**
 * Project position forward 30 minutes at given speed (knots) and course (degrees).
 * Returns [lat, lon] of projected point.
 */
function projectPosition(
  lat: number,
  lon: number,
  courseDegs: number,
  speedKnots: number,
): [number, number] {
  const distNm = speedKnots * 0.5; // 30 min
  const courseRad = (courseDegs * Math.PI) / 180;
  const dLat = (distNm * Math.cos(courseRad)) / 60;
  const dLon = (distNm * Math.sin(courseRad)) / (60 * Math.cos((lat * Math.PI) / 180));
  return [lat + dLat, lon + dLon];
}

// ── Ship DivIcon factory ──────────────────────────────────────────────────────

function createShipIcon(
  course: number,
  color: string,
  selected: boolean,
): L.DivIcon {
  const size = selected ? 22 : 16;
  const stroke = selected ? 'white' : 'rgba(0,0,0,0.6)';
  const strokeW = selected ? 2 : 1;
  const glow = selected ? `filter: drop-shadow(0 0 4px ${color});` : '';
  return L.divIcon({
    html: `<svg
      width="${size}" height="${size}"
      viewBox="0 0 20 24"
      xmlns="http://www.w3.org/2000/svg"
      style="transform: rotate(${course}deg); transform-origin: 50% 50%; ${glow}"
    >
      <polygon
        points="10,1 18,23 10,17 2,23"
        fill="${color}"
        stroke="${stroke}"
        stroke-width="${strokeW}"
        stroke-linejoin="round"
      />
    </svg>`,
    className: '',
    iconSize: [size, size],
    iconAnchor: [size / 2, size / 2],
  });
}

// ── Measurement point icon ────────────────────────────────────────────────────

function createMeasureIcon(label: string): L.DivIcon {
  return L.divIcon({
    html: `<div style="
      width:10px;height:10px;
      background:#facc15;
      border:2px solid #fff;
      border-radius:50%;
      position:relative;
    "><span style="
      position:absolute;left:12px;top:-2px;
      color:#facc15;font-size:9px;
      font-family:monospace;white-space:nowrap;
      text-shadow:0 1px 3px rgba(0,0,0,0.9);
    ">${label}</span></div>`,
    className: '',
    iconSize: [10, 10],
    iconAnchor: [5, 5],
  });
}

// ── Tooltip content ───────────────────────────────────────────────────────────

function tooltipHTML(entity: Entity): string {
  const name = entity.name ?? entity.id;
  const type = entity.type ?? 'Unknown';
  const props = entity.properties ?? {};
  const conf = typeof entity.confidence === 'number'
    ? `${(entity.confidence * 100).toFixed(0)}%`
    : '?';

  const rows: string[] = [];

  if (entity.type?.toLowerCase() === 'ship') {
    const speed = typeof props.speed === 'number' ? `${props.speed.toFixed(1)} kn` : '—';
    const course = typeof props.course === 'number' ? `${Math.round(props.course)}°` : '—';
    rows.push(`Speed: ${speed}`, `Course: ${course}`);
  } else if (entity.type?.toLowerCase() === 'port') {
    const teu = typeof props.total_teu === 'number' ? props.total_teu.toLocaleString() : '—';
    const cong = typeof props.congestion === 'number' ? `${(props.congestion * 100).toFixed(0)}%` : '—';
    rows.push(`TEU: ${teu}`, `Congestion: ${cong}`);
  } else if (entity.type?.toLowerCase() === 'weathersystem') {
    rows.push(`Severity: ${(props.severity as string) ?? '—'}`);
  }
  rows.push(`Confidence: ${conf}`);

  return `
    <div style="
      font-family: 'JetBrains Mono','Fira Code',monospace;
      font-size: 10px;
      line-height: 1.4;
      min-width: 130px;
    ">
      <div style="font-weight:700;color:#e5e7eb;margin-bottom:3px;font-size:11px">${name}</div>
      <div style="color:#6b7280;margin-bottom:4px;font-size:9px;text-transform:uppercase;letter-spacing:.08em">${type}</div>
      ${rows.map(r => `<div style="color:#9ca3af">${r}</div>`).join('')}
    </div>
  `;
}

// ── Grid layer component ──────────────────────────────────────────────────────

function LatLonGrid() {
  const map = useMap();
  const [lines, setLines] = useState<Array<{ positions: [number, number][]; label: string; isLat: boolean }>>([]);

  useEffect(() => {
    const update = () => {
      const bounds = map.getBounds();
      const zoom = map.getZoom();

      // Adaptive step based on zoom
      let step = 10;
      if (zoom >= 8)  step = 1;
      if (zoom >= 10) step = 0.5;
      if (zoom >= 12) step = 0.25;
      if (zoom >= 14) step = 0.1;

      const newLines: typeof lines = [];

      // Lat lines
      const minLat = Math.floor(bounds.getSouth() / step) * step;
      const maxLat = Math.ceil(bounds.getNorth() / step) * step;
      for (let lat = minLat; lat <= maxLat; lat = Math.round((lat + step) * 1e6) / 1e6) {
        newLines.push({
          positions: [[lat, bounds.getWest()], [lat, bounds.getEast()]],
          label: `${Math.abs(lat).toFixed(step < 1 ? 2 : 0)}°${lat >= 0 ? 'N' : 'S'}`,
          isLat: true,
        });
      }

      // Lon lines
      const minLon = Math.floor(bounds.getWest() / step) * step;
      const maxLon = Math.ceil(bounds.getEast() / step) * step;
      for (let lon = minLon; lon <= maxLon; lon = Math.round((lon + step) * 1e6) / 1e6) {
        newLines.push({
          positions: [[bounds.getSouth(), lon], [bounds.getNorth(), lon]],
          label: `${Math.abs(lon).toFixed(step < 1 ? 2 : 0)}°${lon >= 0 ? 'E' : 'W'}`,
          isLat: false,
        });
      }
      setLines(newLines);
    };

    update();
    map.on('moveend zoomend', update);
    return () => { map.off('moveend zoomend', update); };
  }, [map]);

  return (
    <>
      {lines.map((line, i) => (
        <Polyline
          key={i}
          positions={line.positions}
          pathOptions={{ color: '#334155', weight: 0.5, opacity: 0.5, dashArray: '2 4' }}
        />
      ))}
    </>
  );
}

// ── Coordinate tracker (inside MapContainer context) ──────────────────────────

function CoordTracker({
  onCoords,
}: {
  onCoords: (coords: [number, number] | null) => void;
}) {
  useMapEvents({
    mousemove(e) { onCoords([e.latlng.lat, e.latlng.lng]); },
    mouseout()   { onCoords(null); },
  });
  return null;
}

// ── Lasso/box selection handler ───────────────────────────────────────────────

interface LassoHandlerProps {
  active: boolean;
  onSelect: (bounds: L.LatLngBounds) => void;
}

function LassoHandler({ active, onSelect }: LassoHandlerProps) {
  const map = useMap();
  const startRef = useRef<L.LatLng | null>(null);
  const draggingRef = useRef(false);
  const [selectionBounds, setSelectionBounds] = useState<L.LatLngBounds | null>(null);

  useEffect(() => {
    if (!active) {
      setSelectionBounds(null);
      return;
    }

    const onMouseDown = (e: L.LeafletMouseEvent) => {
      if (!e.originalEvent.shiftKey) return;
      e.originalEvent.preventDefault();
      map.dragging.disable();
      startRef.current = e.latlng;
      draggingRef.current = true;
    };

    const onMouseMove = (e: L.LeafletMouseEvent) => {
      if (!draggingRef.current || !startRef.current) return;
      setSelectionBounds(L.latLngBounds(startRef.current, e.latlng));
    };

    const onMouseUp = (e: L.LeafletMouseEvent) => {
      if (!draggingRef.current || !startRef.current) return;
      draggingRef.current = false;
      map.dragging.enable();
      const bounds = L.latLngBounds(startRef.current, e.latlng);
      onSelect(bounds);
      setSelectionBounds(null);
      startRef.current = null;
    };

    map.on('mousedown', onMouseDown as L.LeafletEventHandlerFn);
    map.on('mousemove', onMouseMove as L.LeafletEventHandlerFn);
    map.on('mouseup', onMouseUp as L.LeafletEventHandlerFn);

    return () => {
      map.off('mousedown', onMouseDown as L.LeafletEventHandlerFn);
      map.off('mousemove', onMouseMove as L.LeafletEventHandlerFn);
      map.off('mouseup', onMouseUp as L.LeafletEventHandlerFn);
      map.dragging.enable();
      setSelectionBounds(null);
    };
  }, [active, map, onSelect]);

  if (!selectionBounds) return null;
  return (
    <Rectangle
      bounds={selectionBounds}
      pathOptions={{
        color: '#00e5ff',
        weight: 1.5,
        fillColor: '#00e5ff',
        fillOpacity: 0.05,
        dashArray: '4 3',
      }}
    />
  );
}

// ── Measurement tool handler ──────────────────────────────────────────────────

function MeasurementHandler({ active }: { active: boolean }) {
  const [points, setPoints] = useState<L.LatLng[]>([]);

  useMapEvents({
    click(e) {
      if (!active) return;
      e.originalEvent.stopPropagation();
      setPoints((prev) => {
        if (prev.length >= 2) return [e.latlng];
        return [...prev, e.latlng];
      });
    },
  });

  useEffect(() => {
    if (!active) setPoints([]);
  }, [active]);

  if (!active || points.length === 0) return null;

  const distM = points.length === 2 ? points[0].distanceTo(points[1]) : null;
  const distKm = distM !== null ? (distM / 1000).toFixed(2) : null;
  const distNm = distM !== null ? (distM / 1852).toFixed(2) : null;

  const midLabel = distKm !== null ? `${distKm} km / ${distNm} nm` : 'Click 2nd point';

  return (
    <>
      {points.map((pt, i) => (
        <Marker
          key={i}
          position={pt}
          icon={createMeasureIcon(i === 0 ? 'A' : 'B')}
        />
      ))}
      {points.length === 2 && (
        <>
          <Polyline
            positions={[points[0], points[1]]}
            pathOptions={{ color: '#facc15', weight: 2, dashArray: '6 4' }}
          />
          {/* Mid-point label */}
          <Marker
            position={[
              (points[0].lat + points[1].lat) / 2,
              (points[0].lng + points[1].lng) / 2,
            ]}
            icon={L.divIcon({
              html: `<div style="
                background:rgba(0,0,0,0.85);
                border:1px solid #facc15;
                color:#facc15;
                font-family:monospace;
                font-size:10px;
                padding:2px 6px;
                white-space:nowrap;
                transform:translateX(-50%);
              ">${midLabel}</div>`,
              className: '',
              iconAnchor: [0, 0],
            })}
          />
        </>
      )}
    </>
  );
}

// ── Map camera controller ─────────────────────────────────────────────────────

function MapController({
  zoomToFitTrigger,
  allEntities,
  mapRef,
  zoomToPos,
}: {
  zoomToFitTrigger: number;
  allEntities: Entity[];
  mapRef: React.MutableRefObject<L.Map | null>;
  zoomToPos: [number, number] | null;
}) {
  const map = useMap();

  useEffect(() => {
    mapRef.current = map;
  }, [map, mapRef]);

  useEffect(() => {
    if (zoomToFitTrigger === 0) return;
    const points: L.LatLng[] = [];
    for (const e of allEntities) {
      const pos = getLatLng(e);
      if (pos) points.push(L.latLng(pos[0], pos[1]));
    }
    if (points.length === 0) return;
    const bounds = L.latLngBounds(points);
    map.fitBounds(bounds, { padding: [40, 40], maxZoom: 12 });
  }, [zoomToFitTrigger, map, allEntities]);

  useEffect(() => {
    if (!zoomToPos) return;
    map.setView(zoomToPos, Math.max(map.getZoom(), 13), { animate: true });
  }, [zoomToPos, map]);

  return null;
}

// ── CSS injection for tile layer dark filter ──────────────────────────────────

const DARK_FILTER_STYLE = `
  .leaflet-tile-pane.cop-dark-filter img {
    filter: brightness(0.5) saturate(0.4) hue-rotate(190deg);
  }
`;

function InjectStyle() {
  useEffect(() => {
    const el = document.createElement('style');
    el.textContent = DARK_FILTER_STYLE;
    document.head.appendChild(el);
    return () => el.remove();
  }, []);
  return null;
}

// ── Main Component ────────────────────────────────────────────────────────────

export const MapView: React.FC = () => {
  // ── Store ──
  const entities         = useAppStore((s) => s.entities);
  const selectedEntityId = useAppStore((s) => s.selectedEntityId);
  const selectedEntities = useAppStore((s) => s.selectedEntities);
  const showWeatherLayer = useAppStore((s) => s.showWeatherLayer);
  const showShipTracksLayer = useAppStore((s) => s.showShipTracksLayer);
  const selectEntity     = useAppStore((s) => s.selectEntity);
  const toggleEntitySelection = useAppStore((s) => s.toggleEntitySelection);

  // ── Local state ──
  const [activeTile, setActiveTile] = useState<TileLayerType>('osm');
  const [layers, setLayers] = useState<LayerVisibility>({
    ships:   true,
    ports:   true,
    weather: true,
    tracks:  true,
    vectors: true,
    grid:    false,
  });
  const [measureActive, setMeasureActive] = useState(false);
  const [lassoActive, setLassoActive]     = useState(false);
  const [mouseCoords, setMouseCoords]     = useState<[number, number] | null>(null);
  const [zoomToFitTrigger, setZoomToFitTrigger] = useState(0);
  const [zoomToPos, setZoomToPos]         = useState<[number, number] | null>(null);
  const mapRef = useRef<L.Map | null>(null);

  // ── Derived data ──
  const allEntities = useMemo(() => Array.from(entities.values()), [entities]);

  const ships = useMemo(
    () => allEntities.filter((e) => e.type?.toLowerCase() === 'ship' && hasValidGeometry(e)),
    [allEntities],
  );
  const ports = useMemo(
    () => allEntities.filter((e) => e.type?.toLowerCase() === 'port' && hasValidGeometry(e)),
    [allEntities],
  );
  const weatherSystems = useMemo(
    () => allEntities.filter((e) => e.type?.toLowerCase() === 'weathersystem' && hasValidGeometry(e)),
    [allEntities],
  );

  // ── Handlers ──
  const handleToggleLayer = useCallback((layer: keyof LayerVisibility) => {
    setLayers((prev) => ({ ...prev, [layer]: !prev[layer] }));
  }, []);

  const handleEntityClick = useCallback(
    (id: string) => (e: L.LeafletEvent) => {
      (e as L.LeafletMouseEvent).originalEvent?.stopPropagation();
      selectEntity(id);
    },
    [selectEntity],
  );

  const handleEntityDblClick = useCallback(
    (pos: [number, number]) => (e: L.LeafletEvent) => {
      (e as L.LeafletMouseEvent).originalEvent?.stopPropagation();
      setZoomToPos(pos);
    },
    [],
  );

  const handleLassoSelect = useCallback(
    (bounds: L.LatLngBounds) => {
      const inBounds = allEntities.filter((e) => {
        const pos = getLatLng(e);
        if (!pos) return false;
        return bounds.contains(L.latLng(pos[0], pos[1]));
      });
      inBounds.forEach((e) => toggleEntitySelection(e.id));
    },
    [allEntities, toggleEntitySelection],
  );

  const tileConfig = TILE_CONFIGS[activeTile];
  const tileLayerClass = tileConfig.darkFilter ? 'cop-dark-filter' : '';

  // Sync local layer state with store for weather/tracks
  useEffect(() => {
    // Keep store in sync when toggled via MapControls
    // (store has its own toggles; for simplicity we use local state as source of truth for these)
  }, []);

  // ── Track segments with fading opacity ──
  const renderTrack = useCallback((ship: Entity, isSelected: boolean) => {
    const history = ship.history ?? [];
    const trackPositions = history
      .slice(-50)
      .filter((h) => {
        const c = h.geometry?.coordinates;
        return (
          Array.isArray(c) &&
          c.length >= 2 &&
          typeof c[0] === 'number' &&
          typeof c[1] === 'number' &&
          isFinite(c[0]) &&
          isFinite(c[1])
        );
      })
      .map((h) => {
        const [lon, lat] = h.geometry!.coordinates as number[];
        return [lat, lon] as [number, number];
      });

    if (trackPositions.length < 2) return null;

    // Split into 5 segments for fading effect
    const segCount = 5;
    const segSize = Math.ceil(trackPositions.length / segCount);
    const segments: Array<{ positions: [number, number][]; opacity: number }> = [];

    for (let i = 0; i < segCount; i++) {
      const start = i * segSize;
      const end = Math.min(start + segSize + 1, trackPositions.length);
      const pts = trackPositions.slice(start, end);
      if (pts.length < 2) continue;
      segments.push({
        positions: pts,
        opacity: isSelected ? 0.15 + i * 0.17 : 0.05 + i * 0.06,
      });
    }

    return segments.map((seg, segIdx) => (
      <Polyline
        key={`track-${ship.id}-seg${segIdx}`}
        positions={seg.positions}
        pathOptions={{
          color: isSelected ? '#ffc832' : '#64a0dc',
          weight: isSelected ? 2 : 1,
          opacity: seg.opacity,
        }}
      />
    ));
  }, []);

  return (
    <div
      className="relative flex-1 overflow-hidden"
      role="application"
      aria-label="Military Common Operating Picture map"
    >
      {/* Inject dark filter CSS once */}
      <style>{DARK_FILTER_STYLE}</style>

      <MapContainer
        center={[51.92, 4.27]}
        zoom={10}
        className="h-full w-full"
        style={{ background: '#060b14' }}
        zoomControl={false}
        attributionControl={false}
      >
        {/* Tile layer — keyed to force remount on change */}
        <TileLayer
          key={activeTile}
          url={tileConfig.url}
          attribution={tileConfig.attribution}
          subdomains={tileConfig.subdomains as string | undefined}
          opacity={tileConfig.opacity}
          maxZoom={tileConfig.maxZoom}
          className={tileLayerClass}
        />

        {/* Scale control */}
        <ScaleControl position="bottomleft" imperial metric />

        {/* Internal utility components */}
        <CoordTracker onCoords={setMouseCoords} />
        <MapController
          zoomToFitTrigger={zoomToFitTrigger}
          allEntities={allEntities}
          mapRef={mapRef}
          zoomToPos={zoomToPos}
        />

        {/* Grid */}
        {layers.grid && <LatLonGrid />}

        {/* Measurement tool */}
        <MeasurementHandler active={measureActive} />

        {/* Lasso/box selection */}
        <LassoHandler active={lassoActive} onSelect={handleLassoSelect} />

        {/* ── Weather layer ────────────────────────────────────────────── */}
        {layers.weather &&
          weatherSystems.map((w) => {
            const positions = getWeatherLatLngs(w);
            if (positions.length === 0) return null;
            const severity = (w.properties?.severity as string) ?? '';
            const isSelected = w.id === selectedEntityId;
            return (
              <Polygon
                key={w.id}
                positions={positions}
                pathOptions={{
                  fillColor: weatherFill(severity),
                  fillOpacity: 1,
                  color: isSelected ? '#ffffff' : weatherStroke(severity),
                  weight: isSelected ? 2.5 : 1.5,
                }}
                eventHandlers={{
                  click: handleEntityClick(w.id),
                }}
              >
                <Tooltip
                  direction="top"
                  sticky
                  className="cop-tooltip"
                  opacity={1}
                >
                  <div dangerouslySetInnerHTML={{ __html: tooltipHTML(w) }} />
                </Tooltip>
              </Polygon>
            );
          })}

        {/* ── Ship tracks ──────────────────────────────────────────────── */}
        {layers.tracks &&
          ships.map((ship) => {
            const isSelected = ship.id === selectedEntityId;
            return renderTrack(ship, isSelected);
          })}

        {/* ── Course/speed vectors ─────────────────────────────────────── */}
        {layers.vectors &&
          ships.map((ship) => {
            const pos = getLatLng(ship);
            if (!pos) return null;
            const speed  = typeof ship.properties?.speed  === 'number' ? ship.properties.speed  : 0;
            const course = typeof ship.properties?.course === 'number' ? ship.properties.course : 0;
            if (speed < 0.5) return null; // don't draw vectors for stationary ships

            const projected = projectPosition(pos[0], pos[1], course, speed);
            const isSelected = ship.id === selectedEntityId;
            return (
              <Polyline
                key={`vec-${ship.id}`}
                positions={[pos, projected]}
                pathOptions={{
                  color: isSelected ? '#ffffff' : shipColor(speed, false),
                  weight: isSelected ? 2 : 1,
                  opacity: isSelected ? 0.9 : 0.55,
                  dashArray: '6 3',
                }}
              />
            );
          })}

        {/* ── Port markers ─────────────────────────────────────────────── */}
        {layers.ports &&
          ports.map((port) => {
            const pos = getLatLng(port);
            if (!pos) return null;
            const isSelected = port.id === selectedEntityId;
            const isInMultiSelect = selectedEntities.has(port.id);
            // Size based on TEU
            const teu = typeof port.properties?.total_teu === 'number' ? port.properties.total_teu : 0;
            const radius = isSelected
              ? 14
              : Math.max(6, Math.min(14, 4 + Math.log10(teu + 1) * 2));
            return (
              <CircleMarker
                key={port.id}
                center={pos}
                radius={radius}
                pathOptions={{
                  fillColor: '#f97316',
                  fillOpacity: isSelected ? 0.95 : 0.75,
                  color: isSelected || isInMultiSelect ? '#ffffff' : 'rgba(255,255,255,0.35)',
                  weight: isSelected ? 2.5 : isInMultiSelect ? 2 : 1,
                }}
                eventHandlers={{
                  click: handleEntityClick(port.id),
                  dblclick: (e) => handleEntityDblClick(pos)(e),
                }}
              >
                <Tooltip
                  direction="top"
                  sticky
                  opacity={1}
                  className="cop-tooltip"
                >
                  <div dangerouslySetInnerHTML={{ __html: tooltipHTML(port) }} />
                </Tooltip>
              </CircleMarker>
            );
          })}

        {/* ── Ship markers (directional arrows) ───────────────────────── */}
        {layers.ships &&
          ships.map((ship) => {
            const pos = getLatLng(ship);
            if (!pos) return null;
            const isSelected = ship.id === selectedEntityId;
            const isInMultiSelect = selectedEntities.has(ship.id);
            const speed  = ship.properties?.speed;
            const course = typeof ship.properties?.course === 'number' ? ship.properties.course : 0;
            const color  = shipColor(speed, isSelected);

            return (
              <Marker
                key={ship.id}
                position={pos}
                icon={createShipIcon(course, color, isSelected)}
                zIndexOffset={isSelected ? 1000 : isInMultiSelect ? 500 : 0}
                eventHandlers={{
                  click: handleEntityClick(ship.id),
                  dblclick: (e) => {
                    (e as L.LeafletMouseEvent).originalEvent?.stopPropagation();
                    setZoomToPos(pos);
                  },
                }}
              >
                <Tooltip
                  direction="top"
                  sticky
                  offset={[0, -8]}
                  opacity={1}
                  className="cop-tooltip"
                >
                  <div dangerouslySetInnerHTML={{ __html: tooltipHTML(ship) }} />
                </Tooltip>
              </Marker>
            );
          })}
      </MapContainer>

      {/* ── MapControls overlay ────────────────────────────────────────── */}
      <MapControls
        activeTile={activeTile}
        onTileChange={setActiveTile}
        layers={layers}
        onToggleLayer={handleToggleLayer}
        measureActive={measureActive}
        lassoActive={lassoActive}
        onToggleMeasure={() => { setMeasureActive((v) => !v); if (lassoActive) setLassoActive(false); }}
        onToggleLasso={() => { setLassoActive((v) => !v); if (measureActive) setMeasureActive(false); }}
        onZoomToFit={() => setZoomToFitTrigger((n) => n + 1)}
        mouseCoords={mouseCoords}
        entityCount={{ ships: ships.length, ports: ports.length, weather: weatherSystems.length }}
      />

      {/* ── Bottom-left legend ──────────────────────────────────────────── */}
      <div
        className="absolute bottom-8 left-2 z-[1000] bg-gray-950/90 border border-gray-700/70 p-2 text-[9px]"
        style={{
          fontFamily: "'JetBrains Mono','Fira Code',monospace",
          backdropFilter: 'blur(8px)',
        }}
        aria-label="Map legend"
      >
        <div className="text-gray-500 font-bold tracking-widest uppercase text-[8px] mb-1.5">
          Ships
        </div>
        <div className="space-y-0.5">
          {[
            { color: '#3cc85a', label: '< 10 kn' },
            { color: '#1e8cff', label: '10–20 kn' },
            { color: '#ff7800', label: '> 20 kn' },
            { color: '#ff3232', label: 'Selected' },
          ].map(({ color, label }) => (
            <div key={label} className="flex items-center gap-1.5">
              <svg width="10" height="12" viewBox="0 0 10 14" style={{ color }} fill="currentColor">
                <polygon points="5,0 9,14 5,10 1,14" />
              </svg>
              <span className="text-gray-500">{label}</span>
            </div>
          ))}
        </div>
        <div className="border-t border-gray-800 my-1.5" />
        <div className="text-gray-500 font-bold tracking-widest uppercase text-[8px] mb-1.5">
          Ports
        </div>
        <div className="space-y-0.5">
          {[
            { label: 'Size = TEU volume' },
            { label: 'Orange fill' },
          ].map(({ label }) => (
            <div key={label} className="flex items-center gap-1.5 text-gray-600">
              <span className="w-2.5 h-2.5 border border-orange-500/60" style={{ background: '#f9731620' }} />
              <span>{label}</span>
            </div>
          ))}
        </div>
        {(measureActive || lassoActive) && (
          <>
            <div className="border-t border-gray-800 my-1.5" />
            {measureActive && (
              <div className="text-yellow-500/80 text-[8px]">● Click two points to measure</div>
            )}
            {lassoActive && (
              <div className="text-cyan-500/80 text-[8px]">● Shift+drag to select area</div>
            )}
          </>
        )}
      </div>

      {/* ── Tooltip styles ──────────────────────────────────────────────── */}
      <style>{`
        .cop-tooltip {
          background: rgba(5, 8, 16, 0.96) !important;
          border: 1px solid rgba(75, 85, 99, 0.8) !important;
          border-radius: 0 !important;
          padding: 6px 8px !important;
          box-shadow: 0 4px 16px rgba(0,0,0,0.6) !important;
        }
        .cop-tooltip::before {
          display: none !important;
        }
        .leaflet-tooltip-top.cop-tooltip::before {
          display: none !important;
        }
        .cop-dark-filter .leaflet-tile {
          filter: brightness(0.5) saturate(0.4) hue-rotate(190deg) !important;
        }
        /* Custom zoom control styling */
        .leaflet-control-zoom a {
          background: rgba(5, 8, 16, 0.92) !important;
          border-color: rgba(55, 65, 81, 0.8) !important;
          border-radius: 0 !important;
          color: #9ca3af !important;
        }
        .leaflet-control-zoom a:hover {
          background: rgba(17, 24, 39, 0.95) !important;
          color: #e5e7eb !important;
        }
        .leaflet-control-scale-line {
          background: rgba(5, 8, 16, 0.85) !important;
          border-color: rgba(75, 85, 99, 0.7) !important;
          border-radius: 0 !important;
          color: #6b7280 !important;
          font-family: 'JetBrains Mono', monospace !important;
          font-size: 9px !important;
          padding: 1px 4px !important;
        }
      `}</style>
    </div>
  );
};
