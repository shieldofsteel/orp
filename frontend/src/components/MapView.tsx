/**
 * MapView — Dynamic Common Operating Picture (COP)
 * All entity type rendering driven by EntityTypeRegistry.
 * Zero hardcoded type names — add any entity type, it renders automatically.
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
import 'leaflet/dist/leaflet.css';
import { useAppStore } from '../store/useAppStore';
import type { Entity, EntityTypeConfig } from '../types';
import { useEntityTypes, getEntityTypeConfig, groupEntitiesByType } from '../hooks/useEntityTypes';
import { MapControls, type TileLayerType, type LayerVisibility } from './MapControls';
import { MapLegend } from './MapLegend';

// ── Tile layer configs ────────────────────────────────────────────────────────

interface TileConfig {
  url: string;
  attribution: string;
  subdomains?: string;
  opacity: number;
  maxZoom?: number;
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
    subdomains: '',
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
    subdomains: '',
    opacity: 1,
    darkFilter: true,
    maxZoom: 19,
  },
};

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

function getPolygonLatLngs(d: Entity): [number, number][] {
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

// ── Dynamic color helpers ─────────────────────────────────────────────────────

/**
 * For entity types with a speed field, return a color based on speed magnitude.
 * Falls back to the entity type's base color for types without speed.
 */
function speedColor(
  entity: Entity,
  config: EntityTypeConfig,
  selected: boolean,
): string {
  if (selected) return '#ff3232';
  if (!config.speedField) return config.colorHex;
  const speed = entity.properties?.[config.speedField];
  const s = typeof speed === 'number' && isFinite(speed) ? speed : 0;
  if (s > 20) return '#ff7800';
  if (s >= 10) return '#1e8cff';
  return '#3cc85a';
}

/** Color for polygon area entities (weather zones, regions) */
function areaFill(entity: Entity, config: EntityTypeConfig): string {
  const [r, g, b] = config.color;
  const severity = (entity.properties?.severity as string)?.toLowerCase() ?? '';
  if (severity === 'extreme') return 'rgba(220,20,20,0.30)';
  if (severity === 'high')    return 'rgba(255,90,0,0.25)';
  if (severity === 'moderate') return 'rgba(255,200,0,0.20)';
  return `rgba(${r},${g},${b},0.16)`;
}

function areaStroke(entity: Entity, config: EntityTypeConfig): string {
  const severity = (entity.properties?.severity as string)?.toLowerCase() ?? '';
  if (severity === 'extreme') return '#ff1e1e';
  if (severity === 'high')    return '#ff7800';
  if (severity === 'moderate') return '#ffdc1e';
  return config.colorHex;
}

// ── Vector projection ─────────────────────────────────────────────────────────

function projectPosition(
  lat: number,
  lon: number,
  courseDegs: number,
  speedKnots: number,
): [number, number] {
  const distNm = speedKnots * 0.5; // 30 min projection
  const courseRad = (courseDegs * Math.PI) / 180;
  const dLat = (distNm * Math.cos(courseRad)) / 60;
  const dLon = (distNm * Math.sin(courseRad)) / (60 * Math.cos((lat * Math.PI) / 180));
  return [lat + dLat, lon + dLon];
}

// ── DivIcon factories ─────────────────────────────────────────────────────────

function createArrowIcon(
  course: number,
  color: string,
  selected: boolean,
): L.DivIcon {
  const size = selected ? 22 : 16;
  const stroke = selected ? 'white' : 'rgba(0,0,0,0.6)';
  const strokeW = selected ? 2 : 1;
  const glow = selected ? `filter: drop-shadow(0 0 4px ${color});` : '';
  return L.divIcon({
    html: `<svg width="${size}" height="${size}" viewBox="0 0 20 24" xmlns="http://www.w3.org/2000/svg"
      style="transform: rotate(${course}deg); transform-origin: 50% 50%; ${glow}">
      <polygon points="10,1 18,23 10,17 2,23" fill="${color}" stroke="${stroke}"
        stroke-width="${strokeW}" stroke-linejoin="round"/>
    </svg>`,
    className: '',
    iconSize: [size, size],
    iconAnchor: [size / 2, size / 2],
  });
}

