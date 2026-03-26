import { describe, it, expect, beforeAll, afterAll, afterEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { setupServer } from 'msw/node';
import React from 'react';
import {
  useEntities,
  useEntity,
  useEntitySearch,
  useORPQuery,
} from '../useEntities';
import type { Entity, PaginatedResponse } from '../../types';

// ── Test Fixtures ─────────────────────────────────────────────────────────────

const makeEntity = (id: string): Entity => ({
  id,
  type: 'Ship',
  name: `Ship ${id}`,
  tags: ['cargo'],
  properties: { speed: 15, flag: 'NL' },
  geometry: { type: 'Point', coordinates: [4.27, 51.92] },
  confidence: 0.95,
  freshness: { updated_at: '2024-01-01T00:00:00Z', checked_at: '2024-01-01T00:00:00Z' },
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
  is_active: true,
});

// ── MSW Server ────────────────────────────────────────────────────────────────

const server = setupServer(
  http.get('/api/v1/entities', ({ request }) => {
    const url = new URL(request.url);
    const type = url.searchParams.get('type');
    const page = parseInt(url.searchParams.get('page') ?? '1');
    const limit = parseInt(url.searchParams.get('limit') ?? '10');

    const entities = [makeEntity('e-1'), makeEntity('e-2'), makeEntity('e-3')].filter(
      (e) => !type || e.type === type
    );

    return HttpResponse.json({
      data: entities.slice(0, limit),
      pagination: {
        page,
        limit,
        total_count: entities.length,
        total_pages: Math.ceil(entities.length / limit),
        has_next: page * limit < entities.length,
        has_prev: page > 1,
      },
    });
  }),

  http.get('/api/v1/entities/e-1', () => {
    return HttpResponse.json(makeEntity('e-1'));
  }),

  http.get('/api/v1/entities/not-found', () => {
    return HttpResponse.json({ error: { message: 'Entity not found' } }, { status: 404 });
  }),

  http.get('/api/v1/entities/search', ({ request }) => {
    const url = new URL(request.url);
    const near = url.searchParams.get('near');
    const text = url.searchParams.get('text_search');

    if (near === 'POINT(4.27,51.92)') {
      return HttpResponse.json({
        data: [makeEntity('e-near-1'), makeEntity('e-near-2')],
        count: 2,
        search_time_ms: 12,
      });
    }
    if (text) {
      return HttpResponse.json({
        data: [makeEntity('e-text-1')],
        count: 1,
        search_time_ms: 8,
      });
    }
    return HttpResponse.json({ data: [], count: 0, search_time_ms: 1 });
  }),

  http.post('/api/v1/query', async ({ request }) => {
    const body = await request.json() as { query: string };
    if (body.query === 'FAIL') {
      return HttpResponse.json(
        { error: { message: 'Query syntax error' } },
        { status: 400 }
      );
    }
    return HttpResponse.json({
      status: 'ok',
      results: [{ id: 'e-1', name: 'Ship e-1' }],
      metadata: { execution_time_ms: 5, rows_returned: 1 },
    });
  })
);

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());

// ── Test Wrapper ──────────────────────────────────────────────────────────────

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,      // disable retries globally
        retryDelay: 0,
      },
    },
  });
  return ({ children }: { children: React.ReactNode }) =>
    React.createElement(QueryClientProvider, { client: queryClient }, children);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('useEntities', () => {
  it('returns paginated entity list', async () => {
    const { result } = renderHook(() => useEntities(), { wrapper: createWrapper() });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.data).toHaveLength(3);
    expect(result.current.data?.pagination.total_count).toBe(3);
  });

  it('passes type filter to API', async () => {
    const { result } = renderHook(() => useEntities({ type: 'Ship' }), {
      wrapper: createWrapper(),
    });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.data.every((e) => e.type === 'Ship')).toBe(true);
  });

  it('respects limit parameter', async () => {
    const { result } = renderHook(() => useEntities({ limit: 1 }), {
      wrapper: createWrapper(),
    });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.data).toHaveLength(1);
  });

  it('returns data with correct entity shape', async () => {
    const { result } = renderHook(() => useEntities(), { wrapper: createWrapper() });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    const entity = result.current.data?.data[0];
    expect(entity).toHaveProperty('id');
    expect(entity).toHaveProperty('type');
    expect(entity).toHaveProperty('properties');
    expect(entity).toHaveProperty('confidence');
  });

  it('has correct pagination fields', async () => {
    const { result } = renderHook(() => useEntities({ page: 1, limit: 10 }), {
      wrapper: createWrapper(),
    });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.pagination).toHaveProperty('has_next');
    expect(result.current.data?.pagination).toHaveProperty('total_pages');
  });
});

