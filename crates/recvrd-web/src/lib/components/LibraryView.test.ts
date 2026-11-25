import { fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, it, vi } from 'vitest';
import LibraryView from './LibraryView.svelte';
import { AppStore } from '../store.svelte';
import {
  makeApi,
  makePlaylist,
  makePlaylistDetail,
  makeProviderInfo,
  makeRssFeed,
  makeStatus,
  makeTrack,
} from '../../test/fixtures';

function setupLibrary() {
  const api = makeApi();
  const store = new AppStore(api);
  store.view = { kind: 'library' };
  store.playlists = [makePlaylist('p1')];
  return { api, store };
}

function setupRssLibrary() {
  const api = makeApi();
  const store = new AppStore(api);
  // Keep one feed card across reloads: the mock playlist list
  // ignores the provider, so scope it to the feed here.
  api.playlists = vi.fn(async () => [
    makePlaylist('0123456789abcdef', { name: 'Test Tones' }),
  ]);
  store.providers = [
    makeProviderInfo(),
    makeProviderInfo({
      id: 'rss',
      name: 'RSS',
      default: false,
      capabilities: {
        search: true,
        library: true,
        playlists: true,
        playlist_create: false,
        playlist_edit: false,
        downloads: true,
        covers: true,
      },
      auth: { state: 'ok', source: null, message: null },
    }),
  ];
  store.activeProvider = 'rss';
  store.view = { kind: 'library' };
  store.playlists = [makePlaylist('0123456789abcdef', { name: 'Test Tones' })];
  store.rssFeeds = [makeRssFeed()];
  return { api, store };
}

function setupPlaylist() {
  const api = makeApi();
  const store = new AppStore(api);
  const playlist = makePlaylist('p1');
  store.view = { kind: 'playlist', id: 'p1', name: playlist.name };
  store.playlist = makePlaylistDetail(playlist, [makeTrack('v1'), makeTrack('v2')]);
  store.playlists = [playlist];
  return { api, store };
}

describe('LibraryView', () => {
  it('shows the current track art on the dashboard and opens the full player', async () => {
    const { store } = setupLibrary();
    const track = makeTrack('v9', {
      title: 'Dashboard Song',
      artist: 'Dashboard Artist',
      album: 'Dashboard Album',
    });
    store.status = makeStatus({
      playback: {
        state: 'playing',
        current: track,
        position_secs: 30,
        volume: 50,
        repeat: 'off',
        shuffle: false,
      },
    });
    const { container } = render(LibraryView, { props: { store } });

    expect(
      screen.getByRole('heading', { level: 2, name: 'Dashboard Song' }),
    ).toBeInTheDocument();
    expect(container.querySelector<HTMLImageElement>('.np-art img')?.getAttribute('src')).toBe(
      '/api/tracks/youtube/v9/cover',
    );

    await fireEvent.click(screen.getByRole('button', { name: 'Full player' }));
    expect(store.fullPlayerOpen).toBe(true);
  });

  it('creates a playlist from the library', async () => {
    const { api, store } = setupLibrary();
    const { container } = render(LibraryView, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'New playlist' }));
    await fireEvent.input(screen.getByLabelText('New playlist name'), {
      target: { value: 'Mix' },
    });
    await fireEvent.submit(container.querySelector('form')!);

    expect(api.createPlaylist).toHaveBeenCalledWith({
      title: 'Mix',
      description: '',
      privacy: 'private',
    });
    expect(api.playlists).toHaveBeenCalled();
  });

  it('adds a feed from the RSS library', async () => {
    const { api, store } = setupRssLibrary();
    const { container } = render(LibraryView, { props: { store } });

    expect(screen.getByRole('button', { name: 'Add feed' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Refresh feeds' })).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Add feed' }));
    await fireEvent.input(screen.getByLabelText('Feed URL'), {
      target: { value: 'https://example.com/other.xml' },
    });
    await fireEvent.submit(container.querySelector('form')!);

    expect(api.addRssFeed).toHaveBeenCalledWith('https://example.com/other.xml');
  });

  it('refreshes and removes a feed from its card', async () => {
    const { api, store } = setupRssLibrary();
    render(LibraryView, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(api.refreshRssFeed).toHaveBeenCalledWith('0123456789abcdef');

    await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Remove?' }));
    expect(api.removeRssFeed).toHaveBeenCalledWith('0123456789abcdef');
  });

  it('renames and deletes a playlist with confirmation', async () => {
    const { api, store } = setupPlaylist();
    render(LibraryView, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    const input = screen.getByLabelText('Playlist name');
    await fireEvent.input(input, { target: { value: 'New name' } });
    await fireEvent.submit(input.closest('form')!);
    await tick();

    expect(api.updatePlaylist).toHaveBeenCalledWith('p1', { title: 'New name' });
    expect(store.playlist?.playlist.name).toBe('New name');

    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect(screen.getByText(/Delete “New name”\?/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect(api.deletePlaylist).toHaveBeenCalledWith('p1');
  });

  it('shows the library name when the daemon returns the raw playlist ID', () => {
    const { store } = setupPlaylist();
    const id = 'VLPLEuTNuVCEhNwBcYRtEfw52PMGYPpyX1v4';
    store.view = { kind: 'playlist', id, name: 'Christmas Classics' };
    store.playlist = makePlaylistDetail(makePlaylist(id, { name: id }), [
      makeTrack('v1'),
    ]);
    render(LibraryView, { props: { store } });

    expect(
      screen.getByRole('heading', { level: 1, name: 'Christmas Classics' }),
    ).toBeInTheDocument();
    expect(screen.queryByText(id)).not.toBeInTheDocument();
  });

  it('opens the search palette to add tracks', async () => {
    const { store } = setupPlaylist();
    render(LibraryView, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Add tracks' }));
    expect(store.searchMode).toBe('add-to-playlist');
    expect(store.searchPlaylistId).toBe('p1');
    expect(store.searchOpen).toBe(true);
  });

  it('moves a playlist track down', async () => {
    const { api, store } = setupPlaylist();
    const { container } = render(LibraryView, { props: { store } });

    await fireEvent.contextMenu(container.querySelectorAll('.row')[0]);
    await fireEvent.click(screen.getByRole('menuitem', { name: 'Move down' }));
    expect(api.movePlaylistTrack).toHaveBeenCalledWith('p1', 0, 1);
  });

  it('removes a playlist track after confirmation', async () => {
    const { api, store } = setupPlaylist();
    const { container } = render(LibraryView, { props: { store } });

    await fireEvent.contextMenu(container.querySelectorAll('.row')[0]);
    await fireEvent.click(screen.getByRole('menuitem', { name: 'Remove from playlist' }));
    expect(screen.getByText(/Remove “Track v1”/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
    await tick();

    expect(api.removePlaylistTracks).toHaveBeenCalledWith('p1', [0]);
    expect(store.playlist?.tracks).toHaveLength(1);
  });
});
