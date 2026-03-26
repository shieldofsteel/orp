/**
 * MapControls — Dynamic layer toggles driven by live entity type registry.
 * Zero hardcoded entity type references.
 */
import React from 'react';
import type { EntityTypeConfig } from '../types';

// ── Types ────────────────────────────────────────────────────────────────────

export type TileLayerType = 'osm' | 'satellite' | 'dark' | 'topo';

/** Dynamic layer visibility: keyed by entity type (lowercase) + utility layers */
export type LayerVisibility = Record<string, boolean>;

export interface MapControlsProps {
  activeTile: TileLayerType;
  onTileChange: (tile: TileLayerType) => void;

  /** All discovered entity type configs */
  entityTypes: EntityTypeConfig[];
  /** Entity count per type key */
  entityCounts: Record<string, number>;

  layers: LayerVisibility;
  onToggleLayer: (layer: string) => void;

  measureActive: boolean;
  lassoActive: boolean;
  onToggleMeasure: () => void;
  onToggleLasso: () => void;
  onZoomToFit: () => void;

  mouseCoords: [number, number] | null;
}

// ── Constants ────────────────────────────────────────────────────────────────

const TILE_OPTIONS: { id: TileLayerType; label: string; abbr: string; accent: string }[] = [
  { id: 'osm',       label: 'Street',    abbr: 'STR', accent: '#3b82f6' },
  { id: 'satellite', label: 'Satellite', abbr: 'SAT', accent: '#22c55e' },
  { id: 'dark',      label: 'Dark',      abbr: 'DRK', accent: '#8b5cf6' },
  { id: 'topo',      label: 'Topo',      abbr: 'TPO', accent: '#f59e0b' },
];

// Utility-only layers (not entity-type driven)
const UTILITY_LAYERS = [
  {
    key: 'tracks',
    label: 'Tracks',
    color: '#64a0dc',
    icon: (
      <svg width="12" height="8" viewBox="0 0 12 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeDasharray="2 1.5">
        <path d="M1 7 Q6 1 11 4" />
      </svg>
    ),
  },
  {
    key: 'vectors',
    label: 'Vectors',
    color: '#22c55e',
    icon: (
      <svg width="12" height="10" viewBox="0 0 12 10" fill="none" stroke="currentColor" strokeWidth="1.5">
        <line x1="1" y1="9" x2="10" y2="2" />
        <polyline points="6,1 11,2 10,7" />
      </svg>
    ),
  },
  {
    key: 'grid',
    label: 'Grid',
    color: '#6b7280',
    icon: (
      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1">
        <line x1="0" y1="3.3" x2="10" y2="3.3" />
        <line x1="0" y1="6.6" x2="10" y2="6.6" />
        <line x1="3.3" y1="0" x2="3.3" y2="10" />
        <line x1="6.6" y1="0" x2="6.6" y2="10" />
      </svg>
    ),
  },
];

// ── Entity type icon renderer ────────────────────────────────────────────────

