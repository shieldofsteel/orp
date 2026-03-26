import { describe, it, expect, beforeEach } from 'vitest';
import { useAppStore } from '../useAppStore';
import type { AlertEvent, Entity, QueryHistoryEntry } from '../../types';

// Helper to reset store between tests
beforeEach(() => {
  useAppStore.setState({
    selectedEntityId: null,
    selectedEntities: new Set(),
    inspectorOpen: false,
    inspectorTab: 'properties',
    sidebarOpen: true,
    entities: new Map(),
    alerts: [],
    queryHistory: [],
    queryMode: 'structured',
    lastQuery: '',
    queryResults: [],
    queryLoading: false,
    queryError: null,
    wsConnected: false,
    subscriptions: new Map(),
    relationships: new Map(),
    timeline: {
      playing: false,
      currentTime: new Date('2024-01-01T00:00:00Z'),
      speed: 1,
      minTime: new Date('2023-12-31T00:00:00Z'),
      maxTime: new Date('2024-01-01T00:00:00Z'),
    },
    sidebarSections: [
      { id: 'datasources', collapsed: false },
      { id: 'query', collapsed: false },
      { id: 'alerts', collapsed: false },
    ],
    mapCenter: [4.27, 51.92],
    mapZoom: 8,
    mapMode: '2d',
    showWeatherLayer: true,
    showShipTracksLayer: true,
    showHeatmapLayer: false,
  });
});

const makeEntity = (id: string, overrides: Partial<Entity> = {}): Entity => ({
  id,
  type: 'Ship',
  name: `Ship ${id}`,
  tags: [],
  properties: { speed: 15 },
  confidence: 0.9,
  freshness: { updated_at: '2024-01-01T00:00:00Z', checked_at: '2024-01-01T00:00:00Z' },
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
  is_active: true,
  ...overrides,
});

const makeAlert = (id: string, severity: AlertEvent['severity'] = 'info'): AlertEvent => ({
  id,
  monitor_id: `mon-${id}`,
  monitor_name: `Monitor ${id}`,
  severity,
  affected_entities: [{ entity_id: 'e-1', entity_type: 'Ship', reason: 'test' }],
  timestamp: new Date().toISOString(),
  acknowledged: false,
});

// ── selectEntity ──────────────────────────────────────────────────────────────
describe('selectEntity', () => {
  it('sets selectedEntityId and opens inspector', () => {
    useAppStore.getState().selectEntity('entity-123');
    const state = useAppStore.getState();
    expect(state.selectedEntityId).toBe('entity-123');
    expect(state.inspectorOpen).toBe(true);
  });

  it('passing null clears selection and closes inspector', () => {
    useAppStore.getState().selectEntity('entity-123');
    useAppStore.getState().selectEntity(null);
    const state = useAppStore.getState();
    expect(state.selectedEntityId).toBeNull();
    expect(state.inspectorOpen).toBe(false);
  });
});

// ── updateEntity ──────────────────────────────────────────────────────────────
describe('updateEntity', () => {
  it('merges partial changes into existing entity', () => {
    const entity = makeEntity('e-1');
    useAppStore.getState().upsertEntity(entity);
    useAppStore.getState().updateEntity('e-1', { name: 'Updated Ship' });
    const updated = useAppStore.getState().entities.get('e-1');
    expect(updated?.name).toBe('Updated Ship');
    expect(updated?.type).toBe('Ship'); // existing fields preserved
  });

  it('does nothing if entity does not exist', () => {
    // should not throw
    expect(() =>
      useAppStore.getState().updateEntity('nonexistent', { name: 'Ghost' })
    ).not.toThrow();
    expect(useAppStore.getState().entities.get('nonexistent')).toBeUndefined();
  });

  it('upsertEntity adds a new entity', () => {
    const entity = makeEntity('e-2');
    useAppStore.getState().upsertEntity(entity);
    expect(useAppStore.getState().entities.get('e-2')).toEqual(entity);
  });

  it('setEntities replaces all entities', () => {
    useAppStore.getState().upsertEntity(makeEntity('old-1'));
    useAppStore.getState().setEntities([makeEntity('new-1'), makeEntity('new-2')]);
    const entities = useAppStore.getState().entities;
    expect(entities.has('old-1')).toBe(false);
    expect(entities.has('new-1')).toBe(true);
    expect(entities.has('new-2')).toBe(true);
  });

  it('removeEntity deletes entity from map', () => {
    useAppStore.getState().upsertEntity(makeEntity('e-3'));
    useAppStore.getState().removeEntity('e-3');
    expect(useAppStore.getState().entities.has('e-3')).toBe(false);
  });
});

