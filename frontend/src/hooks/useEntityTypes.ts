/**
 * useEntityTypes — Auto-discovers entity types from live data and builds
 * an EntityTypeRegistry. Zero hardcoding: add a new data source with
 * aircraft / IoT sensors / cyber threats — the registry auto-expands.
 */
import { useMemo } from 'react';
import type { Entity, EntityTypeConfig, EntityTypeRegistry } from '../types';

// ── Palette for auto-assigned unknown types ───────────────────────────────────

const AUTO_PALETTE: Array<[number, number, number]> = [
  [139, 92, 246],  // violet
  [236, 72, 153],  // pink
  [20, 184, 166],  // teal
  [245, 158, 11],  // amber
  [99, 102, 241],  // indigo
  [16, 185, 129],  // emerald
  [239, 68, 68],   // red
  [59, 130, 246],  // blue
  [234, 179, 8],   // yellow
  [168, 85, 247],  // purple
];

function rgb(r: number, g: number, b: number): [number, number, number] {
  return [r, g, b];
}

function toHex([r, g, b]: [number, number, number]): string {
  return `#${r.toString(16).padStart(2, '0')}${g.toString(16).padStart(2, '0')}${b.toString(16).padStart(2, '0')}`;
}

// ── Known-type default configs ────────────────────────────────────────────────

/**
 * Well-known type matchers. Each entry has:
 * - match: array of substrings to check against entity.type.toLowerCase()
 * - config: partial EntityTypeConfig (type/colorHex computed automatically)
 */
const KNOWN_TYPES: Array<{
  match: string[];
  config: Omit<EntityTypeConfig, 'type' | 'colorHex'>;
}> = [
  {
    match: ['ship', 'vessel', 'ais', 'boat', 'tanker', 'cargo', 'ferry'],
    config: {
      label: 'Ship',
      icon: 'M10,1 L18,23 L10,17 L2,23 Z',
      iconIsEmoji: false,
      color: rgb(59, 130, 246),
      markerStyle: 'arrow',
      speedField: 'speed',
      headingField: 'course',
      showVector: true,
      showTrack: true,
      isArea: false,
    },
  },
  {
    match: ['aircraft', 'flight', 'adsb', 'plane', 'uav', 'drone', 'helicopter'],
    config: {
      label: 'Aircraft',
      icon: 'M12,2 L8,10 L2,10 L5,13 L3,20 L12,15 L21,20 L19,13 L22,10 L16,10 Z',
      iconIsEmoji: false,
      color: rgb(52, 211, 153),
      markerStyle: 'plane',
      speedField: 'speed',
      headingField: 'heading',
      altitudeField: 'altitude',
      showVector: true,
      showTrack: true,
      isArea: false,
    },
  },
  {
    match: ['port', 'harbor', 'harbour', 'terminal', 'anchorage'],
    config: {
      label: 'Port',
      icon: '',
      iconIsEmoji: false,
      color: rgb(249, 115, 22),
      markerStyle: 'circle',
      showVector: false,
      showTrack: false,
      isArea: false,
    },
  },
  {
    match: ['weather', 'storm', 'cyclone', 'hurricane', 'typhoon', 'metoc'],
    config: {
      label: 'Weather',
      icon: '',
      iconIsEmoji: false,
      color: rgb(99, 102, 241),
      markerStyle: 'circle',
      showVector: false,
      showTrack: false,
      isArea: true,
    },
  },
  {
    match: ['sensor', 'radar', 'camera', 'detector', 'sonar', 'lidar'],
    config: {
      label: 'Sensor',
      icon: 'M12,9 A3,3 0 1,0 12,15 A3,3 0 1,0 12,9 Z M7,7 A7,7 0 0,0 7,17 M17,7 A7,7 0 0,1 17,17',
      iconIsEmoji: false,
      color: rgb(167, 139, 250),
      markerStyle: 'dot',
      speedField: undefined,
      headingField: undefined,
      showVector: false,
      showTrack: false,
      isArea: false,
    },
  },
  {
    match: ['host', 'server', 'device', 'endpoint', 'node', 'computer', 'workstation'],
    config: {
      label: 'Host',
      icon: '💻',
      iconIsEmoji: true,
      color: rgb(34, 197, 94),
      markerStyle: 'square',
      showVector: false,
      showTrack: false,
      isArea: false,
    },
  },
  {
    match: ['threat', 'malware', 'attack', 'intrusion', 'exploit', 'vulnerability'],
    config: {
      label: 'Threat',
      icon: '⚠',
      iconIsEmoji: true,
      color: rgb(239, 68, 68),
      markerStyle: 'diamond',
      showVector: false,
      showTrack: false,
      isArea: false,
    },
  },
  {
    match: ['vehicle', 'truck', 'car', 'convoy', 'asset'],
    config: {
      label: 'Vehicle',
      icon: '🚛',
      iconIsEmoji: true,
      color: rgb(251, 191, 36),
      markerStyle: 'arrow',
      speedField: 'speed',
      headingField: 'heading',
      showVector: true,
      showTrack: true,
      isArea: false,
    },
  },
  {
    match: ['person', 'personnel', 'guard', 'agent', 'operative'],
    config: {
      label: 'Personnel',
      icon: '👤',
      iconIsEmoji: true,
      color: rgb(251, 146, 60),
      markerStyle: 'dot',
      speedField: 'speed',
      headingField: 'heading',
      showVector: false,
      showTrack: true,
      isArea: false,
    },
  },
];

