import { fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, it } from 'vitest';
import SearchPalette from './SearchPalette.svelte';
import { AppStore } from '../store.svelte';
import {
  makeApi,
  makePlaylist,
  makeSearchResults,
  makeTrack,
} from '../../test/fixtures';

function makeStore() {
  const api = makeApi();
  const store = new AppStore(api);
  store.openSearch();
  store.searchQuery = 'test';
  store.searchResults = makeSearchResults(
    [makeTrack('v1'), makeTrack('v2')],
    [makePlaylist('p1')],
  );
  return { api, store };
}

describe('SearchPalette', () => {
  it('renders tracks and playlists and plays on Enter', async () => {
    const { api, store } = makeStore();
    render(SearchPalette, { props: { store } });

    expect(screen.getByRole('dialog', { name: 'Search YouTube Music' })).toBeInTheDocument();
    expect(screen.getByText('Track v1')).toBeInTheDocument();
    expect(screen.getByText('Playlist p1')).toBeInTheDocument();

    const input = screen.getByRole('searchbox');
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();
    expect(api.playTrack).toHaveBeenCalledWith(makeTrack('v1'));
    expect(store.searchOpen).toBe(false);
  });

  it('navigates with the arrow keys and opens a playlist', async () => {
    const { api, store } = makeStore();
    render(SearchPalette, { props: { store } });

    const input = screen.getByRole('searchbox');
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();

    expect(api.playlist).toHaveBeenCalledWith('p1', 'youtube');
    expect(store.view).toEqual({
      kind: 'playlist',
      provider: 'youtube',
      id: 'p1',
      name: 'Playlist p1',
    });
  });

  it('queues a track without closing the palette', async () => {
    const { api, store } = makeStore();
    render(SearchPalette, { props: { store } });

    await fireEvent.click(
      screen.getByRole('button', { name: 'Add Track v1 to the queue' }),
    );
    expect(api.queueAdd).toHaveBeenCalledWith(makeTrack('v1'), undefined);
    expect(store.searchOpen).toBe(true);
  });

  it('queues a track next without closing the palette', async () => {
    const { api, store } = makeStore();
    render(SearchPalette, { props: { store } });

    await fireEvent.click(
      screen.getByRole('button', { name: 'Queue Track v1 next' }),
    );
    expect(api.queueAdd).toHaveBeenCalledWith(makeTrack('v1'), 'next');
    expect(store.searchOpen).toBe(true);
  });

  it('adds tracks to a playlist in add mode', async () => {
    const { api, store } = makeStore();
    store.openSearchForPlaylist('p1');
    store.searchQuery = 'test';
    store.searchResults = makeSearchResults(
      [makeTrack('v1')],
      [makePlaylist('p1')],
    );
    render(SearchPalette, { props: { store } });

    expect(screen.getByText(/Add tracks to/)).toBeInTheDocument();
    expect(screen.queryByText('Playlist p1')).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Add Track v1 to the queue' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Queue Track v1 next' }),
    ).not.toBeInTheDocument();

    const input = screen.getByRole('searchbox');
    await fireEvent.keyDown(input, { key: 'Enter' });
    await tick();

    expect(api.addPlaylistTracks).toHaveBeenCalledWith('p1', 'youtube', ['v1']);
    expect(api.playTrack).not.toHaveBeenCalled();
    expect(store.searchOpen).toBe(true);
  });

  it('closes on Escape and reports loading', async () => {
    const { store } = makeStore();
    store.searchLoading = true;
    store.searchResults = null;
    render(SearchPalette, { props: { store } });

    expect(document.querySelectorAll('.result-skeleton')).toHaveLength(3);
    const input = screen.getByRole('searchbox');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(store.searchOpen).toBe(false);
  });

  it('shows channel counts and track metadata', () => {
    const { store } = makeStore();
    store.searchResults = makeSearchResults(
      [
        makeTrack('v1', {
          metadata: {
            channel_title: 'Test Channel',
            season_number: 1,
            episode_number: 2,
            year: 2026,
          },
        }),
      ],
      [makePlaylist('p1', { item_count: 12 })],
    );
    render(SearchPalette, { props: { store } });

    expect(screen.getByText('Artist · Test Channel · S1E2 · 2026')).toBeInTheDocument();
    expect(screen.getByText('YouTube Music · 12 items')).toBeInTheDocument();
  });

  it('shows the no-results state', () => {
    const { store } = makeStore();
    store.searchResults = makeSearchResults();
    render(SearchPalette, { props: { store } });
    expect(screen.getByText(/No results for/)).toBeInTheDocument();
  });
});
