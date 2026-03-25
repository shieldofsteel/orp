import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import type { Entity, EntityFilters, PaginatedResponse, RelationshipsResponse } from '../types';

const API_BASE = '/api/v1';

async function apiFetch<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    headers: { 'Content-Type': 'application/json', ...init?.headers },
    ...init,
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body?.error?.message ?? `API error: ${res.status}`);
  }
  return res.json();
}

/** Fetch paginated entity list */
export function useEntities(filters?: EntityFilters) {
  return useQuery({
    queryKey: ['entities', filters],
    queryFn: () => {
      const params = new URLSearchParams();
      if (filters?.page) params.set('page', String(filters.page));
      if (filters?.limit) params.set('limit', String(filters.limit));
      if (filters?.type) params.set('type', filters.type);
      if (filters?.sort_by) params.set('sort_by', filters.sort_by);
      if (filters?.sort_order) params.set('sort_order', filters.sort_order);
      const qs = params.toString();
      return apiFetch<PaginatedResponse<Entity>>(
        `${API_BASE}/entities${qs ? `?${qs}` : ''}`
      );
    },
    staleTime: 5000,
    gcTime: 1000 * 60 * 5,
    refetchOnWindowFocus: false,
    retry: 2,
  });
}

/** Fetch a single entity by ID */
export function useEntity(id: string | null) {
  return useQuery({
    queryKey: ['entity', id],
    queryFn: () => apiFetch<Entity>(`${API_BASE}/entities/${id}`),
    enabled: !!id,
    staleTime: 5000,
    retry: 1,
  });
}

/** Fetch relationships for an entity */
export function useEntityRelationships(id: string | null) {
  return useQuery({
    queryKey: ['entity-relationships', id],
    queryFn: () =>
      apiFetch<RelationshipsResponse>(`${API_BASE}/entities/${id}/relationships`),
    enabled: !!id,
    staleTime: 10000,
    retry: 1,
  });
}

/** Fetch events for an entity */
export function useEntityEvents(id: string | null, limit = 50) {
  return useQuery({
    queryKey: ['entity-events', id, limit],
    queryFn: () =>
      apiFetch<{ data: Array<Record<string, unknown>>; count: number }>(
        `${API_BASE}/entities/${id}/events?limit=${limit}`
      ),
    enabled: !!id,
    staleTime: 10000,
  });
}

/** Search entities */
export function useEntitySearch(params: {
  type?: string;
  near?: string;
  text_search?: string;
  limit?: number;
}) {
  return useQuery({
    queryKey: ['entity-search', params],
    queryFn: () => {
      const qs = new URLSearchParams();
      if (params.type) qs.set('type', params.type);
      if (params.near) qs.set('near', params.near);
      if (params.text_search) qs.set('text_search', params.text_search);
      if (params.limit) qs.set('limit', String(params.limit));
      return apiFetch<{ data: Entity[]; count: number; search_time_ms: number }>(
        `${API_BASE}/entities/search?${qs.toString()}`
      );
    },
    enabled: !!(params.near || params.text_search),
    staleTime: 10000,
  });
}

/** Execute ORP-QL query */
export function useORPQuery() {
  return useMutation({
    mutationFn: (query: string) =>
      apiFetch<{
        status: string;
        results: Array<Record<string, unknown>>;
        metadata: { execution_time_ms: number; rows_returned: number };
      }>(`${API_BASE}/query`, {
        method: 'POST',
        body: JSON.stringify({ query }),
      }),
  });
}

/** Fetch health status */
export function useHealth() {
  return useQuery({
    queryKey: ['health'],
    queryFn: () => apiFetch<Record<string, unknown>>(`${API_BASE}/health`),
    staleTime: 15000,
    refetchInterval: 30000,
  });
}

/** Fetch connectors */
export function useConnectors() {
  return useQuery({
    queryKey: ['connectors'],
    queryFn: () =>
      apiFetch<{ data: Array<Record<string, unknown>>; count: number }>(
        `${API_BASE}/connectors`
      ),
    staleTime: 15000,
  });
}

/** Fetch alerts */
export function useAlerts(limit = 100) {
  return useQuery({
    queryKey: ['alerts', limit],
    queryFn: () =>
      apiFetch<{ data: Array<Record<string, unknown>>; count: number }>(
        `${API_BASE}/alerts?limit=${limit}`
      ),
    staleTime: 5000,
    refetchInterval: 10000,
  });
}

/** Create entity mutation */
export function useCreateEntity() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (entity: Partial<Entity>) =>
      apiFetch<Entity>(`${API_BASE}/entities`, {
        method: 'POST',
        body: JSON.stringify(entity),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['entities'] });
    },
  });
}