// ── alerts ───────────────────────────────────────────────────────────────────
describe('addAlert', () => {
  it('prepends new alert to the front', () => {
    useAppStore.getState().addAlert(makeAlert('a-1'));
    useAppStore.getState().addAlert(makeAlert('a-2'));
    const alerts = useAppStore.getState().alerts;
    expect(alerts[0].id).toBe('a-2');
    expect(alerts[1].id).toBe('a-1');
  });

  it('caps alerts at 50 entries', () => {
    for (let i = 0; i < 55; i++) {
      useAppStore.getState().addAlert(makeAlert(`a-${i}`));
    }
    expect(useAppStore.getState().alerts.length).toBe(50);
  });
});

describe('acknowledgeAlert', () => {
  it('sets acknowledged=true on the target alert only', () => {
    useAppStore.getState().addAlert(makeAlert('a-1'));
    useAppStore.getState().addAlert(makeAlert('a-2'));
    useAppStore.getState().acknowledgeAlert('a-1');
    const alerts = useAppStore.getState().alerts;
    const a1 = alerts.find((a) => a.id === 'a-1');
    const a2 = alerts.find((a) => a.id === 'a-2');
    expect(a1?.acknowledged).toBe(true);
    expect(a2?.acknowledged).toBe(false);
  });
});

describe('clearAlerts', () => {
  it('empties the alerts array', () => {
    useAppStore.getState().addAlert(makeAlert('a-1'));
    useAppStore.getState().addAlert(makeAlert('a-2'));
    useAppStore.getState().clearAlerts();
    expect(useAppStore.getState().alerts).toHaveLength(0);
  });
});

// ── timeline ──────────────────────────────────────────────────────────────────
describe('timeline', () => {
  it('setTimelinePlaying toggles playing state', () => {
    useAppStore.getState().setTimelinePlaying(true);
    expect(useAppStore.getState().timeline.playing).toBe(true);
    useAppStore.getState().setTimelinePlaying(false);
    expect(useAppStore.getState().timeline.playing).toBe(false);
  });

  it('setTimelineSpeed updates speed without affecting other fields', () => {
    useAppStore.getState().setTimelineSpeed(5);
    const tl = useAppStore.getState().timeline;
    expect(tl.speed).toBe(5);
    expect(tl.playing).toBe(false);
  });

  it('setTimelineCurrent updates currentTime', () => {
    const t = new Date('2024-06-15T12:00:00Z');
    useAppStore.getState().setTimelineCurrent(t);
    expect(useAppStore.getState().timeline.currentTime).toEqual(t);
  });

  it('setTimelineRange updates min and max', () => {
    const min = new Date('2024-01-01T00:00:00Z');
    const max = new Date('2024-12-31T23:59:59Z');
    useAppStore.getState().setTimelineRange(min, max);
    const tl = useAppStore.getState().timeline;
    expect(tl.minTime).toEqual(min);
    expect(tl.maxTime).toEqual(max);
  });
});

