import { describe, it, expect, beforeAll, afterAll, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup, within } from '@testing-library/react';
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

// ── Helpers ───────────────────────────────────────────────────────────────────

async function openInspector(entity = fullEntity) {
  useAppStore.setState({
    inspectorOpen: true,
    selectedEntityId: entity.id,
    entities: new Map([[entity.id, entity]]),
  });
  render(<EntityInspector />);
  // Wait for the entity name to render in the header (uses store entity immediately).
  await waitFor(() =>
    expect(screen.getAllByText(entity.name ?? entity.id).length).toBeGreaterThan(0)
  );
}

const TAB_NAMES = ['OVERVIEW', 'PROPERTIES', 'RELATIONSHIPS', 'EVENTS', 'TRACK', 'INTEL'] as const;

const tabBtn = (name: (typeof TAB_NAMES)[number]) =>
  screen.getByRole('tab', { name });

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('entityInspector - visibility', () => {
  it('rendersNothingWhenInspectorClosed', () => {
    const { container } = render(<EntityInspector />);
    expect(container).toBeEmptyDOMElement();
  });

  it('rendersEntityNameInHeaderWhenOpen', async () => {
    await openInspector();
    expect(screen.getAllByText('MV Alpha').length).toBeGreaterThanOrEqual(1);
  });

  it('rendersEntityTypeBadgeInHeader', async () => {
    await openInspector();
    // Header badge uppercases the entity type ("Ship" → "SHIP").
    expect(screen.getByText('SHIP')).toBeInTheDocument();
  });

  it('closeButtonHidesInspectorAndClearsSelection', async () => {
    await openInspector();
    const closeBtn = screen.getByRole('button', { name: /close entity inspector/i });
    await user.click(closeBtn);
    expect(useAppStore.getState().inspectorOpen).toBe(false);
    expect(useAppStore.getState().selectedEntityId).toBeNull();
  });
});

describe('entityInspector - 6 tabs render', () => {
  it('allSixTabButtonsArePresent', async () => {
    await openInspector();
    for (const name of TAB_NAMES) {
      expect(tabBtn(name)).toBeInTheDocument();
    }
  });

  it('overviewTabIsSelectedByDefault', async () => {
    await openInspector();
    expect(tabBtn('OVERVIEW')).toHaveAttribute('aria-selected', 'true');
    expect(tabBtn('PROPERTIES')).toHaveAttribute('aria-selected', 'false');
  });
});

describe('entityInspector - tab content', () => {
  it('overviewTabShowsIdentifiersAndConfidence', async () => {
    await openInspector();
    // Identifiers grid renders an "ID" label and the entity id value.
    expect(screen.getByText('ID')).toBeInTheDocument();
    expect(screen.getByText('ship-alpha')).toBeInTheDocument();
    // IMO identifier is rendered alongside its value.
    expect(screen.getByText('IMO')).toBeInTheDocument();
    expect(screen.getByText('9876543')).toBeInTheDocument();
    // Confidence meter shows percentage.
    expect(screen.getByText('85%')).toBeInTheDocument();
  });

  it('propertiesTabShowsSearchInputAndPropertyKeys', async () => {
    await openInspector();
    await user.click(tabBtn('PROPERTIES'));
    expect(
      screen.getByPlaceholderText(/search properties/i)
    ).toBeInTheDocument();
    // Core + dynamic keys are listed as text.
    expect(screen.getByText('id')).toBeInTheDocument();
    expect(screen.getByText('imo')).toBeInTheDocument();
    expect(screen.getByText('flag')).toBeInTheDocument();
  });

  it('relationshipsTabShowsHopAndViewControls', async () => {
    await openInspector();
    await user.click(tabBtn('RELATIONSHIPS'));
    // Hop selector and view toggle are present.
    expect(screen.getByRole('button', { name: '1-HOP' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'GRAPH' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'LIST' })).toBeInTheDocument();
  });

  it('relationshipsListViewShowsRelatedEntities', async () => {
    await openInspector();
    await user.click(tabBtn('RELATIONSHIPS'));
    await user.click(screen.getByRole('button', { name: 'LIST' }));
    await waitFor(() => {
      expect(screen.getByText('Port of Rotterdam')).toBeInTheDocument();
      expect(screen.getByText('Maersk Line')).toBeInTheDocument();
    });
  });

  it('eventsTabShowsHistoryEntries', async () => {
    await openInspector();
    await user.click(tabBtn('EVENTS'));
    // The single history entry exposes its source label.
    expect(screen.getByText('ais-connector')).toBeInTheDocument();
    // The "all" filter pill is rendered.
    expect(screen.getByRole('button', { name: 'all' })).toBeInTheDocument();
  });

  it('trackTabRendersPlaybackOrEmptyState', async () => {
    await openInspector();
    await user.click(tabBtn('TRACK'));
    // With a current geometry the panel mounts the mini-map (canvas) — we just
    // assert the tab panel is now wired to the TRACK tab.
    const panel = await screen.findByRole('tabpanel');
    expect(panel).toHaveAttribute('aria-labelledby', 'inspector-tab-track');
  });

  it('intelTabShowsClassificationAndRiskScore', async () => {
    await openInspector();
    await user.click(tabBtn('INTEL'));
    expect(screen.getByText(/UNCLASSIFIED/)).toBeInTheDocument();
    expect(screen.getByText(/Risk Score/i)).toBeInTheDocument();
  });
});

describe('entityInspector - tab switching', () => {
  it('switchingFromOverviewToPropertiesSwapsPanelContent', async () => {
    await openInspector();
    // OVERVIEW shows the identifier grid (label "ID").
    expect(screen.getByText('ID')).toBeInTheDocument();
    // No search input on OVERVIEW.
    expect(screen.queryByPlaceholderText(/search properties/i)).toBeNull();

    await user.click(tabBtn('PROPERTIES'));

    // PROPERTIES shows its search input.
    expect(
      screen.getByPlaceholderText(/search properties/i)
    ).toBeInTheDocument();
    // aria-selected updates on the tab buttons.
    expect(tabBtn('PROPERTIES')).toHaveAttribute('aria-selected', 'true');
    expect(tabBtn('OVERVIEW')).toHaveAttribute('aria-selected', 'false');
  });

  it('switchingFromPropertiesToIntelMountsIntelContent', async () => {
    await openInspector();
    await user.click(tabBtn('PROPERTIES'));
    expect(
      screen.getByPlaceholderText(/search properties/i)
    ).toBeInTheDocument();

    await user.click(tabBtn('INTEL'));

    // INTEL panel shows risk score + sanctions status, neither present on PROPERTIES.
    const panel = screen.getByRole('tabpanel');
    expect(within(panel).getByText(/Risk Score/i)).toBeInTheDocument();
    expect(within(panel).getByText(/Sanctions Status/i)).toBeInTheDocument();
    expect(screen.queryByPlaceholderText(/search properties/i)).toBeNull();
  });
});
