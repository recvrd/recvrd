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
  return { api, store, track };
}

describe('FullPlayer', () => {
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
