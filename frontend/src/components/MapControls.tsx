import React from 'react';

// ── Types ────────────────────────────────────────────────────────────────────

export type TileLayerType = 'osm' | 'satellite' | 'dark' | 'topo';

export interface LayerVisibility {
  ships: boolean;
  ports: boolean;
  weather: boolean;
  tracks: boolean;
  vectors: boolean;
  grid: boolean;
}

export interface MapControlsProps {
  // Tile layer
  activeTile: TileLayerType;
  onTileChange: (tile: TileLayerType) => void;

  // Layer visibility
  layers: LayerVisibility;
  onToggleLayer: (layer: keyof LayerVisibility) => void;

  // Tools
  measureActive: boolean;
  lassoActive: boolean;
  onToggleMeasure: () => void;
  onToggleLasso: () => void;
  onZoomToFit: () => void;

  // State / Info
  mouseCoords: [number, number] | null;
  entityCount: { ships: number; ports: number; weather: number };
}

// ── Constants ────────────────────────────────────────────────────────────────

const TILE_OPTIONS: { id: TileLayerType; label: string; abbr: string; accent: string }[] = [
  { id: 'osm',       label: 'Street',    abbr: 'STR', accent: '#3b82f6' },
  { id: 'satellite', label: 'Satellite', abbr: 'SAT', accent: '#22c55e' },
  { id: 'dark',      label: 'Dark',      abbr: 'DRK', accent: '#8b5cf6' },
  { id: 'topo',      label: 'Topo',      abbr: 'TPO', accent: '#f59e0b' },
];

const LAYER_DEFS: {
  key: keyof LayerVisibility;
  label: string;
  icon: React.ReactNode;
  color: string;
}[] = [
  {
    key: 'ships',
    label: 'Ships',
    icon: (
      <svg width="10" height="12" viewBox="0 0 10 14" fill="currentColor">
        <polygon points="5,0 9,14 5,10 1,14" />
      </svg>
    ),
    color: '#3b82f6',
  },
  {
    key: 'ports',
    label: 'Ports',
    icon: (
      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5">
        <circle cx="5" cy="5" r="4" />
      </svg>
    ),
    color: '#f97316',
  },
  {
    key: 'weather',
    label: 'Weather',
    icon: (
      <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor" opacity="0.7">
        <circle cx="5" cy="5" r="5" />
      </svg>
    ),
    color: '#ef4444',
  },
  {
    key: 'tracks',
    label: 'Tracks',
    icon: (
      <svg width="12" height="8" viewBox="0 0 12 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeDasharray="2 1.5">
        <path d="M1 7 Q6 1 11 4" />
      </svg>
    ),
    color: '#64a0dc',
  },
  {
    key: 'vectors',
    label: 'Vectors',
    icon: (
      <svg width="12" height="10" viewBox="0 0 12 10" fill="none" stroke="currentColor" strokeWidth="1.5">
        <line x1="1" y1="9" x2="10" y2="2" />
        <polyline points="6,1 11,2 10,7" />
      </svg>
    ),
    color: '#22c55e',
  },
  {
    key: 'grid',
    label: 'Grid',
    icon: (
      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1">
        <line x1="0" y1="3.3" x2="10" y2="3.3" />
        <line x1="0" y1="6.6" x2="10" y2="6.6" />
        <line x1="3.3" y1="0" x2="3.3" y2="10" />
        <line x1="6.6" y1="0" x2="6.6" y2="10" />
      </svg>
    ),
    color: '#6b7280',
  },
];

// ── Sub-components ────────────────────────────────────────────────────────────

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[8px] font-bold tracking-[0.15em] text-gray-600 uppercase px-1 mb-1">
      {children}
    </div>
  );
}

function Divider() {
  return <div className="border-t border-gray-800 my-1.5" />;
}

// ── Main Component ────────────────────────────────────────────────────────────

