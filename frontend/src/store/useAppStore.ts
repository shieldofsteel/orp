import { create } from 'zustand';
import type { Entity, Relationship, Subscription, AlertEvent } from '../types';

interface AppState {
  // UI State
  selectedEntityId: string | null;
  selectedEntities: Set<string>;
  sidebarOpen: boolean;
  inspectorOpen: boolean;

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

  // WebSocket State
  wsConnected: boolean;
  subscriptions: Map<string, Subscription>;

  // Data
  entities: Map<string, Entity>;
  relationships: Map<string, Relationship>;
  alerts: AlertEvent[];

  // Timeline
  timelineMin: Date;
  timelineMax: Date;
  timelineCurrent: Date;

  // Actions
  selectEntity: (id: string | null) => void;
  toggleEntitySelection: (id: string) => void;
  clearSelection: () => void;
  setSidebarOpen: (open: boolean) => void;
  setInspectorOpen: (open: boolean) => void;

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

  setWsConnected: (connected: boolean) => void;
  addSubscription: (sub: Subscription) => void;
  removeSubscription: (id: string) => void;

  setEntities: (entities: Entity[]) => void;
  updateEntity: (id: string, changes: Partial<Entity>) => void;
  removeEntity: (id: string) => void;
  addAlert: (alert: AlertEvent) => void;
  clearAlerts: () => void;

  setTimelineCurrent: (time: Date) => void;
}

export const useAppStore = create<AppState>((set) => ({
  // Initial UI State
  selectedEntityId: null,
  selectedEntities: new Set(),
  sidebarOpen: true,
  inspectorOpen: false,

  // Initial Map State — centered on Rotterdam
  mapCenter: [4.27, 51.92],
  mapZoom: 8,
  mapMode: '2d',
  showWeatherLayer: true,
  showShipTracksLayer: true,
  showHeatmapLayer: false,

  // Initial Query State
  lastQuery: '',
  queryResults: [],
  queryLoading: false,
  queryError: null,

  // Initial WebSocket State
  wsConnected: false,
  subscriptions: new Map(),

  // Initial Data
  entities: new Map(),
  relationships: new Map(),
  alerts: [],

  // Initial Timeline
  timelineMin: new Date(Date.now() - 24 * 60 * 60 * 1000),
  timelineMax: new Date(),
  timelineCurrent: new Date(),

  // Actions
  selectEntity: (id) =>
    set({ selectedEntityId: id, inspectorOpen: id !== null }),

  toggleEntitySelection: (id) =>
    set((state) => {
      const next = new Set(state.selectedEntities);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return { selectedEntities: next };
    }),

  clearSelection: () =>
    set({ selectedEntityId: null, selectedEntities: new Set(), inspectorOpen: false }),

  setSidebarOpen: (open) => set({ sidebarOpen: open }),
  setInspectorOpen: (open) => set({ inspectorOpen: open }),

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
      for (const e of entities) {
        map.set(e.id, e);
      }
      return { entities: map };
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
    set((state) => ({
      alerts: [alert, ...state.alerts].slice(0, 200),
    })),

  clearAlerts: () => set({ alerts: [] }),

  setTimelineCurrent: (time) => set({ timelineCurrent: time }),
}));
