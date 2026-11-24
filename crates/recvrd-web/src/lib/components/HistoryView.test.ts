import { fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, it, vi } from 'vitest';
import HistoryView from './HistoryView.svelte';
import { AppStore } from '../store.svelte';
import { makeApi, makeHistoryEntry } from '../../test/fixtures';

function makeStore(entries = [makeHistoryEntry()]) {
  const api = makeApi();
  const store = new AppStore(api);
  store.history = entries;
  return { api, store };
}

describe('HistoryView', () => {
  it('shows recorded plays with play time and listened time', () => {
    const { store } = makeStore();
    render(HistoryView, { props: { store } });

    expect(screen.getByText('Track v1')).toBeInTheDocument();
    expect(screen.getByText('Artist')).toBeInTheDocument();
    expect(screen.getByText('Album')).toBeInTheDocument();
    expect(screen.getByText('1 hr ago')).toBeInTheDocument();
    expect(screen.getByText('3:00 of 3:00')).toBeInTheDocument();
  });

  it('marks plays that ended early', () => {
    const entry = makeHistoryEntry({ completed: false, played_secs: 45 });
    const { store } = makeStore([entry]);
    render(HistoryView, { props: { store } });

    expect(screen.getByText(/left early/)).toBeInTheDocument();
    expect(screen.getByText('0:45 of 3:00')).toBeInTheDocument();
  });

  it('shows an empty state', () => {
    const { store } = makeStore([]);
    render(HistoryView, { props: { store } });

    expect(screen.getByText('No plays recorded yet.')).toBeInTheDocument();
  });

  it('reloads history on button click', async () => {
    const { api, store } = makeStore();
    api.history = vi.fn(async () => []);
    render(HistoryView, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Reload recently played' }));
    await tick();
    expect(api.history).toHaveBeenCalledOnce();
    expect(store.history).toEqual([]);
  });
});
