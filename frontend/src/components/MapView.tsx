import React, { useMemo, useCallback } from 'react';
import {
  MapContainer,
  TileLayer,
  CircleMarker,
  Polygon,
  Polyline,
  Tooltip,
  useMapEvents,
} from 'react-leaflet';
import type { LeafletMouseEvent, LeafletEvent } from 'leaflet';
import 'leaflet/dist/leaflet.css';
import { useAppStore } from '../store/useAppStore';
import type { Entity } from '../types';

// ── Color helpers ────────────────────────────────────────────────────────

function shipColorCSS(speed: unknown, selected: boolean): string {
  if (selected) return '#ff3232';
  const s = typeof speed === 'number' && isFinite(speed) ? speed : 0;
  if (s > 20) return '#ff7800';
  if (s >= 10) return '#1e8cff';
  return '#3cc85a';
}

function congestionColorCSS(congestion: unknown): string {
  const c = typeof congestion === 'number' && isFinite(congestion) ? congestion : 0;
  if (c > 0.75) return '#dc3232';
  if (c > 0.4) return '#ffa500';
  return '#32b464';
}

function weatherFillCSS(severity: string): string {
  switch (severity?.toLowerCase()) {
    case 'extreme': return 'rgba(220,20,20,0.35)';
    case 'high': return 'rgba(255,90,0,0.28)';
    case 'moderate': return 'rgba(255,200,0,0.25)';
    default: return 'rgba(100,160,255,0.18)';
  }
}

function weatherStrokeCSS(severity: string): string {
  switch (severity?.toLowerCase()) {
    case 'extreme': return '#ff1e1e';
    case 'high': return '#ff7800';
    case 'moderate': return '#ffdc1e';
    default: return '#78b4ff';
  }
}

// ── Geometry helpers ────────────────────────────────────────────────────

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
      return [lat, lon]; // Leaflet uses [lat, lng]
    }
  }
  return null;
}

function hasValidPointCoords(d: Entity): boolean {
  return getLatLng(d) !== null;
}

function hasValidGeometry(e: Entity): boolean {
  if (!e.geometry) return false;
  if (e.geometry.type === 'Point') return hasValidPointCoords(e);
  if (e.geometry.type === 'Polygon' && Array.isArray(e.geometry.coordinates)) return true;
  return false;
}

function getWeatherLatLngs(d: Entity): [number, number][] {
  if (d.geometry?.type === 'Polygon' && Array.isArray(d.geometry.coordinates)) {
    const ring = (d.geometry.coordinates as number[][][])[0];
    if (Array.isArray(ring)) {
      return ring.map(([lon, lat]) => [lat, lon] as [number, number]);
    }
  }
  // If it's a Point with radius, generate a circle polygon
  if (
    d.geometry?.type === 'Point' &&
    Array.isArray(d.geometry.coordinates) &&
    d.geometry.coordinates.length >= 2
  ) {
    const [lon, lat] = d.geometry.coordinates as number[];
    if (typeof lon === 'number' && typeof lat === 'number' && isFinite(lon) && isFinite(lat)) {
      const r = ((d.properties?.radius_km as number) ?? 100) / 111;
      const pts: [number, number][] = [];
      for (let i = 0; i <= 32; i++) {
        const angle = (i / 32) * 2 * Math.PI;
        pts.push([lat + r * 0.6 * Math.sin(angle), lon + r * Math.cos(angle)]);
      }
      return pts;
    }
  }
  return [];
}

// ── Tooltip content builder ─────────────────────────────────────────────

function entityTooltipContent(entity: Entity): string {
  const name = entity.name ?? entity.id;
  const typeLower = entity.type?.toLowerCase();
  const typeLabel = entity.type ?? 'Unknown';
  const props = entity.properties ?? {};
  let extra = '';
  if (typeLower === 'ship') {
    const speed = typeof props.speed === 'number' ? props.speed.toFixed(1) : '?';
    extra = ` · ${speed} kn`;
  } else if (typeLower === 'port') {
    const teu = typeof props.total_teu === 'number' ? props.total_teu.toLocaleString() : '0';
    extra = ` · TEU: ${teu}`;
  } else if (typeLower === 'weathersystem') {
    const severity = (props.severity as string) ?? 'unknown';
    extra = ` · ${severity}`;
  }
  return `${name} (${typeLabel})${extra}`;
}