describe('useEntity', () => {
  it('fetches a single entity by ID', async () => {
    const { result } = renderHook(() => useEntity('e-1'), { wrapper: createWrapper() });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.id).toBe('e-1');
    expect(result.current.data?.name).toBe('Ship e-1');
  });

  it('is disabled when id is null', () => {
    const { result } = renderHook(() => useEntity(null), { wrapper: createWrapper() });
    expect(result.current.fetchStatus).toBe('idle');
    expect(result.current.data).toBeUndefined();
  });

  it('returns error state on 404 (with retry disabled)', async () => {
    // Override hook retry by patching the handler to return immediately
    const wrapper = () => {
      const qc = new QueryClient({
        defaultOptions: { queries: { retry: 0, retryDelay: 0 } },
      });
      return ({ children }: { children: React.ReactNode }) =>
        React.createElement(QueryClientProvider, { client: qc }, children);
    };
    const { result } = renderHook(() => useEntity('not-found'), {
      wrapper: wrapper(),
    });
    await waitFor(() => expect(result.current.isError).toBe(true), { timeout: 5000 });
    expect(result.current.error?.message).toContain('Entity not found');
  });
});

describe('useEntitySearch', () => {
  it('returns nearby entities when near param provided', async () => {
    const { result } = renderHook(
      () => useEntitySearch({ near: 'POINT(4.27,51.92)' }),
      { wrapper: createWrapper() }
    );
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.data).toHaveLength(2);
    expect(result.current.data?.search_time_ms).toBe(12);
  });

  it('returns text search results', async () => {
    const { result } = renderHook(
      () => useEntitySearch({ text_search: 'maersk' }),
      { wrapper: createWrapper() }
    );
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.count).toBe(1);
  });

  it('is disabled when neither near nor text_search is provided', () => {
    const { result } = renderHook(() => useEntitySearch({}), {
      wrapper: createWrapper(),
    });
    expect(result.current.fetchStatus).toBe('idle');
  });

  it('includes search_time_ms in response', async () => {
    const { result } = renderHook(
      () => useEntitySearch({ near: 'POINT(4.27,51.92)' }),
      { wrapper: createWrapper() }
    );
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(typeof result.current.data?.search_time_ms).toBe('number');
  });
});

describe('useORPQuery', () => {
  it('executes a query and returns results', async () => {
    const { result } = renderHook(() => useORPQuery(), { wrapper: createWrapper() });
    result.current.mutate('MATCH (e:Ship) LIMIT 10');
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.results).toHaveLength(1);
    expect(result.current.data?.metadata.rows_returned).toBe(1);
  });

  it('returns error on bad query', async () => {
    const { result } = renderHook(() => useORPQuery(), { wrapper: createWrapper() });
    result.current.mutate('FAIL');
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.error?.message).toContain('Query syntax error');
  });

  it('handles server-level 500 error', async () => {
    server.use(
      http.post('/api/v1/query', () => {
        return HttpResponse.json({}, { status: 500 });
      })
    );
    const { result } = renderHook(() => useORPQuery(), { wrapper: createWrapper() });
    result.current.mutate('MATCH (e:Ship)');
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.error?.message).toContain('500');
  });

  it('returns metadata with execution_time_ms', async () => {
    const { result } = renderHook(() => useORPQuery(), { wrapper: createWrapper() });
    result.current.mutate('MATCH (e:Ship) LIMIT 10');
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(result.current.data?.metadata).toHaveProperty('execution_time_ms');
  });
});