function createPlaneIcon(heading: number, color: string, selected: boolean): L.DivIcon {
  const size = selected ? 22 : 16;
  const glow = selected ? `filter: drop-shadow(0 0 4px ${color});` : '';
  return L.divIcon({
    html: `<svg width="${size}" height="${size}" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"
      style="transform: rotate(${heading}deg); transform-origin: 50% 50%; ${glow}">
      <path d="M12 2L8 10H2l3 3-2 7 9-5 9 5-2-7 3-3h-6z" fill="${color}" stroke="rgba(0,0,0,0.5)" stroke-width="0.8"/>
    </svg>`,
    className: '',
    iconSize: [size, size],
    iconAnchor: [size / 2, size / 2],
  });
}

function createDiamondIcon(color: string, selected: boolean): L.DivIcon {
  const size = selected ? 18 : 12;
  const glow = selected ? `filter: drop-shadow(0 0 4px ${color});` : '';
  return L.divIcon({
    html: `<svg width="${size}" height="${size}" viewBox="0 0 12 12" xmlns="http://www.w3.org/2000/svg" style="${glow}">
      <polygon points="6,0 12,6 6,12 0,6" fill="${color}" stroke="rgba(0,0,0,0.5)" stroke-width="0.8"/>
    </svg>`,
    className: '',
    iconSize: [size, size],
    iconAnchor: [size / 2, size / 2],
  });
}

function createSquareIcon(color: string, selected: boolean): L.DivIcon {
  const size = selected ? 16 : 10;
  const glow = selected ? `filter: drop-shadow(0 0 4px ${color});` : '';
  return L.divIcon({
    html: `<svg width="${size}" height="${size}" viewBox="0 0 12 12" xmlns="http://www.w3.org/2000/svg" style="${glow}">
      <rect x="0.5" y="0.5" width="11" height="11" fill="${color}" stroke="rgba(255,255,255,0.3)" stroke-width="0.8"/>
    </svg>`,
    className: '',
    iconSize: [size, size],
    iconAnchor: [size / 2, size / 2],
  });
}

function createEmojiIcon(emoji: string, selected: boolean): L.DivIcon {
  const sz = selected ? '16px' : '12px';
  const glow = selected ? 'text-shadow: 0 0 6px rgba(255,255,255,0.7);' : '';
  return L.divIcon({
    html: `<div style="font-size:${sz};line-height:1;${glow}">${emoji}</div>`,
    className: '',
    iconSize: [20, 20],
    iconAnchor: [10, 10],
  });
}

function createMeasureIcon(label: string): L.DivIcon {
  return L.divIcon({
    html: `<div style="width:10px;height:10px;background:#facc15;border:2px solid #fff;border-radius:50%;position:relative;">
      <span style="position:absolute;left:12px;top:-2px;color:#facc15;font-size:9px;font-family:monospace;white-space:nowrap;text-shadow:0 1px 3px rgba(0,0,0,0.9);">${label}</span>
    </div>`,
    className: '',
    iconSize: [10, 10],
    iconAnchor: [5, 5],
  });
}

// ── Tooltip content (dynamic) ─────────────────────────────────────────────────

function tooltipHTML(entity: Entity, config: EntityTypeConfig): string {
  const name = entity.name ?? entity.id;
  const type = entity.type ?? 'Unknown';
  const props = entity.properties ?? {};
  const conf = typeof entity.confidence === 'number'
    ? `${(entity.confidence * 100).toFixed(0)}%`
    : '?';

  const rows: string[] = [];

  if (config.speedField && props[config.speedField] != null) {
    const v = typeof props[config.speedField] === 'number'
      ? (props[config.speedField] as number).toFixed(1)
      : String(props[config.speedField]);
    rows.push(`Speed: ${v}`);
  }
  if (config.headingField && props[config.headingField] != null) {
    rows.push(`Heading: ${Math.round(props[config.headingField] as number)}°`);
  }
  if (config.altitudeField && props[config.altitudeField] != null) {
    rows.push(`Alt: ${(props[config.altitudeField] as number).toFixed(0)} m`);
  }
  // Generic interesting props (max 3 more)
  const skip = new Set([config.speedField, config.headingField, config.altitudeField].filter(Boolean));
  let extra = 0;
  for (const [k, v] of Object.entries(props)) {
    if (skip.has(k) || extra >= 3) break;
    if (v != null && typeof v !== 'object') {
      rows.push(`${k}: ${v}`);
      extra++;
    }
  }
  rows.push(`Confidence: ${conf}`);

  return `
    <div style="font-family:'JetBrains Mono','Fira Code',monospace;font-size:10px;line-height:1.4;min-width:130px;">
      <div style="font-weight:700;color:#e5e7eb;margin-bottom:3px;font-size:11px">${name}</div>
      <div style="color:${config.colorHex};margin-bottom:4px;font-size:9px;text-transform:uppercase;letter-spacing:.08em">${type}</div>
      ${rows.map(r => `<div style="color:#9ca3af">${r}</div>`).join('')}
    </div>
  `;
}

