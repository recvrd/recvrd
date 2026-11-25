import { fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { beforeEach, describe, expect, it } from 'vitest';
import App from '../App.svelte';
import { AppStore } from '../lib/store.svelte';
import {
  FakeEventSource,
  makeApi,
  makePlaylist,
  makePlaylistDetail,
  makeProviderInfo,
  makeTrack,
} from '../test/fixtures';

function setup() {
  const api = makeApi();
  const sources: FakeEventSource[] = [];
  const store = new AppStore(api, {
    createEventSource: (url) => {
      const source = new FakeEventSource(url);
      sources.push(source);
      return source;
    },
  });
  return { api, store, sources };
}

describe('App shell', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/');
  });

  it('loads the library and responds to global keys', async () => {
    const { store, sources } = setup();
    render(App, { props: { store } });

    expect(
      await screen.findByRole('heading', { level: 2, name: 'Playlist p1' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 1, name: 'Library' })).toBeInTheDocument();
    sources[0].open();
    await tick();
    expect(store.connection).toBe('online');

    await fireEvent.keyDown(window, { key: '?' });
    await tick();
    expect(
      screen.getByRole('dialog', { name: 'Keyboard and mouse help' }),
    ).toBeInTheDocument();

    await fireEvent.keyDown(window, { key: 'Escape' });
    await tick();
    expect(
      screen.queryByRole('dialog', { name: 'Keyboard and mouse help' }),
    ).not.toBeInTheDocument();

    await fireEvent.keyDown(window, { key: '/' });
    await tick();
    expect(
      screen.getByRole('dialog', { name: 'Search YouTube Music' }),
    ).toBeInTheDocument();
  });

  it('handles the new global keys', async () => {
    const { store } = setup();
    render(App, { props: { store } });

    await screen.findByRole('heading', { level: 2, name: 'Playlist p1' });

    await fireEvent.keyDown(window, { key: 'Tab' });
    await tick();
    expect(store.focus).toBe('sidebar');

    await fireEvent.keyDown(window, { key: '2' });
    await tick();
    expect(store.view).toEqual({ kind: 'queue' });

    await fireEvent.keyDown(window, { key: '1' });
    await tick();
    expect(store.view).toEqual({ kind: 'library' });

    await fireEvent.keyDown(window, { key: 'Escape' });
    await tick();
    expect(store.focus).toBe('main');
  });

  it('opens a playlist and shows its tracks', async () => {
    const { store } = setup();
    render(App, { props: { store } });

    store.openPlaylist('p1');
    await tick();

    expect(
      await screen.findByRole('heading', { level: 1, name: 'Playlist p1' }),
    ).toBeInTheDocument();
    expect(await screen.findByText('Track v1')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Play all' })).toBeInTheDocument();
  });

  it('shows the library name when the daemon returns the raw playlist ID', async () => {
    const { api, store } = setup();
    render(App, { props: { store } });
    await screen.findByRole('heading', { level: 2, name: 'Playlist p1' });

    const id = 'VLPLEuTNuVCEhNwBcYRtEfw52PMGYPpyX1v4';
    api.playlist = async () =>
      makePlaylistDetail(makePlaylist(id, { name: id }), [makeTrack('v1')]);
    await store.openPlaylist(id, 'Christmas Classics');
    await tick();

    expect(
      screen.getByRole('heading', { level: 1, name: 'Christmas Classics' }),
    ).toBeInTheDocument();
    expect(screen.getAllByText('Christmas Classics')).toHaveLength(2);
    expect(screen.queryByText(id)).not.toBeInTheDocument();
  });

  it('toggles offline mode from the top bar', async () => {
    const { api, store } = setup();
    render(App, { props: { store } });
    await screen.findByRole('heading', { level: 2, name: 'Playlist p1' });

    const toggle = screen.getByRole('button', { name: 'Offline mode' });
    expect(toggle).toHaveTextContent('online');
    await fireEvent.click(toggle);
    await tick();
    expect(api.setOffline).toHaveBeenCalledWith(true);
    expect(store.offline).toBe(true);
  });

  it('shows the playback mode selector for a provider with several modes', async () => {
    const { api, store } = setup();
    api.providers = async () => [
      makeProviderInfo({
        id: 'rss',
        name: 'RSS',
        default: true,
        playback: 'download',
        playback_modes: ['download', 'stream', 'stream_then_download'],
      }),
    ];
    render(App, { props: { store } });

    const select = await screen.findByLabelText('Playback mode');
    expect(select).toHaveValue('download');
    await fireEvent.change(select, { target: { value: 'stream' } });
    await tick();
    expect(api.setPlaybackMode).toHaveBeenCalledWith('rss', 'stream');
  });

  it('hides the playback mode selector for a single-mode provider', async () => {
    const { store } = setup();
    render(App, { props: { store } });
    await screen.findByRole('heading', { level: 2, name: 'Playlist p1' });
    expect(screen.queryByLabelText('Playback mode')).not.toBeInTheDocument();
  });

  it('closes the full player with Escape', async () => {
    const { store } = setup();
    render(App, { props: { store } });

    await screen.findByRole('heading', { level: 2, name: 'Playlist p1' });
    store.openFullPlayer();
    await tick();
    expect(screen.getByRole('dialog', { name: 'Full player' })).toBeInTheDocument();

    await fireEvent.keyDown(window, { key: 'Escape' });
    await tick();
    expect(screen.queryByRole('dialog', { name: 'Full player' })).not.toBeInTheDocument();
  });
});