// ── Click handler propagator component ──────────────────────────────────

function MapClickReset({ selectEntity }: { selectEntity: (id: string | null) => void }) {
  useMapEvents({
    click() {
      // Clicking on the map background deselects
      // (marker clicks stop propagation so this won't fire for markers)
    },
  });
  return null;
}

// ── Main Component ──────────────────────────────────────────────────────

export const MapView: React.FC = () => {
  const entities = useAppStore((s) => s.entities);
  const selectedEntityId = useAppStore((s) => s.selectedEntityId);
  const showWeatherLayer = useAppStore((s) => s.showWeatherLayer);
  const showShipTracksLayer = useAppStore((s) => s.showShipTracksLayer);
  const selectEntity = useAppStore((s) => s.selectEntity);

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

  const shipsWithTracks = useMemo(
    () => ships.filter((s) => s.history && s.history.length > 1),
    [ships],
  );

  const handleEntityClick = useCallback(
    (id: string) => (e: LeafletEvent) => {
      // Stop propagation so map click reset doesn't fire
      (e as LeafletMouseEvent).originalEvent?.stopPropagation();
      selectEntity(id);
    },
    [selectEntity],
  );

  return (
    <div
      className="relative flex-1 overflow-hidden"
      role="application"
      aria-label="Interactive map showing entities"
    >
      <MapContainer
        center={[51.92, 4.27]}
        zoom={10}
        className="h-full w-full"
        style={{ background: '#0a0e17' }}
        zoomControl={true}
        attributionControl={false}
      >
        <TileLayer
          url="https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png"
          attribution='&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>'
          opacity={0.4}
        />

        <MapClickReset selectEntity={selectEntity} />

        {/* Weather system polygons */}
        {showWeatherLayer &&
          weatherSystems.map((w) => {
            const positions = getWeatherLatLngs(w);
            if (positions.length === 0) return null;
            const severity = (w.properties?.severity as string) ?? '';
            return (
              <Polygon
                key={w.id}
                positions={positions}
                pathOptions={{
                  fillColor: weatherFillCSS(severity),
                  fillOpacity: 1,
                  color: weatherStrokeCSS(severity),
                  weight: 2,
                }}
                eventHandlers={{
                  click: (e) => {
                    e.originalEvent.stopPropagation();
                    selectEntity(w.id);
                  },
                }}
              >
                <Tooltip direction="top" sticky>
                  {entityTooltipContent(w)}
                </Tooltip>
              </Polygon>
            );
          })}

        {/* Ship tracks (historical paths) */}
        {showShipTracksLayer &&
          shipsWithTracks.map((ship) => {
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
            const isSelected = ship.id === selectedEntityId;
            return (
              <Polyline
                key={`track-${ship.id}`}
                positions={trackPositions}
                pathOptions={{
                  color: isSelected ? '#ffc832' : '#64a0dc',
                  weight: isSelected ? 3 : 1.5,
                  opacity: isSelected ? 0.8 : 0.3,
                }}
              />
            );
          })}

        {/* Port markers */}
        {ports.map((port) => {
          const pos = getLatLng(port);
          if (!pos) return null;
          const isSelected = port.id === selectedEntityId;
          return (
            <CircleMarker
              key={port.id}
              center={pos}
              radius={isSelected ? 12 : 8}
              pathOptions={{
                fillColor: congestionColorCSS((port.properties ?? {}).congestion ?? 0),
                fillOpacity: 0.8,
                color: isSelected ? '#ffffff' : 'rgba(255,255,255,0.4)',
                weight: isSelected ? 2 : 1,
              }}
              eventHandlers={{
                click: handleEntityClick(port.id),
              }}
            >
              <Tooltip direction="top" sticky>
                {entityTooltipContent(port)}
              </Tooltip>
            </CircleMarker>
          );
        })}

        {/* Ship markers */}
        {ships.map((ship) => {
          const pos = getLatLng(ship);
          if (!pos) return null;
          const isSelected = ship.id === selectedEntityId;
          const speed = (ship.properties ?? {}).speed;
          return (
            <CircleMarker
              key={ship.id}
              center={pos}
              radius={isSelected ? 12 : 8}
              pathOptions={{
                fillColor: shipColorCSS(speed, isSelected),
                fillOpacity: 0.9,
                color: isSelected ? '#ffffff' : 'rgba(255,255,255,0.3)',
                weight: isSelected ? 2 : 1,
              }}
              eventHandlers={{
                click: handleEntityClick(ship.id),
              }}
            >
              <Tooltip direction="top" sticky>
                {entityTooltipContent(ship)}
              </Tooltip>
            </CircleMarker>
          );
        })}
      </MapContainer>

      {/* Map Legend */}
      <div
        className="absolute bottom-12 right-3 z-[1000] rounded-none bg-gray-900/90 border border-gray-700 p-2.5 text-[10px] text-gray-400 space-y-1 backdrop-blur-sm"
        aria-label="Map legend"
        role="img"
      >
        <div className="text-gray-300 font-semibold text-[11px] mb-1.5" aria-hidden="true">
          Ships
        </div>
        <dl className="space-y-1">
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Green dot</dt>
            <dd className="flex items-center gap-1.5">
              <span
                className="w-2.5 h-2.5 rounded-none bg-green-500 inline-block"
                aria-hidden="true"
              />
              <span>&lt; 10 kn</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Blue dot</dt>
            <dd className="flex items-center gap-1.5">
              <span
                className="w-2.5 h-2.5 rounded-none bg-blue-400 inline-block"
                aria-hidden="true"
              />
              <span>10–20 kn</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Orange dot</dt>
            <dd className="flex items-center gap-1.5">
              <span
                className="w-2.5 h-2.5 rounded-none bg-orange-400 inline-block"
                aria-hidden="true"
              />
              <span>&gt; 20 kn</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Red dot</dt>
            <dd className="flex items-center gap-1.5">
              <span
                className="w-2.5 h-2.5 rounded-none bg-red-500 inline-block"
                aria-hidden="true"
              />
              <span>Selected</span>
            </dd>
          </div>
          <div
            className="border-t border-gray-700 mt-1.5 pt-1.5 text-gray-300 font-semibold text-[11px]"
            aria-hidden="true"
          >
            Ports
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Green port dot</dt>
            <dd className="flex items-center gap-1.5">
              <span
                className="w-2.5 h-2.5 rounded-none bg-green-400 inline-block opacity-80"
                aria-hidden="true"
              />
              <span>Low congestion</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Orange port dot</dt>
            <dd className="flex items-center gap-1.5">
              <span
                className="w-2.5 h-2.5 rounded-none bg-orange-400 inline-block opacity-80"
                aria-hidden="true"
              />
              <span>Medium</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Red port dot</dt>
            <dd className="flex items-center gap-1.5">
              <span
                className="w-2.5 h-2.5 rounded-none bg-red-500 inline-block opacity-80"
                aria-hidden="true"
              />
              <span>High</span>
            </dd>
          </div>
        </dl>
      </div>

      {/* Entity count badge */}
      <div className="absolute top-3 right-3 z-[1000]" aria-live="polite" aria-atomic="true">
        <span
          className="rounded-none bg-gray-900/80 border border-gray-700 px-2 py-1 text-[10px] text-gray-400"
          aria-label={`${ships.length} ships and ${ports.length} ports loaded on map`}
        >
          {ships.length} ships · {ports.length} ports
        </span>
      </div>
    </div>
  );
};