// ── Grid overlay ──────────────────────────────────────────────────────────────

function LatLonGrid() {
  const map = useMap();
  const [lines, setLines] = useState<Array<{ positions: [number, number][]; isLat: boolean }>>([]);

  useEffect(() => {
    const update = () => {
      const bounds = map.getBounds();
      const zoom = map.getZoom();
      let step = 10;
      if (zoom >= 8)  step = 1;
      if (zoom >= 10) step = 0.5;
      if (zoom >= 12) step = 0.25;
      if (zoom >= 14) step = 0.1;

      const newLines: typeof lines = [];
      const minLat = Math.floor(bounds.getSouth() / step) * step;
      const maxLat = Math.ceil(bounds.getNorth() / step) * step;
      for (let lat = minLat; lat <= maxLat; lat = Math.round((lat + step) * 1e6) / 1e6) {
        newLines.push({ positions: [[lat, bounds.getWest()], [lat, bounds.getEast()]], isLat: true });
      }
      const minLon = Math.floor(bounds.getWest() / step) * step;
      const maxLon = Math.ceil(bounds.getEast() / step) * step;
      for (let lon = minLon; lon <= maxLon; lon = Math.round((lon + step) * 1e6) / 1e6) {
        newLines.push({ positions: [[bounds.getSouth(), lon], [bounds.getNorth(), lon]], isLat: false });
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
        <Polyline key={i} positions={line.positions}
          pathOptions={{ color: '#334155', weight: 0.5, opacity: 0.5, dashArray: '2 4' }} />
      ))}
    </>
  );
}

// ── Coord tracker ─────────────────────────────────────────────────────────────

function CoordTracker({ onCoords }: { onCoords: (c: [number, number] | null) => void }) {
  useMapEvents({
    mousemove(e) { onCoords([e.latlng.lat, e.latlng.lng]); },
    mouseout()   { onCoords(null); },
  });
  return null;
}

// ── Lasso handler ─────────────────────────────────────────────────────────────

function LassoHandler({ active, onSelect }: { active: boolean; onSelect: (b: L.LatLngBounds) => void }) {
  const map = useMap();
  const startRef = useRef<L.LatLng | null>(null);
  const draggingRef = useRef(false);
  const [bounds, setBounds] = useState<L.LatLngBounds | null>(null);

  useEffect(() => {
    if (!active) { setBounds(null); return; }
    const onDown = (e: L.LeafletMouseEvent) => {
      if (!e.originalEvent.shiftKey) return;
      e.originalEvent.preventDefault();
      map.dragging.disable();
      startRef.current = e.latlng;
      draggingRef.current = true;
    };
    const onMove = (e: L.LeafletMouseEvent) => {
      if (!draggingRef.current || !startRef.current) return;
      setBounds(L.latLngBounds(startRef.current, e.latlng));
    };
    const onUp = (e: L.LeafletMouseEvent) => {
      if (!draggingRef.current || !startRef.current) return;
      draggingRef.current = false;
      map.dragging.enable();
      onSelect(L.latLngBounds(startRef.current, e.latlng));
      setBounds(null);
      startRef.current = null;
    };
    map.on('mousedown', onDown as L.LeafletEventHandlerFn);
    map.on('mousemove', onMove as L.LeafletEventHandlerFn);
    map.on('mouseup', onUp as L.LeafletEventHandlerFn);
    return () => {
      map.off('mousedown', onDown as L.LeafletEventHandlerFn);
      map.off('mousemove', onMove as L.LeafletEventHandlerFn);
      map.off('mouseup', onUp as L.LeafletEventHandlerFn);
      map.dragging.enable();
    };
  }, [active, map, onSelect]);

  if (!bounds) return null;
  return (
    <Rectangle bounds={bounds}
      pathOptions={{ color: '#00e5ff', weight: 1.5, fillColor: '#00e5ff', fillOpacity: 0.05, dashArray: '4 3' }} />
  );
}

