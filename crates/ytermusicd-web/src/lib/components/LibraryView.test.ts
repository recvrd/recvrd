import { fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, it } from 'vitest';
import LibraryView from './LibraryView.svelte';
import { AppStore } from '../store.svelte';
import {
  makeApi,
  makePlaylist,
  makePlaylistDetail,
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
      '/api/tracks/v9/cover',
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
