import React, { useMemo, useCallback, useState, useRef, useEffect } from 'react';
import DeckGL from '@deck.gl/react';
import {
  ScatterplotLayer,
  PathLayer,
  PolygonLayer,
} from '@deck.gl/layers';
import { useAppStore } from '../store/useAppStore';
import type { Entity } from '../types';
import { EUROPE_COASTLINE, generateGrid } from './mapData';

// Speed color thresholds (RGBA)
function shipColor(speed: unknown, selected: boolean): [number, number, number, number] {
  if (selected) return [255, 50, 50, 255];
  const s = typeof speed === 'number' && isFinite(speed) ? speed : 0;
  if (s > 20) return [255, 120, 0, 240];
  if (s >= 10) return [30, 140, 255, 230];
  return [60, 200, 90, 220];
}

function congestionColor(congestion: unknown): [number, number, number, number] {
  const c = typeof congestion === 'number' && isFinite(congestion) ? congestion : 0;
  if (c > 0.75) return [220, 50, 50, 200];
  if (c > 0.4) return [255, 165, 0, 180];
  return [50, 180, 100, 160];
}

function weatherColor(severity: string): [number, number, number, number] {
  switch (severity?.toLowerCase()) {
    case 'extreme': return [220, 20, 20, 110];
    case 'high': return [255, 90, 0, 90];
    case 'moderate': return [255, 200, 0, 80];
    default: return [100, 160, 255, 60];
  }
}

function weatherLineColor(severity: string): [number, number, number, number] {
  switch (severity?.toLowerCase()) {
    case 'extreme': return [255, 30, 30, 220];
    case 'high': return [255, 120, 0, 200];
    case 'moderate': return [255, 220, 30, 180];
    default: return [120, 180, 255, 160];
  }
}

function getPointCoords(d: Entity): [number, number] {
  if (
    d.geometry?.type === 'Point' &&
    Array.isArray(d.geometry.coordinates) &&
    d.geometry.coordinates.length >= 2 &&
    typeof d.geometry.coordinates[0] === 'number' &&
    typeof d.geometry.coordinates[1] === 'number' &&
    isFinite(d.geometry.coordinates[0] as number) &&
    isFinite(d.geometry.coordinates[1] as number)
  ) {
    return [d.geometry.coordinates[0] as number, d.geometry.coordinates[1] as number];
  }
  return [0, 0];
}

/** Check if entity has valid point coordinates suitable for rendering */
function hasValidPointCoords(d: Entity): boolean {
  if (!d.geometry || d.geometry.type !== 'Point' || !Array.isArray(d.geometry.coordinates)) return false;
  const coords = d.geometry.coordinates;
  if (coords.length < 2) return false;
  const [lon, lat] = coords as number[];
  if (typeof lon !== 'number' || typeof lat !== 'number') return false;
  if (!isFinite(lon) || !isFinite(lat)) return false;
  if (lon === 0 && lat === 0) return false;
  if (lat < -90 || lat > 90 || lon < -180 || lon > 180) return false;
  return true;
}

function getWeatherPolygon(d: Entity): number[][] {
  if (d.geometry?.type === 'Polygon' && Array.isArray(d.geometry.coordinates)) {
    const ring = (d.geometry.coordinates as number[][][])[0];
    return Array.isArray(ring) ? ring : [];
  }
  if (
    d.geometry?.type === 'Point' &&
    Array.isArray(d.geometry.coordinates) &&
    d.geometry.coordinates.length >= 2 &&
    typeof d.geometry.coordinates[0] === 'number' &&
    typeof d.geometry.coordinates[1] === 'number' &&
    isFinite(d.geometry.coordinates[0] as number) &&
    isFinite(d.geometry.coordinates[1] as number)
  ) {
    const [lon, lat] = d.geometry.coordinates as number[];
    const r = ((d.properties?.radius_km as number) ?? 100) / 111;
    const pts: number[][] = [];
    for (let i = 0; i <= 32; i++) {
      const angle = (i / 32) * 2 * Math.PI;
      pts.push([lon + r * Math.cos(angle), lat + r * 0.6 * Math.sin(angle)]);
    }
    return pts;
  }
  return [];
}

// How many degrees to pan per keypress and zoom per keypress
const PAN_DELTA = 0.5;
const ZOOM_DELTA = 0.5;

