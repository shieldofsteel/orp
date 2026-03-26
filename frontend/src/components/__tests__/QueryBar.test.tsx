import { describe, it, expect, beforeAll, afterAll, afterEach, vi } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { http, HttpResponse } from 'msw';
import { setupServer } from 'msw/node';
import React from 'react';
import { QueryBar } from '../QueryBar';
import { useAppStore } from '../../store/useAppStore';

// ── MSW Setup ─────────────────────────────────────────────────────────────────

const server = setupServer(
  http.post('/api/v1/query', async ({ request }) => {
    const body = await request.json() as { query: string };
    if (body.query.includes('FAIL')) {
      return HttpResponse.json({ error: { message: 'Syntax error in query' } }, { status: 400 });
    }
    return HttpResponse.json({
      results: [
        { id: 'e-1', name: 'Ship Atlas', speed: 15 },
        { id: 'e-2', name: 'Ship Borealis', speed: 22 },
      ],
    });
  }),
  http.post('/api/v1/query/natural', async () => {
    return HttpResponse.json({
      results: [{ id: 'e-3', name: 'Ship Caesar', speed: 18 }],
    });
  })
);

beforeAll(() => server.listen({ onUnhandledRequest: 'warn' }));
afterEach(() => {
  server.resetHandlers();
  useAppStore.setState({
    queryMode: 'structured',
    queryHistory: [],
    queryResults: [],
    queryLoading: false,
    queryError: null,
    lastQuery: '',
  });
});
afterAll(() => server.close());

// ── Helpers ───────────────────────────────────────────────────────────────────

const user = userEvent.setup({ delay: null });

function renderQueryBar() {
  return render(<QueryBar />);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('QueryBar - mode toggle', () => {
  it('renders ORP-QL and Natural mode buttons', () => {
    renderQueryBar();
    expect(screen.getByRole('button', { name: /orp-ql/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /natural/i })).toBeInTheDocument();
  });

  it('clicking Natural switches query mode', async () => {
    renderQueryBar();
    await user.click(screen.getByRole('button', { name: /natural/i }));
    expect(useAppStore.getState().queryMode).toBe('natural');
  });

  it('switching mode changes placeholder text', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    expect(textarea).toHaveAttribute('placeholder', expect.stringContaining('MATCH'));
    await user.click(screen.getByRole('button', { name: /natural/i }));
    expect(textarea).toHaveAttribute('placeholder', expect.stringContaining('Rotterdam'));
  });
});

describe('QueryBar - autocomplete suggestions', () => {
  it('shows suggestions as user types', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'MATCH');
    await waitFor(() => {
      expect(screen.getByText(/MATCH \(e:Ship\)/)).toBeInTheDocument();
    });
  });

  it('hides suggestions when input is cleared', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'MATCH');
    await waitFor(() => expect(screen.getByText(/MATCH \(e:Ship\)/)).toBeInTheDocument());
    await user.clear(textarea);
    await waitFor(() =>
      expect(screen.queryByText(/MATCH \(e:Ship\)/)).not.toBeInTheDocument()
    );
  });

  it('clicking a suggestion fills the input', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'MATCH');
    await waitFor(() => expect(screen.getByText(/MATCH \(e:Ship\)/)).toBeInTheDocument());
    const suggestion = screen.getAllByText(/MATCH \(e:Ship\)/)[0];
    // Use mousedown to avoid blur hiding the suggestions
    await user.pointer({ keys: '[MouseLeft>]', target: suggestion });
    expect((textarea as HTMLTextAreaElement).value).toBe('MATCH (e:Ship)');
  });
});

describe('QueryBar - keyboard navigation', () => {
  it('ArrowDown highlights next suggestion', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'MATCH');
    await waitFor(() => expect(screen.getByText(/MATCH \(e:Ship\)/)).toBeInTheDocument());
    await user.keyboard('{ArrowDown}');
    // First suggestion should be highlighted (bg-blue-900/50)
    const firstSuggestion = screen.getAllByRole('option').find(
      (b) => b.textContent?.includes('MATCH (e:Ship)')
    );
    expect(firstSuggestion?.className).toContain('bg-blue-900');
  });

  it('Escape closes suggestions', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'MATCH');
    await waitFor(() => expect(screen.getByText(/MATCH \(e:Ship\)/)).toBeInTheDocument());
    await user.keyboard('{Escape}');
    await waitFor(() =>
      expect(screen.queryByText(/MATCH \(e:Ship\)/)).not.toBeInTheDocument()
    );
  });
});

describe('QueryBar - query execution', () => {
  it('Execute button is disabled when input is empty', () => {
    renderQueryBar();
    const executeBtn = screen.getByRole('button', { name: /execute/i });
    expect(executeBtn).toBeDisabled();
  });

  it('clicking Execute submits query and shows results', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'MATCH (e:Ship) LIMIT 10');
    const executeBtn = screen.getByRole('button', { name: /execute/i });
    await user.click(executeBtn);
    await waitFor(() => {
      expect(screen.getByText('Ship Atlas')).toBeInTheDocument();
    });
    expect(screen.getByText('Ship Borealis')).toBeInTheDocument();
  });

  it('shows error message on query failure', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'FAIL QUERY');
    await user.click(screen.getByRole('button', { name: /execute/i }));
    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
    });
  });

  it('Cmd+Enter triggers query execution', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'MATCH (e:Ship) LIMIT 5');
    await user.keyboard('{Meta>}{Enter}{/Meta}');
    await waitFor(() => {
      expect(screen.getByText('Ship Atlas')).toBeInTheDocument();
    });
  });
});

describe('QueryBar - history', () => {
  it('shows history button after a successful query', async () => {
    renderQueryBar();
    const textarea = screen.getByRole('combobox');
    await user.type(textarea, 'MATCH (e:Port) LIMIT 5');
    await user.click(screen.getByRole('button', { name: /execute/i }));
    await waitFor(() => {
      expect(screen.getByText(/history/i)).toBeInTheDocument();
    });
  });

  it('clicking History shows past queries', async () => {
    // Pre-populate history in store
    useAppStore.getState().addQueryHistory({
      id: 'qh-1',
      query: 'MATCH (e:Ship) LIMIT 10',
      mode: 'structured',
      timestamp: new Date(),
      resultCount: 2,
    });
    renderQueryBar();
    await user.click(screen.getByText(/history \(1\)/i));
    expect(screen.getByText('MATCH (e:Ship) LIMIT 10')).toBeInTheDocument();
  });
});
