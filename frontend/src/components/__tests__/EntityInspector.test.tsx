import { describe, it, expect, beforeAll, afterAll, afterEach, beforeEach } from 'vitest';
import { render, screen, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { http, HttpResponse } from 'msw';
import { setupServer } from 'msw/node';
import React from 'react';
import { EntityInspector } from '../EntityInspector';
import { useAppStore } from '../../store/useAppStore';
import type { Entity } from '../../types';

const user = userEvent.setup({ delay: null });

// ── Fixtures ──────────────────────────────────────────────────────────────────

const fullEntity: Entity = {
  id: 'ship-alpha',
  type: 'Ship',
  name: 'MV Alpha',
  tags: ['cargo', 'active'],
  properties: { speed: 15, flag: 'NL', imo: '9876543' },
  geometry: { type: 'Point', coordinates: [4.27, 51.92] },
  confidence: 0.85,
  freshness: {
    updated_at: new Date(Date.now() - 60_000).toISOString(),
    checked_at: new Date(Date.now() - 30_000).toISOString(),
  },
  created_at: new Date(Date.now() - 86400_000).toISOString(),
  updated_at: new Date(Date.now() - 60_000).toISOString(),
  is_active: true,
  history: [
    {
      timestamp: new Date(Date.now() - 120_000).toISOString(),
      changed_properties: { speed: 12 },
      source: 'ais-connector',
    },
  ],
};

const relationships = {
  entity_id: 'ship-alpha',
  outgoing: [
    {
      id: 'rel-1',
      type: 'HEADING_TO',
      target_id: 'port-rotterdam',
      target_type: 'Port',
      target_name: 'Port of Rotterdam',
      confidence: 0.9,
    },
  ],
  incoming: [
    {
      id: 'rel-2',
      type: 'OPERATED_BY',
      source_id: 'company-maersk',
      source_type: 'Company',
      source_name: 'Maersk Line',
      confidence: 0.95,
    },
  ],
  total: 2,
};

// ── MSW Server ────────────────────────────────────────────────────────────────

const server = setupServer(
  http.get('/api/v1/entities/ship-alpha', () =>
    HttpResponse.json(fullEntity)
  ),
  http.get('/api/v1/entities/ship-alpha/relationships', () =>
    HttpResponse.json(relationships)
  ),
  http.get('/api/v1/entities/ship-broken', () =>
    HttpResponse.json({}, { status: 503 })
  ),
  http.get('/api/v1/entities/ship-broken/relationships', () =>
    HttpResponse.json({}, { status: 503 })
  )
);

beforeAll(() => server.listen({ onUnhandledRequest: 'warn' }));
afterEach(() => {
  server.resetHandlers();
  cleanup();
  useAppStore.setState({
    selectedEntityId: null,
    inspectorOpen: false,
    inspectorTab: 'properties',
    entities: new Map(),
  });
});
afterAll(() => server.close());

// ── Helper ────────────────────────────────────────────────────────────────────

async function openInspector(entity = fullEntity) {
  useAppStore.setState({
    inspectorOpen: true,
    selectedEntityId: entity.id,
    entities: new Map([[entity.id, entity]]),
    inspectorTab: 'properties',
  });
  render(<EntityInspector />);
  // Wait for entity name to appear (uses store entity immediately)
  await waitFor(() => expect(screen.getAllByText('MV Alpha').length).toBeGreaterThan(0));
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('EntityInspector - visibility', () => {
  it('renders nothing when inspector is closed', () => {
    const { container } = render(<EntityInspector />);
    expect(container).toBeEmptyDOMElement();
  });

  it('renders entity name when open', async () => {
    await openInspector();
    expect(screen.getAllByText('MV Alpha').length).toBeGreaterThanOrEqual(1);
  });

  it('close button hides inspector and clears selection', async () => {
    await openInspector();
    const closeBtn = screen.getByRole('button', { name: '✕' });
    await user.click(closeBtn);
    expect(useAppStore.getState().inspectorOpen).toBe(false);
    expect(useAppStore.getState().selectedEntityId).toBeNull();
  });

  it('renders entity type badge in header', async () => {
    await openInspector();
    // "Ship" appears in both the header badge and properties tab — use getAllByText
    expect(screen.getAllByText('Ship').length).toBeGreaterThanOrEqual(1);
  });
});

describe('EntityInspector - 4 tabs render', () => {
  it('all 4 tab buttons are present', async () => {
    await openInspector();
    expect(screen.getByRole('button', { name: 'Properties' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Relations' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'History' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Quality' })).toBeInTheDocument();
  });
});

describe('EntityInspector - Properties tab', () => {
  it('shows dynamic properties (speed, flag, imo)', async () => {
    await openInspector();
    await user.click(screen.getByRole('button', { name: 'Properties' }));
    expect(screen.getByText('15')).toBeInTheDocument();   // speed
    expect(screen.getByText('NL')).toBeInTheDocument();   // flag
    expect(screen.getByText('9876543')).toBeInTheDocument(); // imo
  });

  it('shows active status in properties view', async () => {
    await openInspector();
    // "Active: Yes" is shown in the core fields list
    expect(screen.getByText('Yes')).toBeInTheDocument();
    // ID is shown
    expect(screen.getByText('ship-alpha')).toBeInTheDocument();
  });

  it('shows tags when present', async () => {
    await openInspector();
    expect(screen.getByText('cargo')).toBeInTheDocument();
    expect(screen.getByText('active')).toBeInTheDocument();
  });
});

describe('EntityInspector - Relationships tab', () => {
  it('shows outgoing and incoming relationship labels', async () => {
    await openInspector();
    await user.click(screen.getByRole('button', { name: 'Relations' }));
    await waitFor(() => {
      expect(screen.getByText('Port of Rotterdam')).toBeInTheDocument();
      expect(screen.getByText('Maersk Line')).toBeInTheDocument();
    });
  });

  it('clicking a relationship name navigates to that entity', async () => {
    await openInspector();
    await user.click(screen.getByRole('button', { name: 'Relations' }));
    await waitFor(() => expect(screen.getByText('Port of Rotterdam')).toBeInTheDocument());
    await user.click(screen.getByText('Port of Rotterdam'));
    expect(useAppStore.getState().selectedEntityId).toBe('port-rotterdam');
  });

  it('shows relationship type badges', async () => {
    await openInspector();
    await user.click(screen.getByRole('button', { name: 'Relations' }));
    await waitFor(() => {
      expect(screen.getByText('HEADING_TO')).toBeInTheDocument();
      expect(screen.getByText('OPERATED_BY')).toBeInTheDocument();
    });
  });
});

describe('EntityInspector - Quality tab', () => {
  it('shows confidence bar with correct percentage', async () => {
    await openInspector();
    await user.click(screen.getByRole('button', { name: 'Quality' }));
    expect(screen.getByText('Confidence')).toBeInTheDocument();
    expect(screen.getByText('85%')).toBeInTheDocument();
  });

  it('confidence bar div has width proportional to confidence', async () => {
    await openInspector();
    await user.click(screen.getByRole('button', { name: 'Quality' }));
    // Find the inner bar by looking for style with width: 85%
    const bars = document.querySelectorAll('[style*="width: 85%"]');
    expect(bars.length).toBeGreaterThanOrEqual(1);
  });
});

describe('EntityInspector - History tab', () => {
  it('shows history entries from entity history', async () => {
    await openInspector();
    await user.click(screen.getByRole('button', { name: 'History' }));
    expect(screen.getByText('ais-connector')).toBeInTheDocument();
  });

  it('shows "No history" when entity has no history', async () => {
    const entityNoHistory = { ...fullEntity, id: 'ship-beta', history: [] };
    useAppStore.setState({
      inspectorOpen: true,
      selectedEntityId: 'ship-beta',
      entities: new Map([['ship-beta', entityNoHistory]]),
      inspectorTab: 'history',
    });
    render(<EntityInspector />);
    await waitFor(() => expect(screen.getByText(/no history/i)).toBeInTheDocument());
  });
});