// ── sidebar ───────────────────────────────────────────────────────────────────
describe('sidebar', () => {
  it('setSidebarOpen sets sidebarOpen', () => {
    useAppStore.getState().setSidebarOpen(false);
    expect(useAppStore.getState().sidebarOpen).toBe(false);
  });

  it('toggleSidebarSection toggles collapsed state', () => {
    useAppStore.getState().toggleSidebarSection('query');
    expect(
      useAppStore.getState().sidebarSections.find((s) => s.id === 'query')?.collapsed
    ).toBe(true);

    useAppStore.getState().toggleSidebarSection('query');
    expect(
      useAppStore.getState().sidebarSections.find((s) => s.id === 'query')?.collapsed
    ).toBe(false);
  });

  it('toggleSidebarSection only affects target section', () => {
    useAppStore.getState().toggleSidebarSection('alerts');
    const sections = useAppStore.getState().sidebarSections;
    expect(sections.find((s) => s.id === 'datasources')?.collapsed).toBe(false);
    expect(sections.find((s) => s.id === 'query')?.collapsed).toBe(false);
    expect(sections.find((s) => s.id === 'alerts')?.collapsed).toBe(true);
  });
});

// ── query history ─────────────────────────────────────────────────────────────
describe('queryHistory', () => {
  const makeHistoryEntry = (id: string): QueryHistoryEntry => ({
    id,
    query: `MATCH (e:Ship) LIMIT 10`,
    mode: 'structured',
    timestamp: new Date(),
    resultCount: 5,
  });

  it('addQueryHistory prepends entry to front', () => {
    useAppStore.getState().addQueryHistory(makeHistoryEntry('qh-1'));
    useAppStore.getState().addQueryHistory(makeHistoryEntry('qh-2'));
    expect(useAppStore.getState().queryHistory[0].id).toBe('qh-2');
  });

  it('addQueryHistory caps at 10 entries', () => {
    for (let i = 0; i < 15; i++) {
      useAppStore.getState().addQueryHistory(makeHistoryEntry(`qh-${i}`));
    }
    expect(useAppStore.getState().queryHistory.length).toBe(10);
  });

  it('clearQueryHistory empties query history', () => {
    useAppStore.getState().addQueryHistory(makeHistoryEntry('qh-1'));
    useAppStore.getState().clearQueryHistory();
    expect(useAppStore.getState().queryHistory).toHaveLength(0);
  });
});

// ── toggleEntitySelection ─────────────────────────────────────────────────────
describe('toggleEntitySelection', () => {
  it('adds entity id to selectedEntities set', () => {
    useAppStore.getState().toggleEntitySelection('e-1');
    expect(useAppStore.getState().selectedEntities.has('e-1')).toBe(true);
  });

  it('removes entity id if already selected', () => {
    useAppStore.getState().toggleEntitySelection('e-1');
    useAppStore.getState().toggleEntitySelection('e-1');
    expect(useAppStore.getState().selectedEntities.has('e-1')).toBe(false);
  });

  it('clearSelection resets all selection state', () => {
    useAppStore.getState().selectEntity('e-1');
    useAppStore.getState().toggleEntitySelection('e-2');
    useAppStore.getState().clearSelection();
    const state = useAppStore.getState();
    expect(state.selectedEntityId).toBeNull();
    expect(state.selectedEntities.size).toBe(0);
    expect(state.inspectorOpen).toBe(false);
  });
});

// ── map layer toggles ─────────────────────────────────────────────────────────
describe('map layer toggles', () => {
  it('toggleWeatherLayer flips showWeatherLayer', () => {
    expect(useAppStore.getState().showWeatherLayer).toBe(true);
    useAppStore.getState().toggleWeatherLayer();
    expect(useAppStore.getState().showWeatherLayer).toBe(false);
    useAppStore.getState().toggleWeatherLayer();
    expect(useAppStore.getState().showWeatherLayer).toBe(true);
  });

  it('toggleHeatmapLayer flips showHeatmapLayer', () => {
    expect(useAppStore.getState().showHeatmapLayer).toBe(false);
    useAppStore.getState().toggleHeatmapLayer();
    expect(useAppStore.getState().showHeatmapLayer).toBe(true);
  });
});
