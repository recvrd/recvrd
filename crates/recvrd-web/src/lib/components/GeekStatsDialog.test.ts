import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import GeekStatsDialog from './GeekStatsDialog.svelte';
import { AppStore } from '../store.svelte';
import { makeApi, makeTrack, makeTrackStats } from '../../test/fixtures';

function makeStore() {
  const api = makeApi();
  const store = new AppStore(api);
  const track = makeTrack('v1', {
    title: 'Stats Song',
    artist: 'Stats Artist',
    album: 'Stats Album',
  });
  store.geekStatsOpen = true;
  store.geekStatsTrack = track;
  store.geekStats = makeTrackStats(track);
  return { store, track };
}

describe('GeekStatsDialog', () => {
  it('renders the technical facts', () => {
    const { store } = makeStore();
    render(GeekStatsDialog, { props: { store } });

    const dialog = screen.getByRole('dialog', { name: 'Geek stats' });
    expect(within(dialog).getByText('Stats Song')).toBeInTheDocument();
    expect(within(dialog).getByText('youtube:v1')).toBeInTheDocument();
    expect(within(dialog).getByText('192 kbps')).toBeInTheDocument();
    expect(within(dialog).getByText('44100 Hz')).toBeInTheDocument();
    expect(within(dialog).getByText('2 channels')).toBeInTheDocument();
    expect(within(dialog).getByText('download')).toBeInTheDocument();
    expect(within(dialog).getByText('Play count')).toBeInTheDocument();
  });

  it('closes from the close button', async () => {
    const { store } = makeStore();
    render(GeekStatsDialog, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Close geek stats' }));
    expect(store.geekStatsOpen).toBe(false);
  });
});