export const MapView: React.FC = () => {
  const entities = useAppStore((s) => s.entities);
  const selectedEntityId = useAppStore((s) => s.selectedEntityId);
  const mapCenter = useAppStore((s) => s.mapCenter);
  const mapZoom = useAppStore((s) => s.mapZoom);
  const showWeatherLayer = useAppStore((s) => s.showWeatherLayer);
  const showShipTracksLayer = useAppStore((s) => s.showShipTracksLayer);
  const showHeatmapLayer = useAppStore((s) => s.showHeatmapLayer);
  const selectEntity = useAppStore((s) => s.selectEntity);

  const [tooltip, setTooltip] = useState<{
    x: number;
    y: number;
    content: string;
  } | null>(null);

  const [viewState, setViewState] = useState({
    longitude: mapCenter[0],
    latitude: mapCenter[1],
    zoom: mapZoom,
    pitch: 0,
    bearing: 0,
  });

  // Track whether the map container is focused for keyboard controls
  const [mapFocused, setMapFocused] = useState(false);
  const mapWrapperRef = useRef<HTMLDivElement>(null);

  const allEntities = useMemo(() => Array.from(entities.values()), [entities]);

  const hasValidGeometry = (e: Entity): boolean => {
    if (!e.geometry) return false;
    if (e.geometry.type === 'Point') return hasValidPointCoords(e);
    if (e.geometry.type === 'Polygon' && Array.isArray(e.geometry.coordinates)) return true;
    return false;
  };

  const ships = useMemo(
    () => allEntities.filter((e) => e.type?.toLowerCase() === 'ship' && hasValidGeometry(e)),
    [allEntities]
  );

  const ports = useMemo(
    () => allEntities.filter((e) => e.type?.toLowerCase() === 'port' && hasValidGeometry(e)),
    [allEntities]
  );

  const weatherSystems = useMemo(
    () => allEntities.filter((e) => e.type?.toLowerCase() === 'weathersystem' && hasValidGeometry(e)),
    [allEntities]
  );

  const shipsWithTracks = useMemo(
    () => ships.filter((s) => s.history && s.history.length > 1),
    [ships]
  );

  const handleHover = useCallback((info: { object?: Entity; x?: number; y?: number }) => {
    if (info.object && info.x != null && info.y != null) {
      const name = info.object.name ?? info.object.id;
      const typeLower = info.object.type?.toLowerCase();
      const props = info.object.properties ?? {};
      const extra =
        typeLower === 'ship'
          ? ` · ${typeof props.speed === 'number' ? props.speed.toFixed(1) : '?'} kn`
          : typeLower === 'port'
          ? ` · TEU: ${(typeof props.total_teu === 'number' ? props.total_teu : 0).toLocaleString()}`
          : '';
      setTooltip({ x: info.x, y: info.y, content: `${name}${extra}` });
    } else {
      setTooltip(null);
    }
  }, []);

  // ── Keyboard navigation for map ─────────────────────────────────────────
  const handleMapKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      // Only intercept when map is focused and no modifier keys that might be
      // browser shortcuts (Ctrl/Meta).
      if (e.ctrlKey || e.metaKey) return;

      let handled = false;

      switch (e.key) {
        case 'ArrowLeft':
          setViewState((vs) => ({ ...vs, longitude: vs.longitude - PAN_DELTA }));
          handled = true;
          break;
        case 'ArrowRight':
          setViewState((vs) => ({ ...vs, longitude: vs.longitude + PAN_DELTA }));
          handled = true;
          break;
        case 'ArrowUp':
          setViewState((vs) => ({ ...vs, latitude: Math.min(85, vs.latitude + PAN_DELTA) }));
          handled = true;
          break;
        case 'ArrowDown':
          setViewState((vs) => ({ ...vs, latitude: Math.max(-85, vs.latitude - PAN_DELTA) }));
          handled = true;
          break;
        case '+':
        case '=':
          setViewState((vs) => ({ ...vs, zoom: Math.min(22, vs.zoom + ZOOM_DELTA) }));
          handled = true;
          break;
        case '-':
          setViewState((vs) => ({ ...vs, zoom: Math.max(0, vs.zoom - ZOOM_DELTA) }));
          handled = true;
          break;
        case 'Home':
          // Reset to default view
          setViewState({
            longitude: mapCenter[0],
            latitude: mapCenter[1],
            zoom: mapZoom,
            pitch: 0,
            bearing: 0,
          });
          handled = true;
          break;
      }

      if (handled) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
    [mapCenter, mapZoom]
  );

  // Pre-compute grid data outside layers memo to avoid regenerating each frame
  const gridLines = useMemo(() => generateGrid(-12, 16, 44, 66, 2), []);

  const layers = useMemo(() => {
    const result = [];

    // ── Coordinate grid for spatial reference ──
    result.push(
      new PathLayer({
        id: 'grid-layer',
        data: gridLines,
        pickable: false,
        getPath: (d: { path: [number, number][] }) => d.path,
        getColor: [255, 255, 255, 18],
        getWidth: 1,
        widthMinPixels: 1,
        widthMaxPixels: 1,
      })
    );

    // ── Coastline outlines for spatial reference ──
    result.push(
      new PathLayer({
        id: 'coastline-layer',
        data: EUROPE_COASTLINE.map((coords, i) => ({ id: i, path: coords })),
        pickable: false,
        getPath: (d: { path: [number, number][] }) => d.path,
        getColor: [100, 140, 180, 80],
        getWidth: 1.5,
        widthMinPixels: 1,
        widthMaxPixels: 3,
        capRounded: true,
        jointRounded: true,
      })
    );

    if (showWeatherLayer) {
      result.push(
        new PolygonLayer({
          id: 'weather-layer',
          data: weatherSystems,
          pickable: true,
          stroked: true,
          filled: true,
          extruded: false,
          getPolygon: (d: Entity) => getWeatherPolygon(d),
          getFillColor: (d: Entity) => weatherColor((d.properties?.severity as string) ?? ''),
          getLineColor: (d: Entity) => weatherLineColor((d.properties?.severity as string) ?? ''),
          getLineWidth: 2,
          lineWidthMinPixels: 1,
          onClick: (info) => {
            if (info.object) selectEntity((info.object as Entity).id);
          },
          onHover: handleHover,
          updateTriggers: {
            getFillColor: [selectedEntityId],
          },
        })
      );
    }

    result.push(
      new ScatterplotLayer({
        id: 'port-layer',
        data: ports,
        pickable: true,
        radiusScale: 200,
        radiusMinPixels: 5,
        radiusMaxPixels: 60,
        getPosition: (d: Entity) => getPointCoords(d),
        getRadius: (d: Entity) => {
          const props = d.properties ?? {};
          const teu = (typeof props.total_teu === 'number' ? props.total_teu : 1_000_000);
          return Math.sqrt(teu / 1_000_000) * 5;
        },
        getFillColor: (d: Entity) =>
          congestionColor((d.properties ?? {}).congestion ?? 0),
        getLineColor: [255, 255, 255, 120],
        lineWidthMinPixels: 1,
        stroked: true,
        onClick: (info) => {
          if (info.object) selectEntity((info.object as Entity).id);
        },
        onHover: handleHover,
        updateTriggers: {
          getFillColor: [selectedEntityId],
        },
      })
    );

    if (showShipTracksLayer) {
      result.push(
        new PathLayer({
          id: 'ship-tracks-layer',
          data: shipsWithTracks,
          pickable: false,
          getPath: (d: Entity) => {
            const history = d.history ?? [];
            const pts = history
              .slice(-50)
              .filter((h) => {
                const c = h.geometry?.coordinates;
                return Array.isArray(c) && c.length >= 2 &&
                  typeof c[0] === 'number' && typeof c[1] === 'number' &&
                  isFinite(c[0]) && isFinite(c[1]);
              })
              .map((h) => [
                (h.geometry!.coordinates as number[])[0],
                (h.geometry!.coordinates as number[])[1],
              ] as [number, number]);
            return pts as unknown as [number, number][];
          },
          getColor: (d: Entity) =>
            d.id === selectedEntityId
              ? ([255, 200, 50, 200] as [number, number, number, number])
              : ([100, 160, 220, 80] as [number, number, number, number]),
          getWidth: (d: Entity) => (d.id === selectedEntityId ? 3 : 1.5),
          widthMinPixels: 1,
          widthMaxPixels: 6,
          capRounded: true,
          jointRounded: true,
          updateTriggers: {
            getColor: [selectedEntityId],
            getWidth: [selectedEntityId],
          },
        })
      );
    }

    result.push(
      new ScatterplotLayer({
        id: 'ship-layer',
        data: ships,
        pickable: true,
        radiusMinPixels: 3,
        radiusMaxPixels: 14,
        getPosition: (d: Entity) => getPointCoords(d),
        getRadius: (d: Entity) => (d.id === selectedEntityId ? 10 : 6),
        getFillColor: (d: Entity) =>
          shipColor(
            (d.properties ?? {}).speed ?? 0,
            d.id === selectedEntityId
          ),
        getLineColor: (d: Entity) =>
          d.id === selectedEntityId
            ? ([255, 255, 255, 255] as [number, number, number, number])
            : ([255, 255, 255, 80] as [number, number, number, number]),
        lineWidthMinPixels: 1,
        stroked: true,
        onClick: (info) => {
          if (info.object) selectEntity((info.object as Entity).id);
        },
        onHover: handleHover,
        updateTriggers: {
          getFillColor: [selectedEntityId],
          getRadius: [selectedEntityId],
          getLineColor: [selectedEntityId],
        },
      })
    );

    if (showHeatmapLayer && ships.length > 0) {
      result.push(
        new ScatterplotLayer({
          id: 'heatmap-layer',
          data: ships,
          pickable: false,
          radiusScale: 800,
          radiusMinPixels: 20,
          radiusMaxPixels: 120,
          getPosition: (d: Entity) => getPointCoords(d),
          getRadius: () => 3,
          getFillColor: [80, 140, 255, 35],
          stroked: false,
          opacity: 0.6,
        })
      );
    }

    return result;
  }, [
    ships,
    ports,
    weatherSystems,
    shipsWithTracks,
    selectedEntityId,
    showWeatherLayer,
    showShipTracksLayer,
    showHeatmapLayer,
    selectEntity,
    handleHover,
    gridLines,
  ]);

  return (
    <div
      ref={mapWrapperRef}
      className="relative flex-1 overflow-hidden"
      // Accessible label and role for screen readers
      role="application"
      aria-label="Interactive map showing entities. Use arrow keys to pan, + and - to zoom, Home to reset view."
      // Make the map focusable so keyboard events work
      tabIndex={0}
      onKeyDown={handleMapKeyDown}
      onFocus={() => setMapFocused(true)}
      onBlur={() => setMapFocused(false)}
    >
      {/* Keyboard focus ring indicator */}
      {mapFocused && (
        <div
          className="pointer-events-none absolute inset-0 z-50 ring-2 ring-blue-500 ring-inset"
          aria-hidden="true"
        />
      )}

      <DeckGL
        viewState={viewState}
        onViewStateChange={(e) => setViewState(e.viewState as typeof viewState)}
        controller={{ dragRotate: false, keyboard: false /* we handle keyboard ourselves */ }}
        layers={layers}
        getCursor={({ isDragging, isHovering }) =>
          isDragging ? 'grabbing' : isHovering ? 'pointer' : 'grab'
        }
        style={{ background: '#0a0e17' }}
      />

      {/* Tooltip — announced via existing hover; not separately announced as it's supplemental */}
      {tooltip && (
        <div
          className="pointer-events-none absolute z-50 rounded-none bg-gray-900/95 border border-gray-700 px-2.5 py-1.5 text-xs text-gray-100 shadow-lg whitespace-nowrap"
          style={{ left: tooltip.x + 12, top: tooltip.y - 10 }}
          aria-hidden="true"
        >
          {tooltip.content}
        </div>
      )}

      {/* Keyboard instructions — visible only on focus */}
      {mapFocused && (
        <div
          className="absolute top-3 left-3 z-10 rounded-none bg-gray-900/90 border border-blue-700 p-2 text-[10px] text-gray-300 backdrop-blur-sm"
          aria-live="polite"
          role="status"
        >
          <span className="font-semibold text-blue-300">Keyboard controls:</span>{' '}
          Arrow keys to pan · + / − to zoom · Home to reset
        </div>
      )}

      {/* Map Legend — aria-hidden since it duplicates tooltip info */}
      <div
        className="absolute bottom-12 right-3 z-10 rounded-none bg-gray-900/90 border border-gray-700 p-2.5 text-[10px] text-gray-400 space-y-1 backdrop-blur-sm"
        aria-label="Map legend"
        role="img"
      >
        <div className="text-gray-300 font-semibold text-[11px] mb-1.5" aria-hidden="true">Ships</div>
        <dl className="space-y-1">
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Green dot</dt>
            <dd className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-none bg-green-500 inline-block" aria-hidden="true" />
              <span>&lt; 10 kn</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Blue dot</dt>
            <dd className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-none bg-blue-400 inline-block" aria-hidden="true" />
              <span>10–20 kn</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Orange dot</dt>
            <dd className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-none bg-orange-400 inline-block" aria-hidden="true" />
              <span>&gt; 20 kn</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Red dot</dt>
            <dd className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-none bg-red-500 inline-block" aria-hidden="true" />
              <span>Selected</span>
            </dd>
          </div>
          <div className="border-t border-gray-700 mt-1.5 pt-1.5 text-gray-300 font-semibold text-[11px]" aria-hidden="true">
            Ports
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Green port dot</dt>
            <dd className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-none bg-green-400 inline-block opacity-80" aria-hidden="true" />
              <span>Low congestion</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Orange port dot</dt>
            <dd className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-none bg-orange-400 inline-block opacity-80" aria-hidden="true" />
              <span>Medium</span>
            </dd>
          </div>
          <div className="flex items-center gap-1.5">
            <dt className="sr-only">Red port dot</dt>
            <dd className="flex items-center gap-1.5">
              <span className="w-2.5 h-2.5 rounded-none bg-red-500 inline-block opacity-80" aria-hidden="true" />
              <span>High</span>
            </dd>
          </div>
        </dl>
      </div>

      {/* Entity count badge */}
      <div className="absolute top-3 right-3 z-10" aria-live="polite" aria-atomic="true">
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
