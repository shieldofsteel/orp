import { create } from 'zustand';
import type {
  Entity,
  Relationship,
  Subscription,
  AlertEvent,
  QueryMode,
  QueryHistoryEntry,
  SidebarSection,
  TimelineState,
} from '../types';

interface AppState {
  // UI State
  selectedEntityId: string | null;
  selectedEntities: Set<string>;
  sidebarOpen: boolean;
  inspectorOpen: boolean;
  inspectorTab: 'properties' | 'relationships' | 'history' | 'quality';

  // Map/View State
  mapCenter: [number, number];
  mapZoom: number;
  mapMode: '2d' | '3d';
  showWeatherLayer: boolean;
  showShipTracksLayer: boolean;
  showHeatmapLayer: boolean;

  // Query State
  lastQuery: string;
  queryResults: Array<Record<string, unknown>>;
  queryLoading: boolean;
  queryError: string | null;
  queryMode: QueryMode;
  queryHistory: QueryHistoryEntry[];

  // WebSocket State
  wsConnected: boolean;
  subscriptions: Map<string, Subscription>;

  // Data
  entities: Map<string, Entity>;
  relationships: Map<string, Relationship>;
  alerts: AlertEvent[];

  // Timeline
  timeline: TimelineState;

  // Sidebar sections collapsed state
  sidebarSections: SidebarSection[];

  // ── Actions ──

  selectEntity: (id: string | null) => void;
  toggleEntitySelection: (id: string) => void;
  clearSelection: () => void;
  setSidebarOpen: (open: boolean) => void;
  setInspectorOpen: (open: boolean) => void;
  setInspectorTab: (tab: AppState['inspectorTab']) => void;

  setMapCenter: (center: [number, number]) => void;
  setMapZoom: (zoom: number) => void;
  setMapMode: (mode: '2d' | '3d') => void;
  toggleWeatherLayer: () => void;
  toggleShipTracksLayer: () => void;
  toggleHeatmapLayer: () => void;

  setLastQuery: (query: string) => void;
  setQueryResults: (results: Array<Record<string, unknown>>) => void;
  setQueryLoading: (loading: boolean) => void;
  setQueryError: (error: string | null) => void;
  setQueryMode: (mode: QueryMode) => void;
  addQueryHistory: (entry: QueryHistoryEntry) => void;
  clearQueryHistory: () => void;

  setWsConnected: (connected: boolean) => void;
  addSubscription: (sub: Subscription) => void;
  removeSubscription: (id: string) => void;

  setEntities: (entities: Entity[]) => void;
  upsertEntity: (entity: Entity) => void;
  updateEntity: (id: string, changes: Partial<Entity>) => void;
  removeEntity: (id: string) => void;

  addAlert: (alert: AlertEvent) => void;
  acknowledgeAlert: (id: string) => void;
  clearAlerts: () => void;

  setTimelineCurrent: (time: Date) => void;
  setTimelinePlaying: (playing: boolean) => void;
  setTimelineSpeed: (speed: number) => void;
  setTimelineRange: (min: Date, max: Date) => void;

  toggleSidebarSection: (id: SidebarSection['id']) => void;
}

const DEFAULT_SIDEBAR_SECTIONS: SidebarSection[] = [
  { id: 'datasources', collapsed: false },
  { id: 'query', collapsed: false },
  { id: 'alerts', collapsed: false },
];

