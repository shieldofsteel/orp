import React, { useMemo, useCallback, useState } from 'react';
import DeckGL from '@deck.gl/react';
import {
  ScatterplotLayer,
  PathLayer,
  PolygonLayer,
} from '@deck.gl/layers';
import { Map } from 'react-map-gl/maplibre';
import { useAppStore } from '../store/useAppStore';
import type { Entity } from '../types';
import 'maplibre-gl/dist/maplibre-gl.css';

const MAPLIBRE_STYLE = 'https://demotiles.maplibre.org/style.json';

// Speed color thresholds (RGBA)
function shipColor(speed: number, selected: boolean): [number, number, number, number] {
  if (selected) return [255, 50, 50, 255];
  if (speed > 20) return [255, 120, 0, 240];
  if (speed >= 10) return [30, 140, 255, 230];
  return [60, 200, 90, 220];
}

function congestionColor(congestion: number): [number, number, number, number] {
  // 0 = green, 0.5 = amber, 1 = red
  if (congestion > 0.75) return [220, 50, 50, 200];
  if (congestion > 0.4) return [255, 165, 0, 180];
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
  if (d.geometry?.type === 'Point' && Array.isArray(d.geometry.coordinates)) {
    return [d.geometry.coordinates[0] as number, d.geometry.coordinates[1] as number];
  }
  return [0, 0];
}