function EntityTypeIcon({ config }: { config: EntityTypeConfig }) {
  if (config.iconIsEmoji && config.icon) {
    return <span style={{ fontSize: 10, lineHeight: 1 }}>{config.icon}</span>;
  }

  const [r, g, b] = config.color;

  switch (config.markerStyle) {
    case 'arrow':
      return (
        <svg width="10" height="12" viewBox="0 0 10 14" fill="currentColor">
          <polygon points="5,0 9,14 5,10 1,14" />
        </svg>
      );
    case 'plane':
      return (
        <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
          <path d="M12 2L8 10H2l3 3-2 7 9-5 9 5-2-7 3-3h-6z" />
        </svg>
      );
    case 'circle':
      return (
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5">
          <circle cx="5" cy="5" r="4" />
        </svg>
      );
    case 'diamond':
      return (
        <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor">
          <polygon points="5,0 10,5 5,10 0,5" />
        </svg>
      );
    case 'square':
      return (
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5">
          <rect x="1" y="1" width="8" height="8" />
        </svg>
      );
    case 'cross':
      return (
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="2">
          <line x1="5" y1="0" x2="5" y2="10" />
          <line x1="0" y1="5" x2="10" y2="5" />
        </svg>
      );
    default: // dot
      return (
        <svg width="8" height="8" viewBox="0 0 8 8" fill={`rgb(${r},${g},${b})`}>
          <circle cx="4" cy="4" r="4" />
        </svg>
      );
  }
}

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
  entityTypes,
  entityCounts,
  layers,
  onToggleLayer,
  measureActive,
  lassoActive,
  onToggleMeasure,
  onToggleLasso,
  onZoomToFit,
  mouseCoords,
}) => {
  const totalEntities = Object.values(entityCounts).reduce((a, b) => a + b, 0);

  return (
    <div
      className="absolute top-2 right-2 z-[1000] flex flex-col gap-0 select-none"
      style={{ fontFamily: "'JetBrains Mono', 'Fira Code', 'Consolas', monospace" }}
      role="complementary"
      aria-label="Map controls panel"
    >
      {/* ── Main control panel ──────────────────────────────────────────── */}
      <div
        className="w-[152px] bg-gray-950/95 border border-gray-700/80 text-gray-400 text-[10px]"
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
          <span className="text-[8px] text-gray-600 font-mono">{totalEntities}</span>
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

          {/* Entity type layer toggles — fully dynamic */}
          <SectionLabel>Entity Layers</SectionLabel>
          <div className="space-y-0.5">
            {entityTypes.length === 0 ? (
              <div className="text-[9px] text-gray-700 px-1 py-0.5">No data yet</div>
            ) : (
              entityTypes.map((cfg) => {
                const active = layers[cfg.type] !== false; // default on
                const count = entityCounts[cfg.type] ?? 0;
                const [r, g, b] = cfg.color;
                const colorHex = cfg.colorHex;
                return (
                  <button
                    key={cfg.type}
                    onClick={() => onToggleLayer(cfg.type)}
                    className={`
                      w-full flex items-center justify-between px-1.5 py-[3px]
                      border transition-all duration-100
                      ${active
                        ? 'border-gray-700 bg-gray-800/60 text-gray-200'
                        : 'border-gray-800 bg-transparent text-gray-600 hover:border-gray-700 hover:text-gray-500'}
                    `}
                    aria-pressed={active}
                    title={`Toggle ${cfg.label} layer (${count})`}
                  >
                    <div className="flex items-center gap-1.5 min-w-0">
                      <span
                        className="flex items-center justify-center w-4 flex-shrink-0"
                        style={{ color: active ? colorHex : undefined }}
                      >
                        <EntityTypeIcon config={cfg} />
                      </span>
                      <span className="text-[9px] tracking-wide truncate">{cfg.label}</span>
                    </div>
                    <div className="flex items-center gap-1 flex-shrink-0">
                      {count > 0 && (
                        <span
                          className="text-[8px] font-mono"
                          style={{ color: active ? colorHex : '#4b5563' }}
                        >
                          {count}
                        </span>
                      )}
                      <div
                        className={`w-2 h-2 border ${active ? 'bg-current border-transparent' : 'border-gray-700'}`}
                        style={{ color: active ? colorHex : undefined }}
                      />
                    </div>
                  </button>
                );
              })
            )}
          </div>

          <Divider />

          {/* Utility layers */}
          <SectionLabel>Overlays</SectionLabel>
          <div className="space-y-0.5">
            {UTILITY_LAYERS.map((def) => {
              const active = layers[def.key] !== false;
              return (
                <button
                  key={def.key}
                  onClick={() => onToggleLayer(def.key)}
                  className={`
                    w-full flex items-center justify-between px-1.5 py-[3px]
                    border transition-all duration-100
                    ${active
                      ? 'border-gray-700 bg-gray-800/60 text-gray-200'
                      : 'border-gray-800 bg-transparent text-gray-600 hover:border-gray-700 hover:text-gray-500'}
                  `}
                  aria-pressed={active}
                >
                  <div className="flex items-center gap-1.5">
                    <span
                      className="flex items-center justify-center w-4"
                      style={{ color: active ? def.color : undefined }}
                    >
                      {def.icon}
                    </span>
                    <span className="text-[9px] tracking-wide">{def.label}</span>
                  </div>
                  <div
                    className={`w-2 h-2 border ${active ? 'bg-current border-transparent' : 'border-gray-700'}`}
                    style={{ color: active ? def.color : undefined }}
                  />
                </button>
              );
            })}
          </div>

          <Divider />

          {/* Tools */}
          <SectionLabel>Tools</SectionLabel>
          <div className="space-y-0.5">
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
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
                <line x1="0" y1="5" x2="10" y2="5" />
                <line x1="0" y1="3" x2="0" y2="7" />
                <line x1="10" y1="3" x2="10" y2="7" />
                <line x1="5" y1="4" x2="5" y2="6" />
              </svg>
              <span className="text-[9px] tracking-wide">Measure</span>
              {measureActive && <span className="ml-auto text-[7px] text-yellow-500 font-bold">ON</span>}
            </button>

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
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2" strokeDasharray="2 1">
                <rect x="1" y="1" width="8" height="8" />
              </svg>
              <span className="text-[9px] tracking-wide">Box Select</span>
              {lassoActive && <span className="ml-auto text-[7px] text-cyan-500 font-bold">ON</span>}
            </button>

            <button
              onClick={onZoomToFit}
              className="w-full flex items-center gap-1.5 px-1.5 py-[3px] border border-gray-800 bg-transparent text-gray-600 hover:border-gray-700 hover:text-gray-400 transition-all duration-100"
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
                <rect x="1" y="1" width="8" height="8" />
                <line x1="3" y1="5" x2="7" y2="5" />
                <line x1="5" y1="3" x2="5" y2="7" />
              </svg>
              <span className="text-[9px] tracking-wide">Zoom to Fit</span>
            </button>
          </div>

          {/* Entity count summary */}
          {entityTypes.length > 0 && (
            <>
              <Divider />
              <SectionLabel>Summary</SectionLabel>
              <div
                className="grid gap-0.5"
                style={{ gridTemplateColumns: `repeat(${Math.min(entityTypes.length, 3)}, 1fr)` }}
              >
                {entityTypes.map((cfg) => (
                  <div key={cfg.type} className="bg-gray-900/60 border border-gray-800 py-1 text-center">
                    <div
                      className="text-[11px] font-bold"
                      style={{ color: cfg.colorHex }}
                    >
                      {entityCounts[cfg.type] ?? 0}
                    </div>
                    <div className="text-[7px] text-gray-600 tracking-wider truncate px-0.5">
                      {cfg.label.toUpperCase().slice(0, 5)}
                    </div>
                  </div>
                ))}
              </div>
            </>
          )}
        </div>
      </div>

      {/* ── Coordinates display ──────────────────────────────────────────── */}
      <div
        className="mt-0.5 bg-gray-950/90 border border-gray-700/60 px-2 py-1.5 text-[9px] font-mono w-[152px]"
        style={{ backdropFilter: 'blur(8px)' }}
        aria-live="polite"
      >
        {mouseCoords ? (
          <div className="flex items-center gap-2">
            <span className="text-gray-600">LAT</span>
            <span className="text-green-400 tabular-nums">{mouseCoords[0].toFixed(4)}°</span>
            <span className="text-gray-600">LON</span>
            <span className="text-green-400 tabular-nums">{mouseCoords[1].toFixed(4)}°</span>
          </div>
        ) : (
          <div className="text-gray-700 tracking-wider">LAT ——.——— · LON ——.———</div>
        )}
      </div>
    </div>
  );
};