export const MapControls: React.FC<MapControlsProps> = ({
  activeTile,
  onTileChange,
  layers,
  onToggleLayer,
  measureActive,
  lassoActive,
  onToggleMeasure,
  onToggleLasso,
  onZoomToFit,
  mouseCoords,
  entityCount,
}) => {
  return (
    <div
      className="absolute top-2 right-2 z-[1000] flex flex-col gap-0 select-none"
      style={{ fontFamily: "'JetBrains Mono', 'Fira Code', 'Consolas', monospace" }}
      role="complementary"
      aria-label="Map controls panel"
    >
      {/* ── Main control panel ──────────────────────────────────────────── */}
      <div
        className="w-[148px] bg-gray-950/95 border border-gray-700/80 text-gray-400 text-[10px]"
        style={{ backdropFilter: 'blur(8px)' }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-2 py-1.5 border-b border-gray-800 bg-gray-900/60">
          <div className="flex items-center gap-1.5">
            <div className="w-1.5 h-1.5 bg-green-500 animate-pulse" />
            <span className="text-[9px] font-bold tracking-widest text-gray-300 uppercase">
              MAP CTRL
            </span>
          </div>
          <span className="text-[8px] text-gray-600 font-mono">
            {entityCount.ships + entityCount.ports}
          </span>
        </div>

        <div className="p-1.5 space-y-0">
          {/* Tile Layer */}
          <SectionLabel>Basemap</SectionLabel>
          <div className="grid grid-cols-2 gap-0.5">
            {TILE_OPTIONS.map((tile) => (
              <button
                key={tile.id}
                onClick={() => onTileChange(tile.id)}
                className={`
                  relative flex flex-col items-center justify-center
                  h-10 text-[8px] font-bold tracking-wider uppercase
                  border transition-all duration-100
                  ${activeTile === tile.id
                    ? 'border-gray-400 bg-gray-800 text-gray-100'
                    : 'border-gray-800 bg-gray-900/50 text-gray-600 hover:border-gray-700 hover:text-gray-400'}
                `}
                style={activeTile === tile.id ? { borderColor: tile.accent, color: tile.accent } : {}}
                title={`Switch to ${tile.label} basemap`}
              >
                <TilePreview tileId={tile.id} active={activeTile === tile.id} accent={tile.accent} />
                <span className="mt-0.5">{tile.abbr}</span>
              </button>
            ))}
          </div>

          <Divider />

          {/* Layer toggles */}
          <SectionLabel>Layers</SectionLabel>
          <div className="space-y-0.5">
            {LAYER_DEFS.map((def) => (
              <button
                key={def.key}
                onClick={() => onToggleLayer(def.key)}
                className={`
                  w-full flex items-center justify-between px-1.5 py-[3px]
                  border transition-all duration-100
                  ${layers[def.key]
                    ? 'border-gray-700 bg-gray-800/60 text-gray-200'
                    : 'border-gray-800 bg-transparent text-gray-600 hover:border-gray-700 hover:text-gray-500'}
                `}
                aria-pressed={layers[def.key]}
                title={`Toggle ${def.label} layer`}
              >
                <div className="flex items-center gap-1.5">
                  <span
                    className="flex items-center justify-center w-4"
                    style={{ color: layers[def.key] ? def.color : undefined }}
                  >
                    {def.icon}
                  </span>
                  <span className="text-[9px] tracking-wide">{def.label}</span>
                </div>
                <div
                  className={`w-2 h-2 border ${layers[def.key] ? 'bg-current border-transparent' : 'border-gray-700'}`}
                  style={{ color: layers[def.key] ? def.color : undefined }}
                />
              </button>
            ))}
          </div>

          <Divider />

          {/* Tools */}
          <SectionLabel>Tools</SectionLabel>
          <div className="space-y-0.5">
            {/* Measure */}
            <button
              onClick={onToggleMeasure}
              className={`
                w-full flex items-center gap-1.5 px-1.5 py-[3px]
                border transition-all duration-100
                ${measureActive
                  ? 'border-yellow-500/60 bg-yellow-900/20 text-yellow-300'
                  : 'border-gray-800 bg-transparent text-gray-600 hover:border-gray-700 hover:text-gray-400'}
              `}
              aria-pressed={measureActive}
              title="Measurement tool — click two points to measure distance"
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
                <line x1="0" y1="5" x2="10" y2="5" />
                <line x1="0" y1="3" x2="0" y2="7" />
                <line x1="10" y1="3" x2="10" y2="7" />
                <line x1="5" y1="4" x2="5" y2="6" />
              </svg>
              <span className="text-[9px] tracking-wide">Measure</span>
              {measureActive && (
                <span className="ml-auto text-[7px] text-yellow-500 font-bold">ON</span>
              )}
            </button>

            {/* Lasso */}
            <button
              onClick={onToggleLasso}
              className={`
                w-full flex items-center gap-1.5 px-1.5 py-[3px]
                border transition-all duration-100
                ${lassoActive
                  ? 'border-cyan-500/60 bg-cyan-900/20 text-cyan-300'
                  : 'border-gray-800 bg-transparent text-gray-600 hover:border-gray-700 hover:text-gray-400'}
              `}
              aria-pressed={lassoActive}
              title="Box select — hold Shift and drag to select entities"
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2" strokeDasharray="2 1">
                <rect x="1" y="1" width="8" height="8" />
              </svg>
              <span className="text-[9px] tracking-wide">Box Select</span>
              {lassoActive && (
                <span className="ml-auto text-[7px] text-cyan-500 font-bold">ON</span>
              )}
            </button>

            {/* Zoom to fit */}
            <button
              onClick={onZoomToFit}
              className="w-full flex items-center gap-1.5 px-1.5 py-[3px] border border-gray-800 bg-transparent text-gray-600 hover:border-gray-700 hover:text-gray-400 transition-all duration-100"
              title="Zoom to fit all entities in view"
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
                <rect x="1" y="1" width="8" height="8" />
                <line x1="3" y1="5" x2="7" y2="5" />
                <line x1="5" y1="3" x2="5" y2="7" />
                <polyline points="1,1 0,0 2,0 0,0 0,2" />
                <polyline points="9,1 10,0 8,0 10,0 10,2" />
                <polyline points="1,9 0,10 2,10 0,10 0,8" />
                <polyline points="9,9 10,10 8,10 10,10 10,8" />
              </svg>
              <span className="text-[9px] tracking-wide">Zoom to Fit</span>
            </button>
          </div>

          <Divider />

          {/* Entity counts */}
          <SectionLabel>Entities</SectionLabel>
          <div className="grid grid-cols-3 gap-0.5 text-center">
            <div className="bg-gray-900/60 border border-gray-800 py-1">
              <div className="text-[11px] font-bold text-blue-400">{entityCount.ships}</div>
              <div className="text-[7px] text-gray-600 tracking-wider">SHIPS</div>
            </div>
            <div className="bg-gray-900/60 border border-gray-800 py-1">
              <div className="text-[11px] font-bold text-orange-400">{entityCount.ports}</div>
              <div className="text-[7px] text-gray-600 tracking-wider">PORTS</div>
            </div>
            <div className="bg-gray-900/60 border border-gray-800 py-1">
              <div className="text-[11px] font-bold text-red-400">{entityCount.weather}</div>
              <div className="text-[7px] text-gray-600 tracking-wider">WX</div>
            </div>
          </div>
        </div>
      </div>

      {/* ── Coordinates display ──────────────────────────────────────────── */}
      <div
        className="mt-0.5 bg-gray-950/90 border border-gray-700/60 px-2 py-1.5 text-[9px] font-mono"
        style={{ backdropFilter: 'blur(8px)' }}
        aria-live="polite"
        aria-atomic="true"
        aria-label="Current mouse position coordinates"
      >
        {mouseCoords ? (
          <div className="flex items-center gap-2">
            <span className="text-gray-600">LAT</span>
            <span className="text-green-400 tabular-nums w-[72px] inline-block">
              {mouseCoords[0].toFixed(4)}°
            </span>
            <span className="text-gray-600">LON</span>
            <span className="text-green-400 tabular-nums w-[72px] inline-block">
              {mouseCoords[1].toFixed(4)}°
            </span>
          </div>
        ) : (
          <div className="text-gray-700 tracking-wider">
            LAT ——.——— · LON ——.———
          </div>
        )}
      </div>
    </div>
  );
};

// ── Tile preview mini-map ────────────────────────────────────────────────────

function TilePreview({ tileId, active, accent }: { tileId: TileLayerType; active: boolean; accent: string }) {
  // Simple SVG-based visual representation of each tile type
  const previews: Record<TileLayerType, React.ReactNode> = {
    osm: (
      <svg width="28" height="14" viewBox="0 0 28 14">
        <rect width="28" height="14" fill="#1a2433" />
        <path d="M0 8 Q7 4 14 7 Q21 10 28 5" fill="none" stroke="#3b82f6" strokeWidth="1" opacity="0.7" />
        <rect x="4" y="4" width="4" height="3" fill="#374151" opacity="0.8" />
        <rect x="20" y="6" width="5" height="4" fill="#374151" opacity="0.8" />
        <line x1="0" y1="11" x2="28" y2="11" stroke="#4b5563" strokeWidth="0.5" />
      </svg>
    ),
    satellite: (
      <svg width="28" height="14" viewBox="0 0 28 14">
        <rect width="28" height="14" fill="#0a1a0a" />
        <rect x="3" y="3" width="6" height="5" fill="#1a3a1a" opacity="0.9" />
        <rect x="12" y="1" width="8" height="6" fill="#0f2a0f" opacity="0.9" />
        <rect x="18" y="7" width="7" height="5" fill="#152a15" opacity="0.9" />
        <path d="M0 9 Q14 7 28 10" fill="none" stroke="#1a4a1a" strokeWidth="0.8" />
      </svg>
    ),
    dark: (
      <svg width="28" height="14" viewBox="0 0 28 14">
        <rect width="28" height="14" fill="#080c14" />
        <path d="M0 9 Q7 7 14 8 Q21 9 28 7" fill="none" stroke="#1e3a5f" strokeWidth="1" />
        <rect x="5" y="5" width="3" height="2" fill="#1a2a3a" opacity="0.8" />
        <circle cx="20" cy="7" r="2" fill="none" stroke="#1e3a5f" strokeWidth="0.5" />
        <line x1="0" y1="4" x2="28" y2="4" stroke="#0f1e2f" strokeWidth="0.5" />
        <line x1="0" y1="10" x2="28" y2="10" stroke="#0f1e2f" strokeWidth="0.5" />
      </svg>
    ),
    topo: (
      <svg width="28" height="14" viewBox="0 0 28 14">
        <rect width="28" height="14" fill="#1a1508" />
        <path d="M0 10 Q5 4 10 8 Q15 12 20 6 Q24 2 28 5" fill="none" stroke="#8b6914" strokeWidth="0.8" opacity="0.8" />
        <path d="M0 12 Q5 6 10 10 Q15 13 20 8 Q24 4 28 7" fill="none" stroke="#6b5010" strokeWidth="0.5" opacity="0.6" />
        <path d="M2 8 Q7 2 12 6" fill="none" stroke="#8b6914" strokeWidth="0.5" opacity="0.5" />
      </svg>
    ),
  };

  return (
    <div
      className="overflow-hidden"
      style={{
        outline: active ? `1px solid ${accent}` : 'none',
        opacity: active ? 1 : 0.6,
      }}
    >
      {previews[tileId]}
    </div>
  );
}
