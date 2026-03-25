import React, { useMemo, useCallback } from 'react';
import DeckGL from '@deck.gl/react';
import { IconLayer, ScatterplotLayer, PathLayer, PolygonLayer, HeatmapLayer } from '@deck.gl/layers';
import { Map } from 'react-map-gl/maplibre';
import { useAppStore } from '../store/useAppStore';
import type { Entity } from '../types';
import 'maplibre-gl/dist/maplibre-gl.css';

const MAPLIBRE_STYLE = 'https://basemaps.cartocdn.com/gl/dark-matter-gl-style/style.json';

export const MapView: React.FC = () => {
  const entities = useAppStore((s) => s.entities);
  const selectedEntityId = useAppStore((s) => s.selectedEntityId);
  const mapCenter = useAppStore((s) => s.mapCenter);
  const mapZoom = useAppStore((s) => s.mapZoom);
  const showWeatherLayer = useAppStore((s) => s.showWeatherLayer);
  const showShipTracksLayer = useAppStore((s) => s.showShipTracksLayer);
  const showHeatmapLayer = useAppStore((s) => s.showHeatmapLayer);
  const selectEntity = useAppStore((s) => s.selectEntity);

  const [viewState, setViewState] = React.useState({
    longitude: mapCenter[0],
    latitude: mapCenter[1],
    zoom: mapZoom,
    pitch: 0,
    bearing: 0,
  });

  const allEntities = useMemo(() => Array.from(entities.values()), [entities]);

  const ships = useMemo(
    () => allEntities.filter((e) => e.type === 'Ship'),
    [allEntities]
  );

  const ports = useMemo(
    () => allEntities.filter((e) => e.type === 'Port'),
    [allEntities]
  );

  const weatherSystems = useMemo(
    () => allEntities.filter((e) => e.type === 'WeatherSystem'),
    [allEntities]
  );

  const getPointCoords = useCallback(
    (d: Entity): [number, number] => {
      if (d.geometry?.type === 'Point' && Array.isArray(d.geometry.coordinates)) {
        return [d.geometry.coordinates[0] as number, d.geometry.coordinates[1] as number];
      }
      return [0, 0];
    },
    []
  );

  const layers = useMemo(() => {
    const result = [];

    // Ship icon layer — using ScatterplotLayer as a reliable fallback (no icon atlas required)
    result.push(
      new ScatterplotLayer({
        id: 'ship-layer',
        data: ships,
        pickable: true,
        radiusScale: 1,
        radiusMinPixels: 4,
        radiusMaxPixels: 16,
        getPosition: (d: Entity) => getPointCoords(d),
        getRadius: (d: Entity) => (d.id === selectedEntityId ? 12 : 8),
        getFillColor: (d: Entity) => {
          if (d.id === selectedEntityId) return [255, 50, 50, 255];
          const speed = (d.properties.speed as number) ?? 0;
          if (speed > 20) return [255, 100, 0, 255];
          if (speed > 10) return [0, 150, 255, 255];
          return [100, 200, 100, 255];
        },
        getLineColor: [255, 255, 255, 180],
        lineWidthMinPixels: 1,
        onClick: (info) => {
          if (info.object) selectEntity((info.object as Entity).id);
        },
        updateTriggers: {
          getFillColor: [selectedEntityId],
          getRadius: [selectedEntityId],
        },
      })
    );

    // Port layer
    result.push(
      new ScatterplotLayer({
        id: 'port-layer',
        data: ports,
        pickable: true,
        radiusScale: 100,
        radiusMinPixels: 6,
        radiusMaxPixels: 60,
        getPosition: (d: Entity) => getPointCoords(d),
        getRadius: (d: Entity) => {
          const cap = (d.properties.capacity as number) ?? (d.properties.total_teu as number) ?? 1000;
          return Math.log(cap + 1) / 10;
        },
        getFillColor: (d: Entity) => {
          const congestion = (d.properties.congestion as number) ?? 0;
          if (congestion > 0.8) return [255, 0, 0, 200];
          if (congestion > 0.5) return [255, 200, 0, 200];
          return [0, 200, 0, 200];
        },
        getLineColor: [255, 255, 255, 100],
        lineWidthMinPixels: 1,
        onClick: (info) => {
          if (info.object) selectEntity((info.object as Entity).id);
        },
      })
    );

    // Weather polygon layer
    if (showWeatherLayer && weatherSystems.length > 0) {
      result.push(
        new PolygonLayer({
          id: 'weather-layer',
          data: weatherSystems,
          pickable: true,
          stroked: true,
          filled: true,
          extruded: false,
          getPolygon: (d: Entity) => {
            if (d.geometry?.type === 'Polygon' && Array.isArray(d.geometry.coordinates)) {
              return d.geometry.coordinates[0] as number[][];
            }
            // Generate a circle polygon around Point geometry for weather display
            if (d.geometry?.type === 'Point') {
              const [lon, lat] = d.geometry.coordinates as number[];
              const radius = (d.properties.radius_km as number) ?? 100;
              const degPerKm = 1 / 111;
              const points: number[][] = [];
              for (let i = 0; i <= 36; i++) {
                const angle = (i * 10 * Math.PI) / 180;
                points.push([
                  lon + radius * degPerKm * Math.cos(angle),
                  lat + radius * degPerKm * Math.sin(angle),
                ]);
              }
              return points;
            }
            return [];
          },
          getFillColor: (d: Entity) => {
            const severity = d.properties.severity as string;
            switch (severity) {
              case 'critical': return [200, 0, 0, 100];
              case 'high':
              case 'warning': return [255, 100, 0, 100];
              case 'moderate': return [255, 200, 0, 80];
              default: return [100, 200, 255, 60];
            }
          },
          getLineColor: [200, 200, 200, 150],
          getLineWidth: 2,
          lineWidthMinPixels: 1,
          onClick: (info) => {
            if (info.object) selectEntity((info.object as Entity).id);
          },
        })
      );
    }

    // Ship tracks path layer
    if (showShipTracksLayer) {
      const shipsWithHistory = ships.filter(
        (s) => s.history && s.history.length > 1
      );
      if (shipsWithHistory.length > 0) {
        result.push(
          new PathLayer({
            id: 'ship-tracks-layer',
            data: shipsWithHistory,
            pickable: false,
            getPath: (d: Entity) =>
              (d.history ?? [])
                .filter((h) => h.geometry?.coordinates)
                .slice(-50)
                .map((h) => [
                  h.geometry!.coordinates[0],
                  h.geometry!.coordinates[1],
                ]),
            getColor: (d: Entity) =>
              d.id === selectedEntityId ? [255, 50, 50, 150] : [150, 150, 150, 80],
            getWidth: (d: Entity) => (d.id === selectedEntityId ? 3 : 1),
            widthMinPixels: 1,
            widthMaxPixels: 5,
            capRounded: true,
            jointRounded: true,
            updateTriggers: {
              getColor: [selectedEntityId],
              getWidth: [selectedEntityId],
            },
          })
        );
      }
    }

    // Heatmap layer
    if (showHeatmapLayer && ships.length > 0) {
      result.push(
        new HeatmapLayer({
          id: 'heatmap-layer',
          data: ships,
          getPosition: (d: Entity) => getPointCoords(d),
          getWeight: 1,
          radiusPixels: 50,
          intensity: 0.8,
          threshold: 0.05,
          colorRange: [
            [26, 26, 127, 255],
            [55, 48, 163, 255],
            [63, 0, 250, 255],
            [255, 0, 0, 255],
          ],
          opacity: 0.3,
        })
      );
    }

    return result;
  }, [ships, ports, weatherSystems, selectedEntityId, showWeatherLayer, showShipTracksLayer, showHeatmapLayer, selectEntity, getPointCoords]);

  const onViewStateChange = useCallback(
    ({ viewState: vs }: { viewState: Record<string, unknown> }) => {
      setViewState(vs as typeof viewState);
    },
    []
  );

  return (
    <div className="relative flex-1 h-full">
      <DeckGL
        viewState={viewState}
        onViewStateChange={onViewStateChange}
        controller={true}
        layers={layers}
        getTooltip={({ object }: { object?: Entity }) => {
          if (!object) return null;
          return {
            text: `${object.name ?? object.id}\n${object.type}${
              object.properties.speed != null
                ? `\nSpeed: ${object.properties.speed} kn`
                : ''
            }`,
            style: {
              backgroundColor: 'rgba(0,0,0,0.8)',
              color: '#fff',
              fontSize: '12px',
              padding: '6px 10px',
              borderRadius: '4px',
            },
          };
        }}
      >
        <Map mapStyle={MAPLIBRE_STYLE} />
      </DeckGL>

      {/* Layer toggle controls */}
      <div className="absolute top-3 right-3 flex flex-col gap-1.5 z-10">
        <LayerToggle
          label="Weather"
          active={showWeatherLayer}
          onClick={useAppStore.getState().toggleWeatherLayer}
        />
        <LayerToggle
          label="Tracks"
          active={showShipTracksLayer}
          onClick={useAppStore.getState().toggleShipTracksLayer}
        />
        <LayerToggle
          label="Heatmap"
          active={showHeatmapLayer}
          onClick={useAppStore.getState().toggleHeatmapLayer}
        />
      </div>
    </div>
  );
};

function LayerToggle({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`px-2.5 py-1 text-xs rounded font-medium transition-colors ${
        active
          ? 'bg-blue-600 text-white'
          : 'bg-gray-800 text-gray-400 hover:bg-gray-700'
      }`}
    >
      {label}
    </button>
  );
}