// Build polygon from WeatherSystem geometry or create approximate circle
function getWeatherPolygon(d: Entity): number[][] {
  if (d.geometry?.type === 'Polygon') {
    return (d.geometry.coordinates as number[][][])[0] ?? [];
  }
  // Fallback: generate circle around point
  if (d.geometry?.type === 'Point') {
    const [lon, lat] = d.geometry.coordinates as number[];
    const r = ((d.properties.radius_km as number) ?? 100) / 111; // rough degrees
    const pts: number[][] = [];
    for (let i = 0; i <= 32; i++) {
      const angle = (i / 32) * 2 * Math.PI;
      pts.push([lon + r * Math.cos(angle), lat + r * 0.6 * Math.sin(angle)]);
    }
    return pts;
  }
  return [];
}

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

  const allEntities = useMemo(() => Array.from(entities.values()), [entities]);

  const hasValidGeometry = (e: Entity): boolean => {
    if (!e.geometry) return false;
    if (e.geometry.type === 'Point' && Array.isArray(e.geometry.coordinates)) {
      const [lon, lat] = e.geometry.coordinates as number[];
      // Filter out entities at [0,0] (likely missing geometry)
      if (lon === 0 && lat === 0) return false;
      // Validate coordinate ranges
      if (lat < -90 || lat > 90 || lon < -180 || lon > 180) return false;
      return true;
    }
    if (e.geometry.type === 'Polygon') return true;
    return false;
  };

  const ships = useMemo(
    () => allEntities.filter((e) => e.type === 'Ship' && hasValidGeometry(e)),
    [allEntities]
  );

  const ports = useMemo(
    () => allEntities.filter((e) => e.type === 'Port' && hasValidGeometry(e)),
    [allEntities]
  );

  const weatherSystems = useMemo(
    () => allEntities.filter((e) => e.type === 'WeatherSystem' && hasValidGeometry(e)),
    [allEntities]
  );

  const shipsWithTracks = useMemo(
    () => ships.filter((s) => s.history && s.history.length > 1),
    [ships]
  );

  const handleHover = useCallback((info: { object?: Entity; x?: number; y?: number }) => {
    if (info.object && info.x != null && info.y != null) {
      const name = info.object.name ?? info.object.id;
      const type = info.object.type;
      const extra =
        type === 'Ship'
          ? ` · ${(info.object.properties.speed as number | undefined)?.toFixed(1) ?? '?'} kn`
          : type === 'Port'
          ? ` · TEU: ${((info.object.properties.total_teu as number | undefined) ?? 0).toLocaleString()}`
          : '';
      setTooltip({ x: info.x, y: info.y, content: `${name}${extra}` });
    } else {
      setTooltip(null);
    }
  }, []);

  const layers = useMemo(() => {
    const result = [];

    // ── 1. Weather PolygonLayer (rendered first, behind ships) ──
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
          getFillColor: (d: Entity) => weatherColor(d.properties.severity as string),
          getLineColor: (d: Entity) => weatherLineColor(d.properties.severity as string),
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

    // ── 2. Port ScatterplotLayer (size by TEU, color by congestion) ──
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
          const teu = (d.properties.total_teu as number | undefined) ?? 1_000_000;
          return Math.sqrt(teu / 1_000_000) * 5;
        },
        getFillColor: (d: Entity) =>
          congestionColor((d.properties.congestion as number | undefined) ?? 0),
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

    // ── 3. Ship Track PathLayer (last 50 positions) ──
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
              .filter((h) => Array.isArray(h.geometry?.coordinates) && (h.geometry!.coordinates as number[]).length >= 2)
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

    // ── 4. Ship IconLayer (course angle, speed color) ──
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
            (d.properties.speed as number | undefined) ?? 0,
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

    // ── 5. Density overlay (heatmap-style ScatterplotLayer, toggle-able) ──
    // Note: @deck.gl/aggregation-layers (HeatmapLayer) is not bundled; we approximate
    // density with a large-radius, low-opacity ScatterplotLayer.
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
  ]);

  return (
    <div className="relative flex-1 overflow-hidden">
      <DeckGL
        viewState={viewState}
        onViewStateChange={(e) => setViewState(e.viewState as typeof viewState)}
        controller={{ dragRotate: false }}
        layers={layers}
        getCursor={({ isDragging, isHovering }) =>
          isDragging ? 'grabbing' : isHovering ? 'pointer' : 'grab'
        }
      >
        <Map
          mapStyle={MAPLIBRE_STYLE}
          reuseMaps
          attributionControl={false}
        />
      </DeckGL>

      {/* Tooltip */}
      {tooltip && (
        <div
          className="pointer-events-none absolute z-50 rounded bg-gray-900/95 border border-gray-700 px-2.5 py-1.5 text-xs text-gray-100 shadow-lg whitespace-nowrap"
          style={{ left: tooltip.x + 12, top: tooltip.y - 10 }}
        >
          {tooltip.content}
        </div>
      )}

      {/* Map Legend */}
      <div className="absolute bottom-12 right-3 z-10 rounded-md bg-gray-900/90 border border-gray-700 p-2.5 text-[10px] text-gray-400 space-y-1 backdrop-blur-sm">
        <div className="text-gray-300 font-semibold text-[11px] mb-1.5">Ships</div>
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-green-500 inline-block" />
          <span>&lt; 10 kn</span>
        </div>
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-blue-400 inline-block" />
          <span>10–20 kn</span>
        </div>
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-orange-400 inline-block" />
          <span>&gt; 20 kn</span>
        </div>
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-red-500 inline-block" />
          <span>Selected</span>
        </div>
        <div className="border-t border-gray-700 mt-1.5 pt-1.5 text-gray-300 font-semibold text-[11px]">
          Ports
        </div>
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-green-400 inline-block opacity-80" />
          <span>Low congestion</span>
        </div>
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-orange-400 inline-block opacity-80" />
          <span>Medium</span>
        </div>
        <div className="flex items-center gap-1.5">
          <span className="w-2.5 h-2.5 rounded-full bg-red-500 inline-block opacity-80" />
          <span>High</span>
        </div>
      </div>

      {/* Entity count badge */}
      <div className="absolute top-3 right-3 z-10">
        <span className="rounded bg-gray-900/80 border border-gray-700 px-2 py-1 text-[10px] text-gray-400">
          {ships.length} ships · {ports.length} ports
        </span>
      </div>
    </div>
  );
};