// ── Type resolution ───────────────────────────────────────────────────────────

function resolveKnownConfig(
  rawType: string,
): Omit<EntityTypeConfig, 'type' | 'colorHex'> | null {
  const lower = rawType.toLowerCase();
  for (const kt of KNOWN_TYPES) {
    if (kt.match.some((m) => lower.includes(m))) {
      return kt.config;
    }
  }
  return null;
}

let _paletteIdx = 0;
const _typeColorCache = new Map<string, [number, number, number]>();

function autoColor(type: string): [number, number, number] {
  if (_typeColorCache.has(type)) return _typeColorCache.get(type)!;
  const color = AUTO_PALETTE[_paletteIdx % AUTO_PALETTE.length];
  _paletteIdx++;
  _typeColorCache.set(type, color);
  return color;
}

function buildConfig(rawType: string): EntityTypeConfig {
  const known = resolveKnownConfig(rawType);
  if (known) {
    return {
      ...known,
      type: rawType.toLowerCase(),
      colorHex: toHex(known.color),
    };
  }
  // Unknown type: auto-assign color and generic icon
  const color = autoColor(rawType.toLowerCase());
  return {
    type: rawType.toLowerCase(),
    label: rawType.charAt(0).toUpperCase() + rawType.slice(1),
    icon: '',
    iconIsEmoji: false,
    color,
    colorHex: toHex(color),
    markerStyle: 'dot',
    showVector: false,
    showTrack: false,
    isArea: false,
  };
}

// ── Hook ──────────────────────────────────────────────────────────────────────

/**
 * Builds an EntityTypeRegistry from the current entity store values.
 * Call with the live entity map (or array) — updates whenever entities change.
 */
export function useEntityTypes(entities: Map<string, Entity>): EntityTypeRegistry {
  return useMemo(() => {
    const registry: EntityTypeRegistry = new Map();
    for (const entity of entities.values()) {
      if (!entity.type) continue;
      const key = entity.type.toLowerCase();
      if (!registry.has(key)) {
        registry.set(key, buildConfig(entity.type));
      }
    }
    return registry;
  }, [entities]);
}

/**
 * Returns the EntityTypeConfig for a single entity,
 * falling back to a generic unknown config.
 */
export function getEntityTypeConfig(
  registry: EntityTypeRegistry,
  entity: Entity,
): EntityTypeConfig {
  return (
    registry.get(entity.type?.toLowerCase() ?? '') ??
    buildConfig(entity.type ?? 'unknown')
  );
}

/**
 * Group entities by type.
 */
export function groupEntitiesByType(
  entities: Map<string, Entity>,
  registry: EntityTypeRegistry,
): Map<string, Entity[]> {
  const groups = new Map<string, Entity[]>();
  for (const [key] of registry) {
    groups.set(key, []);
  }
  for (const entity of entities.values()) {
    const key = entity.type?.toLowerCase() ?? 'unknown';
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(entity);
  }
  return groups;
}
