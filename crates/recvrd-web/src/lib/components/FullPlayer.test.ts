import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, it } from 'vitest';
import FullPlayer from './FullPlayer.svelte';
import { AppStore } from '../store.svelte';
import { makeApi, makeQueue, makeStatus, makeTrack } from '../../test/fixtures';

function makeStore() {
  const api = makeApi();
  const store = new AppStore(api);
  const track = makeTrack('v1', {
    title: 'Full Player Song',
    artist: 'Artist',
    album: 'Album',
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
  store.queue = makeQueue([track, makeTrack('v2')], { current: 0 });
  store.fullPlayerOpen = true;
  store.visualizerOn = false;
  return { api, store, track };
}

describe('FullPlayer', () => {
  it('toggles the visualizer', async () => {
    const { store } = makeStore();
    render(FullPlayer, { props: { store } });
    const toggle = screen.getByRole('button', { name: 'Toggle visualizer' });

    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    await fireEvent.click(toggle);
    expect(store.visualizerOn).toBe(true);
    expect(toggle).toHaveAttribute('aria-pressed', 'true');
  });

  it('opens geek stats for the current track', async () => {
    const { api, store } = makeStore();
    render(FullPlayer, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Geek stats' }));
    expect(api.trackStats).toHaveBeenCalledWith('youtube', 'v1');
    expect(store.geekStatsOpen).toBe(true);
  });

  it('renders the track, cover art and queue position', () => {
    const { store } = makeStore();
    const { container } = render(FullPlayer, { props: { store } });

    const dialog = screen.getByRole('dialog', { name: 'Full player' });
    expect(
      within(dialog).getByRole('heading', { level: 1, name: 'Full Player Song' }),
    ).toBeInTheDocument();
    expect(within(dialog).getByText('Artist')).toBeInTheDocument();
    expect(within(dialog).getByText('Album')).toBeInTheDocument();
    expect(within(dialog).getByText('Track 1 of 2')).toBeInTheDocument();
    expect(container.querySelector<HTMLImageElement>('.art')?.getAttribute('src')).toBe(
      '/api/tracks/youtube/v1/cover',
    );
  });

  it('shows provider metadata, source and the loaded description', async () => {
    const api = makeApi();
    const store = new AppStore(api);
    const track = makeTrack('YvIRJrjZVe', {
      provider: 'podverse',
      title: 'Podcast Song',
      album: 'Rock The Halls',
      metadata: {
        channel_title: 'Rock The Halls',
        published_at: '2026-11-10T18:49:40.000Z',
        year: 2026,
        season_number: 1,
        episode_number: 2,
        genre: 'Music',
      },
    });
    store.status = makeStatus({
      playback: {
        state: 'playing',
        current: track,
        position_secs: 0,
        volume: 50,
        repeat: 'off',
        shuffle: false,
        source_kind: 'stream',
      },
    });
    store.queue = makeQueue([track], { current: 0 });
    store.fullPlayerOpen = true;
    render(FullPlayer, { props: { store } });

    const dialog = screen.getByRole('dialog', { name: 'Full player' });
    expect(within(dialog).getByText('S1E2 · 2026 · Music')).toBeInTheDocument();
    expect(within(dialog).getByText('Published 2026-11-10')).toBeInTheDocument();
    expect(within(dialog).getByText('streaming from the provider')).toBeInTheDocument();
    await fireEvent.click(await within(dialog).findByText('Description'));
    expect(await within(dialog).findByText('Description for Track YvIRJrjZVe')).toBeInTheDocument();
  });

  it('runs the transport controls', async () => {
    const { api, store } = makeStore();
    render(FullPlayer, { props: { store } });
    const dialog = screen.getByRole('dialog', { name: 'Full player' });

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Pause' }));
    expect(api.pause).toHaveBeenCalledOnce();

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Next track' }));
    expect(api.next).toHaveBeenCalledOnce();

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Previous track' }));
    expect(api.previous).toHaveBeenCalledOnce();

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Shuffle' }));
    expect(api.setShuffle).toHaveBeenCalledWith(true);
    await tick();
  });

  it('seeks and changes the volume', async () => {
    const { api, store } = makeStore();
    render(FullPlayer, { props: { store } });
    const dialog = screen.getByRole('dialog', { name: 'Full player' });

    const seek = within(dialog).getByLabelText('Seek');
    await fireEvent.input(seek, { target: { value: '60' } });
    await fireEvent.change(seek, { target: { value: '60' } });
    expect(api.seek).toHaveBeenCalledWith(60);

    const volume = within(dialog).getByLabelText('Volume');
    await fireEvent.input(volume, { target: { value: '70' } });
    await fireEvent.change(volume, { target: { value: '70' } });
    expect(api.setVolume).toHaveBeenCalledWith(70);
  });

  it('closes from the close button', async () => {
    const { store } = makeStore();
    render(FullPlayer, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Close full player' }));
    expect(store.fullPlayerOpen).toBe(false);
    await tick();
    expect(screen.queryByRole('dialog', { name: 'Full player' })).not.toBeInTheDocument();
  });

  it('handles the empty state', () => {
    const store = new AppStore(makeApi());
    store.status = makeStatus();
    store.fullPlayerOpen = true;
    render(FullPlayer, { props: { store } });

    expect(screen.getByText('Nothing playing')).toBeInTheDocument();
    expect(screen.getByLabelText('Seek')).toBeDisabled();
  });
});