export const useAppStore = create<AppState>((set) => ({
  // ── Initial State ──
  selectedEntityId: null,
  selectedEntities: new Set(),
  sidebarOpen: true,
  inspectorOpen: false,
  inspectorTab: 'properties',

  mapCenter: [4.27, 51.92],
  mapZoom: 10,
  mapMode: '2d',
  showWeatherLayer: true,
  showShipTracksLayer: true,
  showHeatmapLayer: false,

  lastQuery: '',
  queryResults: [],
  queryLoading: false,
  queryError: null,
  queryMode: 'structured',
  queryHistory: [],

  wsConnected: false,
  subscriptions: new Map(),

  entities: new Map(),
  relationships: new Map(),
  alerts: [],

  timeline: {
    playing: false,
    currentTime: new Date(),
    speed: 1,
    minTime: new Date(Date.now() - 24 * 60 * 60 * 1000),
    maxTime: new Date(),
  },

  sidebarSections: DEFAULT_SIDEBAR_SECTIONS,

  // ── Actions ──
  selectEntity: (id) =>
    set({ selectedEntityId: id, inspectorOpen: id !== null }),

  toggleEntitySelection: (id) =>
    set((state) => {
      const next = new Set(state.selectedEntities);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return { selectedEntities: next };
    }),

  clearSelection: () =>
    set({ selectedEntityId: null, selectedEntities: new Set(), inspectorOpen: false }),

  setSidebarOpen: (open) => set({ sidebarOpen: open }),
  setInspectorOpen: (open) => set({ inspectorOpen: open }),
  setInspectorTab: (tab) => set({ inspectorTab: tab }),

  setMapCenter: (center) => set({ mapCenter: center }),
  setMapZoom: (zoom) => set({ mapZoom: zoom }),
  setMapMode: (mode) => set({ mapMode: mode }),
  toggleWeatherLayer: () => set((s) => ({ showWeatherLayer: !s.showWeatherLayer })),
  toggleShipTracksLayer: () => set((s) => ({ showShipTracksLayer: !s.showShipTracksLayer })),
  toggleHeatmapLayer: () => set((s) => ({ showHeatmapLayer: !s.showHeatmapLayer })),

  setLastQuery: (query) => set({ lastQuery: query }),
  setQueryResults: (results) => set({ queryResults: results, queryLoading: false }),
  setQueryLoading: (loading) => set({ queryLoading: loading }),
  setQueryError: (error) => set({ queryError: error, queryLoading: false }),
  setQueryMode: (mode) => set({ queryMode: mode }),
  addQueryHistory: (entry) =>
    set((s) => ({ queryHistory: [entry, ...s.queryHistory].slice(0, 10) })),
  clearQueryHistory: () => set({ queryHistory: [] }),

  setWsConnected: (connected) => set({ wsConnected: connected }),
  addSubscription: (sub) =>
    set((state) => {
      const next = new Map(state.subscriptions);
      next.set(sub.id, sub);
      return { subscriptions: next };
    }),
  removeSubscription: (id) =>
    set((state) => {
      const next = new Map(state.subscriptions);
      next.delete(id);
      return { subscriptions: next };
    }),

  setEntities: (entities) =>
    set(() => {
      const map = new Map<string, Entity>();
      for (const e of entities) map.set(e.id, e);
      return { entities: map };
    }),

  upsertEntity: (entity) =>
    set((state) => {
      const next = new Map(state.entities);
      next.set(entity.id, entity);
      return { entities: next };
    }),

  updateEntity: (id, changes) =>
    set((state) => {
      const entity = state.entities.get(id);
      if (!entity) return state;
      const next = new Map(state.entities);
      next.set(id, { ...entity, ...changes });
      return { entities: next };
    }),

  removeEntity: (id) =>
    set((state) => {
      const next = new Map(state.entities);
      next.delete(id);
      return { entities: next };
    }),

  addAlert: (alert) =>
    set((state) => ({ alerts: [alert, ...state.alerts].slice(0, 50) })),

  acknowledgeAlert: (id) =>
    set((state) => ({
      alerts: state.alerts.map((a) => (a.id === id ? { ...a, acknowledged: true } : a)),
    })),

  clearAlerts: () => set({ alerts: [] }),

  setTimelineCurrent: (time) =>
    set((s) => ({ timeline: { ...s.timeline, currentTime: time } })),

  setTimelinePlaying: (playing) =>
    set((s) => ({ timeline: { ...s.timeline, playing } })),

  setTimelineSpeed: (speed) =>
    set((s) => ({ timeline: { ...s.timeline, speed } })),

  setTimelineRange: (min, max) =>
    set((s) => ({ timeline: { ...s.timeline, minTime: min, maxTime: max } })),

  toggleSidebarSection: (id) =>
    set((state) => ({
      sidebarSections: state.sidebarSections.map((s) =>
        s.id === id ? { ...s, collapsed: !s.collapsed } : s
      ),
    })),
}));
