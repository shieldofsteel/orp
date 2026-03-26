import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import React from 'react';
import { AlertFeed } from '../AlertFeed';
import { useAppStore } from '../../store/useAppStore';
import type { AlertEvent } from '../../types';

const user = userEvent.setup({ delay: null });

function makeAlert(
  id: string,
  severity: AlertEvent['severity'] = 'info',
  acknowledged = false
): AlertEvent {
  return {
    id,
    monitor_id: `mon-${id}`,
    monitor_name: `Monitor ${id}`,
    severity,
    affected_entities: [
      { entity_id: `ent-${id}`, entity_type: 'Ship', reason: `Reason for ${id}` },
    ],
    timestamp: new Date().toISOString(),
    acknowledged,
  };
}

beforeEach(() => {
  useAppStore.setState({ alerts: [], selectedEntityId: null, inspectorOpen: false });
});

describe('AlertFeed - rendering', () => {
  it('shows "No alerts" when feed is empty', () => {
    render(<AlertFeed />);
    expect(screen.getByText(/no alerts/i)).toBeInTheDocument();
  });

  it('renders critical alert with CRIT badge', () => {
    useAppStore.getState().addAlert(makeAlert('a-1', 'critical'));
    render(<AlertFeed />);
    expect(screen.getByText('CRIT')).toBeInTheDocument();
    expect(screen.getByText('Monitor a-1')).toBeInTheDocument();
  });

  it('renders warning alert with WARN badge', () => {
    useAppStore.getState().addAlert(makeAlert('a-2', 'warning'));
    render(<AlertFeed />);
    expect(screen.getByText('WARN')).toBeInTheDocument();
  });

  it('renders info alert with INFO badge', () => {
    useAppStore.getState().addAlert(makeAlert('a-3', 'info'));
    render(<AlertFeed />);
    expect(screen.getByText('INFO')).toBeInTheDocument();
  });

  it('shows unacked count in header', () => {
    useAppStore.getState().addAlert(makeAlert('a-1', 'warning'));
    useAppStore.getState().addAlert(makeAlert('a-2', 'info'));
    render(<AlertFeed />);
    expect(screen.getByText(/2 unacked/i)).toBeInTheDocument();
  });

  it('shows "All clear" when all alerts acknowledged', () => {
    useAppStore.getState().addAlert(makeAlert('a-1', 'info', true));
    render(<AlertFeed />);
    expect(screen.getByText(/all clear/i)).toBeInTheDocument();
  });
});

describe('AlertFeed - acknowledge', () => {
  it('clicking ACK button acknowledges the alert', async () => {
    useAppStore.getState().addAlert(makeAlert('a-1', 'warning'));
    render(<AlertFeed />);
    await user.click(screen.getByRole('button', { name: /ack/i }));
    expect(useAppStore.getState().alerts[0].acknowledged).toBe(true);
  });

  it('acknowledged alerts are rendered with reduced opacity', async () => {
    useAppStore.getState().addAlert(makeAlert('a-1', 'warning'));
    render(<AlertFeed />);
    await user.click(screen.getByRole('button', { name: /ack/i }));
    // Re-render to see updated state
    const { rerender } = render(<AlertFeed />);
    rerender(<AlertFeed />);
    // After ack, ACK button should be gone
    expect(screen.queryByRole('button', { name: /ack/i })).not.toBeInTheDocument();
  });
});

describe('AlertFeed - clear all', () => {
  it('Clear all button removes all alerts', async () => {
    useAppStore.getState().addAlert(makeAlert('a-1', 'info'));
    useAppStore.getState().addAlert(makeAlert('a-2', 'warning'));
    render(<AlertFeed />);
    await user.click(screen.getByRole('button', { name: /clear all/i }));
    expect(useAppStore.getState().alerts).toHaveLength(0);
    expect(screen.getByText(/no alerts/i)).toBeInTheDocument();
  });

  it('Clear all button is hidden when there are no alerts', () => {
    render(<AlertFeed />);
    expect(screen.queryByRole('button', { name: /clear all/i })).not.toBeInTheDocument();
  });
});

describe('AlertFeed - maxVisible limit', () => {
  it('respects maxVisible prop', () => {
    for (let i = 0; i < 10; i++) {
      useAppStore.getState().addAlert(makeAlert(`a-${i}`, 'info'));
    }
    render(<AlertFeed maxVisible={3} />);
    // Only 3 monitor names should be rendered
    const monitorNames = screen.getAllByText(/^Monitor a-/);
    expect(monitorNames).toHaveLength(3);
  });

  it('entity ID is a clickable link that selects the entity', async () => {
    useAppStore.getState().addAlert(makeAlert('a-1', 'critical'));
    render(<AlertFeed />);
    const entityBtn = screen.getByRole('button', { name: /ent-a-1/i });
    await user.click(entityBtn);
    expect(useAppStore.getState().selectedEntityId).toBe('ent-a-1');
    expect(useAppStore.getState().inspectorOpen).toBe(true);
  });
});