// ── Measurement tool ──────────────────────────────────────────────────────────

function MeasurementHandler({ active }: { active: boolean }) {
  const [points, setPoints] = useState<L.LatLng[]>([]);
  useMapEvents({
    click(e) {
      if (!active) return;
      e.originalEvent.stopPropagation();
      setPoints((prev) => prev.length >= 2 ? [e.latlng] : [...prev, e.latlng]);
    },
  });
  useEffect(() => { if (!active) setPoints([]); }, [active]);

  if (!active || points.length === 0) return null;
  const distM = points.length === 2 ? points[0].distanceTo(points[1]) : null;
  const label = distM !== null
    ? `${(distM / 1000).toFixed(2)} km / ${(distM / 1852).toFixed(2)} nm`
    : 'Click 2nd point';

  return (
    <>
      {points.map((pt, i) => <Marker key={i} position={pt} icon={createMeasureIcon(i === 0 ? 'A' : 'B')} />)}
      {points.length === 2 && (
        <>
          <Polyline positions={[points[0], points[1]]}
            pathOptions={{ color: '#facc15', weight: 2, dashArray: '6 4' }} />
          <Marker
            position={[(points[0].lat + points[1].lat) / 2, (points[0].lng + points[1].lng) / 2]}
            icon={L.divIcon({
              html: `<div style="background:rgba(0,0,0,0.85);border:1px solid #facc15;color:#facc15;
                font-family:monospace;font-size:10px;padding:2px 6px;white-space:nowrap;
                transform:translateX(-50%);">${label}</div>`,
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
  useEffect(() => { mapRef.current = map; }, [map, mapRef]);
  useEffect(() => {
    if (zoomToFitTrigger === 0) return;
    const points = allEntities.flatMap(e => { const p = getLatLng(e); return p ? [L.latLng(p[0], p[1])] : []; });
    if (points.length === 0) return;
    map.fitBounds(L.latLngBounds(points), { padding: [40, 40], maxZoom: 12 });
  }, [zoomToFitTrigger, map, allEntities]);
  useEffect(() => {
    if (!zoomToPos) return;
    map.setView(zoomToPos, Math.max(map.getZoom(), 13), { animate: true });
  }, [zoomToPos, map]);
  return null;
}

// ── Track renderer (fading polyline) ─────────────────────────────────────────

function useRenderTrack() {
  return useCallback((entity: Entity, config: EntityTypeConfig, isSelected: boolean) => {
    const history = entity.history ?? [];
    const positions = history
      .slice(-50)
      .filter(h => {
        const c = h.geometry?.coordinates;
        return Array.isArray(c) && c.length >= 2 &&
          typeof c[0] === 'number' && typeof c[1] === 'number' &&
          isFinite(c[0]) && isFinite(c[1]);
      })
      .map(h => {
        const [lon, lat] = h.geometry!.coordinates as number[];
        return [lat, lon] as [number, number];
      });

    if (positions.length < 2) return null;
    const segCount = 5;
    const segSize = Math.ceil(positions.length / segCount);
    const segments = [];
    for (let i = 0; i < segCount; i++) {
      const pts = positions.slice(i * segSize, Math.min((i + 1) * segSize + 1, positions.length));
      if (pts.length < 2) continue;
      segments.push({
        pts,
        opacity: isSelected ? 0.15 + i * 0.17 : 0.05 + i * 0.06,
      });
    }
    const trackColor = isSelected ? '#ffc832' : config.colorHex + '88';
    return segments.map((seg, si) => (
      <Polyline
        key={`track-${entity.id}-${si}`}
        positions={seg.pts}
        pathOptions={{
          color: trackColor,
          weight: isSelected ? 2 : 1,
          opacity: seg.opacity,
        }}
      />
    ));
  }, []);
}

// ── Entity marker renderer ────────────────────────────────────────────────────

interface EntityMarkersProps {
  entities: Entity[];
  config: EntityTypeConfig;
  layerOn: boolean;
  tracksOn: boolean;
  vectorsOn: boolean;
  selectedEntityId: string | null;
  selectedEntities: Set<string>;
  onEntityClick: (id: string) => (e: L.LeafletEvent) => void;
  onEntityDblClick: (pos: [number, number]) => (e: L.LeafletEvent) => void;
  renderTrack: (entity: Entity, config: EntityTypeConfig, isSelected: boolean) => React.ReactNode;
}

const EntityMarkers: React.FC<EntityMarkersProps> = ({
  entities,
  config,
  layerOn,
  tracksOn,
  vectorsOn,
  selectedEntityId,
  selectedEntities,
  onEntityClick,
  onEntityDblClick,
  renderTrack,
}) => {
  if (!layerOn) return null;

  return (
    <>
      {/* Area entities (polygons) */}
      {config.isArea && entities.map((entity) => {
        const positions = getPolygonLatLngs(entity);
        if (positions.length === 0) return null;
        const isSelected = entity.id === selectedEntityId;
        return (
          <Polygon
            key={entity.id}
            positions={positions}
            pathOptions={{
              fillColor: areaFill(entity, config),
              fillOpacity: 1,
              color: isSelected ? '#ffffff' : areaStroke(entity, config),
              weight: isSelected ? 2.5 : 1.5,
            }}
            eventHandlers={{ click: onEntityClick(entity.id) }}
          >
            <Tooltip direction="top" sticky className="cop-tooltip" opacity={1}>
              <div dangerouslySetInnerHTML={{ __html: tooltipHTML(entity, config) }} />
            </Tooltip>
          </Polygon>
        );
      })}

      {/* Point entities */}
      {!config.isArea && entities.map((entity) => {
        const pos = getLatLng(entity);
        if (!pos) return null;
        const isSelected = entity.id === selectedEntityId;
        const isMulti = selectedEntities.has(entity.id);
        const color = speedColor(entity, config, isSelected);

        // Track
        const track = (tracksOn && config.showTrack) ? renderTrack(entity, config, isSelected) : null;

        // Vector
        const vecEl = (() => {
          if (!vectorsOn || !config.showVector || !config.headingField || !config.speedField) return null;
          const speed = entity.properties?.[config.speedField];
          const heading = entity.properties?.[config.headingField];
          if (typeof speed !== 'number' || typeof heading !== 'number' || speed < 0.5) return null;
          const projected = projectPosition(pos[0], pos[1], heading, speed);
          return (
            <Polyline
              key={`vec-${entity.id}`}
              positions={[pos, projected]}
              pathOptions={{
                color: isSelected ? '#ffffff' : color,
                weight: isSelected ? 2 : 1,
                opacity: isSelected ? 0.9 : 0.55,
                dashArray: '6 3',
              }}
            />
          );
        })();

        // Choose marker type
        const marker = (() => {
          const tooltip = (
            <Tooltip direction="top" sticky offset={[0, -8]} opacity={1} className="cop-tooltip">
              <div dangerouslySetInnerHTML={{ __html: tooltipHTML(entity, config) }} />
            </Tooltip>
          );

          if (config.markerStyle === 'circle') {
            const sizeProp = entity.properties?.total_teu ?? entity.properties?.capacity ?? 0;
            const radius = isSelected
              ? 14
              : Math.max(6, Math.min(14, 4 + Math.log10((sizeProp as number) + 1) * 2));
            return (
              <CircleMarker
                key={entity.id}
                center={pos}
                radius={radius}
                pathOptions={{
                  fillColor: color,
                  fillOpacity: isSelected ? 0.95 : 0.75,
                  color: isSelected || isMulti ? '#ffffff' : 'rgba(255,255,255,0.35)',
                  weight: isSelected ? 2.5 : isMulti ? 2 : 1,
                }}
                eventHandlers={{
                  click: onEntityClick(entity.id),
                  dblclick: onEntityDblClick(pos),
                }}
              >
                {tooltip}
              </CircleMarker>
            );
          }

          if (config.markerStyle === 'dot') {
            return (
              <CircleMarker
                key={entity.id}
                center={pos}
                radius={isSelected ? 8 : 5}
                pathOptions={{
                  fillColor: color,
                  fillOpacity: 0.85,
                  color: isSelected ? '#ffffff' : 'rgba(0,0,0,0.4)',
                  weight: isSelected ? 2 : 1,
                }}
                eventHandlers={{ click: onEntityClick(entity.id) }}
              >
                {tooltip}
              </CircleMarker>
            );
          }

          // Arrow / plane / diamond / square / emoji — use DivIcon Marker
          const heading = config.headingField
            ? (typeof entity.properties?.[config.headingField] === 'number'
                ? entity.properties[config.headingField] as number
                : 0)
            : 0;

          let icon: L.DivIcon;
          if (config.iconIsEmoji && config.icon) {
            icon = createEmojiIcon(config.icon, isSelected);
          } else if (config.markerStyle === 'plane') {
            icon = createPlaneIcon(heading, color, isSelected);
          } else if (config.markerStyle === 'diamond') {
            icon = createDiamondIcon(color, isSelected);
          } else if (config.markerStyle === 'square') {
            icon = createSquareIcon(color, isSelected);
          } else {
            icon = createArrowIcon(heading, color, isSelected);
          }

          return (
            <Marker
              key={entity.id}
              position={pos}
              icon={icon}
              zIndexOffset={isSelected ? 1000 : isMulti ? 500 : 0}
              eventHandlers={{
                click: onEntityClick(entity.id),
                dblclick: (e: L.LeafletEvent) => {
                  (e as L.LeafletMouseEvent).originalEvent?.stopPropagation();
                  onEntityDblClick(pos)(e);
                },
              }}
            >
              {tooltip}
            </Marker>
          );
        })();

        return (
          <React.Fragment key={entity.id}>
            {track}
            {vecEl}
            {marker}
          </React.Fragment>
        );
      })}
    </>
  );
};

// ── Main Component ────────────────────────────────────────────────────────────

const DARK_FILTER_STYLE = `
  .cop-dark-filter .leaflet-tile {
    filter: brightness(0.5) saturate(0.4) hue-rotate(190deg) !important;
  }
`;

export const MapView: React.FC = () => {
  const entities         = useAppStore((s) => s.entities);
  const selectedEntityId = useAppStore((s) => s.selectedEntityId);
  const selectedEntities = useAppStore((s) => s.selectedEntities);
  const selectEntity     = useAppStore((s) => s.selectEntity);
  const toggleEntitySelection = useAppStore((s) => s.toggleEntitySelection);

  const [activeTile, setActiveTile] = useState<TileLayerType>('osm');
  const [layers, setLayers] = useState<LayerVisibility>({
    tracks: true,
    vectors: true,
    grid: false,
  });
  const [measureActive, setMeasureActive] = useState(false);
  const [lassoActive, setLassoActive]     = useState(false);
  const [mouseCoords, setMouseCoords]     = useState<[number, number] | null>(null);
  const [zoomToFitTrigger, setZoomToFitTrigger] = useState(0);
  const [zoomToPos, setZoomToPos]         = useState<[number, number] | null>(null);
  const mapRef = useRef<L.Map | null>(null);

  const allEntities = useMemo(() => Array.from(entities.values()), [entities]);

  // ── Dynamic entity type registry ──
  const registry = useEntityTypes(entities);
  const entityTypesList = useMemo(() => Array.from(registry.values()), [registry]);

  // ── Group entities by type ──
  const grouped = useMemo(() => groupEntitiesByType(entities, registry), [entities, registry]);

  // ── Entity counts per type ──
  const entityCounts = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const [type, list] of grouped) {
      counts[type] = list.filter(hasValidGeometry).length;
    }
    return counts;
  }, [grouped]);

  // ── Entity types visible on map (have geometry) ──
  const visibleTypes = useMemo(
    () => entityTypesList.filter((cfg) => (entityCounts[cfg.type] ?? 0) > 0),
    [entityTypesList, entityCounts],
  );

  const renderTrack = useRenderTrack();

  const handleToggleLayer = useCallback((layer: string) => {
    setLayers((prev) => ({ ...prev, [layer]: !(prev[layer] !== false) }));
  }, []);

  const handleEntityClick = useCallback(
    (id: string) => (e: L.LeafletEvent) => {
      (e as L.LeafletMouseEvent).originalEvent?.stopPropagation();
      selectEntity(id);
    },
    [selectEntity],
  );

  const handleEntityDblClick = useCallback(
    (pos: [number, number]) => (_e: L.LeafletEvent) => { setZoomToPos(pos); },
    [],
  );

  const handleLassoSelect = useCallback(
    (bounds: L.LatLngBounds) => {
      allEntities.forEach((e) => {
        const pos = getLatLng(e);
        if (pos && bounds.contains(L.latLng(pos[0], pos[1]))) toggleEntitySelection(e.id);
      });
    },
    [allEntities, toggleEntitySelection],
  );

  const tileConfig = TILE_CONFIGS[activeTile];
  const tileLayerClass = tileConfig.darkFilter ? 'cop-dark-filter' : '';

  return (
    <div className="relative flex-1 overflow-hidden" role="application" aria-label="Common Operating Picture map">
      <style>{`
        ${DARK_FILTER_STYLE}
        .cop-tooltip {
          background: rgba(5, 8, 16, 0.96) !important;
          border: 1px solid rgba(75, 85, 99, 0.8) !important;
          border-radius: 0 !important;
          padding: 6px 8px !important;
          box-shadow: 0 4px 16px rgba(0,0,0,0.6) !important;
        }
        .cop-tooltip::before { display: none !important; }
        .leaflet-tooltip-top.cop-tooltip::before { display: none !important; }
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

      <MapContainer
        center={[51.92, 4.27]}
        zoom={10}
        className="h-full w-full"
        style={{ background: '#060b14' }}
        zoomControl={false}
        attributionControl={false}
      >
        <TileLayer
          key={activeTile}
          url={tileConfig.url}
          attribution={tileConfig.attribution}
          subdomains={tileConfig.subdomains}
          opacity={tileConfig.opacity}
          maxZoom={tileConfig.maxZoom}
          className={tileLayerClass}
        />

        <ScaleControl position="bottomleft" imperial metric />
        <CoordTracker onCoords={setMouseCoords} />
        <MapController
          zoomToFitTrigger={zoomToFitTrigger}
          allEntities={allEntities}
          mapRef={mapRef}
          zoomToPos={zoomToPos}
        />

        {layers.grid && <LatLonGrid />}
        <MeasurementHandler active={measureActive} />
        <LassoHandler active={lassoActive} onSelect={handleLassoSelect} />

        {/* ── Dynamic entity layers — one pass per type ─────────────────── */}
        {visibleTypes.map((config) => (
          <EntityMarkers
            key={config.type}
            entities={(grouped.get(config.type) ?? []).filter(hasValidGeometry)}
            config={config}
            layerOn={layers[config.type] !== false}
            tracksOn={layers['tracks'] !== false}
            vectorsOn={layers['vectors'] !== false}
            selectedEntityId={selectedEntityId}
            selectedEntities={selectedEntities}
            onEntityClick={handleEntityClick}
            onEntityDblClick={handleEntityDblClick}
            renderTrack={renderTrack}
          />
        ))}
      </MapContainer>

      {/* ── Controls overlay ──────────────────────────────────────────────── */}
      <MapControls
        activeTile={activeTile}
        onTileChange={setActiveTile}
        entityTypes={visibleTypes}
        entityCounts={entityCounts}
        layers={layers}
        onToggleLayer={handleToggleLayer}
        measureActive={measureActive}
        lassoActive={lassoActive}
        onToggleMeasure={() => { setMeasureActive((v) => !v); if (lassoActive) setLassoActive(false); }}
        onToggleLasso={() => { setLassoActive((v) => !v); if (measureActive) setMeasureActive(false); }}
        onZoomToFit={() => setZoomToFitTrigger((n) => n + 1)}
        mouseCoords={mouseCoords}
      />

      {/* ── Dynamic legend ────────────────────────────────────────────────── */}
      <MapLegend
        entityTypes={visibleTypes}
        measureActive={measureActive}
        lassoActive={lassoActive}
      />
    </div>
  );
};
