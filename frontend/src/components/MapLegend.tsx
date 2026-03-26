/**
 * MapLegend — Fully dynamic legend auto-generated from entity type registry.
 * No hardcoded types. Add a new entity type → legend updates automatically.
 */
import React from 'react';
import type { EntityTypeConfig } from '../types';

interface MapLegendProps {
  entityTypes: EntityTypeConfig[];
  measureActive: boolean;
  lassoActive: boolean;
}

// ── Icon renderer ─────────────────────────────────────────────────────────────

function LegendIcon({ config }: { config: EntityTypeConfig }) {
  const hex = config.colorHex;

  if (config.iconIsEmoji && config.icon) {
    return <span style={{ fontSize: 11, lineHeight: 1 }}>{config.icon}</span>;
  }

  switch (config.markerStyle) {
    case 'arrow':
      return (
        <svg width="10" height="12" viewBox="0 0 10 14" style={{ color: hex }} fill="currentColor">
          <polygon points="5,0 9,14 5,10 1,14" />
        </svg>
      );
    case 'plane':
      return (
        <svg width="12" height="12" viewBox="0 0 24 24" style={{ color: hex }} fill="currentColor">
          <path d="M12 2L8 10H2l3 3-2 7 9-5 9 5-2-7 3-3h-6z" />
        </svg>
      );
    case 'circle':
      return (
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke={hex} strokeWidth="1.5">
          <circle cx="5" cy="5" r="4" />
        </svg>
      );
    case 'diamond':
      return (
        <svg width="10" height="10" viewBox="0 0 10 10" fill={hex}>
          <polygon points="5,0 10,5 5,10 0,5" />
        </svg>
      );
    case 'square':
      return (
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke={hex} strokeWidth="1.5">
          <rect x="1" y="1" width="8" height="8" />
        </svg>
      );
    case 'cross':
      return (
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke={hex} strokeWidth="2">
          <line x1="5" y1="0" x2="5" y2="10" />
          <line x1="0" y1="5" x2="10" y2="5" />
        </svg>
      );
    default: // dot
      return (
        <svg width="8" height="8" viewBox="0 0 8 8" fill={hex}>
          <circle cx="4" cy="4" r="4" />
        </svg>
      );
  }
}

// ── Speed color scale (for types with speedField) ─────────────────────────────

function SpeedScale({ config }: { config: EntityTypeConfig }) {
  if (!config.speedField) return null;
  return (
    <div className="mt-1 ml-3.5 space-y-0.5">
      {[
        { color: '#3cc85a', label: 'Slow' },
        { color: '#1e8cff', label: 'Medium' },
        { color: '#ff7800', label: 'Fast' },
        { color: '#ff3232', label: 'Selected' },
      ].map(({ color, label }) => (
        <div key={label} className="flex items-center gap-1.5">
          <LegendIcon config={{ ...config, colorHex: color, color: [0, 0, 0] }} />
          <span className="text-gray-600 text-[8px]">{label}</span>
        </div>
      ))}
    </div>
  );
}

// ── Altitude scale (for types with altitudeField) ────────────────────────────

function AltitudeScale({ config }: { config: EntityTypeConfig }) {
  if (!config.altitudeField) return null;
  return (
    <div className="mt-1 ml-3.5 space-y-0.5">
      {[
        { color: '#22c55e', label: 'Low alt' },
        { color: '#3b82f6', label: 'Mid alt' },
        { color: '#a855f7', label: 'High alt' },
      ].map(({ color, label }) => (
        <div key={label} className="flex items-center gap-1.5">
          <LegendIcon config={{ ...config, colorHex: color, color: [0, 0, 0] }} />
          <span className="text-gray-600 text-[8px]">{label}</span>
        </div>
      ))}
    </div>
  );
}

// ── Main component ────────────────────────────────────────────────────────────

export const MapLegend: React.FC<MapLegendProps> = ({
  entityTypes,
  measureActive,
  lassoActive,
}) => {
  if (entityTypes.length === 0 && !measureActive && !lassoActive) return null;

  return (
    <div
      className="absolute bottom-8 left-2 z-[1000] bg-gray-950/90 border border-gray-700/70 p-2 text-[9px]"
      style={{
        fontFamily: "'JetBrains Mono','Fira Code',monospace",
        backdropFilter: 'blur(8px)',
        maxHeight: 'calc(100vh - 100px)',
        overflowY: 'auto',
        scrollbarWidth: 'none',
      }}
      aria-label="Map legend"
    >
      {entityTypes.map((config, idx) => (
        <React.Fragment key={config.type}>
          {idx > 0 && <div className="border-t border-gray-800 my-1.5" />}

          {/* Type header */}
          <div className="text-[8px] font-bold tracking-widest uppercase mb-1" style={{ color: config.colorHex }}>
            {config.label}
          </div>

          {/* Main marker row */}
          <div className="flex items-center gap-1.5 mb-0.5">
            <span className="w-4 flex items-center justify-center flex-shrink-0">
              <LegendIcon config={config} />
            </span>
            <span className="text-gray-500 text-[8px]">{config.label} marker</span>
          </div>

          {/* Area indicator */}
          {config.isArea && (
            <div className="flex items-center gap-1.5 mb-0.5">
              <span
                className="w-4 h-3 border flex-shrink-0"
                style={{
                  borderColor: config.colorHex,
                  background: `${config.colorHex}22`,
                }}
              />
              <span className="text-gray-500 text-[8px]">Zone boundary</span>
            </div>
          )}

          {/* Speed-based coloring */}
          {config.speedField && <SpeedScale config={config} />}

          {/* Altitude-based coloring */}
          {config.altitudeField && <AltitudeScale config={config} />}

          {/* Track line */}
          {config.showTrack && (
            <div className="flex items-center gap-1.5 mt-0.5">
              <svg width="16" height="6" viewBox="0 0 16 6" className="flex-shrink-0">
                <path d="M0 5 Q8 1 16 3" fill="none" stroke="#64a0dc" strokeWidth="1.2" strokeDasharray="2 1" />
              </svg>
              <span className="text-gray-600 text-[8px]">Track history</span>
            </div>
          )}

          {/* Vector line */}
          {config.showVector && (
            <div className="flex items-center gap-1.5 mt-0.5">
              <svg width="16" height="8" viewBox="0 0 16 8" className="flex-shrink-0">
                <line x1="1" y1="7" x2="14" y2="2" stroke="#22c55e" strokeWidth="1.2" strokeDasharray="3 2" />
                <polyline points="11,1 15,2 14,6" fill="none" stroke="#22c55e" strokeWidth="1" />
              </svg>
              <span className="text-gray-600 text-[8px]">30-min vector</span>
            </div>
          )}
        </React.Fragment>
      ))}

      {/* Tool hints */}
      {(measureActive || lassoActive) && (
        <>
          {entityTypes.length > 0 && <div className="border-t border-gray-800 my-1.5" />}
          {measureActive && (
            <div className="text-yellow-500/80 text-[8px]">● Click two points to measure</div>
          )}
          {lassoActive && (
            <div className="text-cyan-500/80 text-[8px]">● Shift+drag to select area</div>
          )}
        </>
      )}
    </div>
  );
};