// ── Tile preview mini-map ────────────────────────────────────────────────────

function TilePreview({ tileId, active, accent }: { tileId: TileLayerType; active: boolean; accent: string }) {
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
      </svg>
    ),
    dark: (
      <svg width="28" height="14" viewBox="0 0 28 14">
        <rect width="28" height="14" fill="#080c14" />
        <path d="M0 9 Q7 7 14 8 Q21 9 28 7" fill="none" stroke="#1e3a5f" strokeWidth="1" />
        <rect x="5" y="5" width="3" height="2" fill="#1a2a3a" opacity="0.8" />
        <circle cx="20" cy="7" r="2" fill="none" stroke="#1e3a5f" strokeWidth="0.5" />
      </svg>
    ),
    topo: (
      <svg width="28" height="14" viewBox="0 0 28 14">
        <rect width="28" height="14" fill="#1a1508" />
        <path d="M0 10 Q5 4 10 8 Q15 12 20 6 Q24 2 28 5" fill="none" stroke="#8b6914" strokeWidth="0.8" opacity="0.8" />
        <path d="M0 12 Q5 6 10 10 Q15 13 20 8 Q24 4 28 7" fill="none" stroke="#6b5010" strokeWidth="0.5" opacity="0.6" />
      </svg>
    ),
  };

  return (
    <div style={{ outline: active ? `1px solid ${accent}` : 'none', opacity: active ? 1 : 0.6 }}>
      {previews[tileId]}
    </div>
  );
}
