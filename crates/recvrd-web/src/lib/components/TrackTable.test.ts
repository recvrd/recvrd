import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import TrackTable from './TrackTable.svelte';
import { downloadStatus, makeTrack } from '../../test/fixtures';

function props(tracks = [makeTrack('v1'), makeTrack('v2')]) {
  return {
    tracks,
    selection: 0,
    currentTrack: tracks[0] ?? null,
    downloadStatus: () => downloadStatus('not_downloaded'),
    onselect: vi.fn(),
    onplay: vi.fn(),
    onqueue: vi.fn(),
    onretry: vi.fn(),
  };
}

describe('TrackTable', () => {
  it('renders the required columns and rows', () => {
    const { container } = render(TrackTable, { props: props() });

    expect(screen.getByText('Title')).toBeInTheDocument();
    expect(screen.getAllByText('Artist')).toHaveLength(3);
    expect(screen.getAllByText('Album')).toHaveLength(3);
    expect(screen.getByText('Time')).toBeInTheDocument();
    expect(screen.getByText('Status')).toBeInTheDocument();
    expect(container.querySelectorAll('.row')).toHaveLength(2);
    expect(container.querySelector('.row.playing')).not.toBeNull();
  });

  it('loads cover art for each row', () => {
    const { container } = render(TrackTable, { props: props() });
    const covers = container.querySelectorAll<HTMLImageElement>('.row .cover img');

    expect(covers).toHaveLength(2);
    expect(covers[0].getAttribute('src')).toBe('/api/tracks/youtube/v1/cover');
    expect(covers[0]).toHaveAttribute('loading', 'lazy');
  });

  it('hides a cover that fails to load and shows it again on load', async () => {
    const { container } = render(TrackTable, { props: props() });
    const cover = container.querySelector<HTMLImageElement>('.row .cover img')!;

    await fireEvent.error(cover);
    expect(cover.style.display).toBe('none');

    await fireEvent.load(cover);
    expect(cover.style.display).toBe('');
  });

  it('selects, plays and opens the context menu', async () => {
    const actions = props();
    const { container } = render(TrackTable, { props: actions });
    const rows = container.querySelectorAll('.row');

    await fireEvent.click(rows[1]);
    expect(actions.onselect).toHaveBeenCalledWith(1);

    await fireEvent.dblClick(rows[1]);
    expect(actions.onplay).toHaveBeenCalledWith(actions.tracks[1]);

    await fireEvent.contextMenu(rows[0]);
    expect(screen.getByRole('menu')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('menuitem', { name: 'Add to queue' }));
    expect(actions.onqueue).toHaveBeenCalledWith(actions.tracks[0]);
  });

  it('adds playlist actions to the context menu', async () => {
    const actions = {
      ...props(),
      onremove: vi.fn(),
      onmoveup: vi.fn(),
      onmovedown: vi.fn(),
    };
    const { container } = render(TrackTable, { props: actions });

    await fireEvent.contextMenu(container.querySelectorAll('.row')[0]);
    expect(screen.getByRole('menuitem', { name: 'Move up' })).toBeDisabled();
    expect(screen.getByRole('menuitem', { name: 'Move down' })).not.toBeDisabled();
    await fireEvent.click(screen.getByRole('menuitem', { name: 'Move down' }));
    expect(actions.onmovedown).toHaveBeenCalledWith(0);

    await fireEvent.contextMenu(container.querySelectorAll('.row')[0]);
    await fireEvent.click(screen.getByRole('menuitem', { name: 'Remove from playlist' }));
    expect(actions.onremove).toHaveBeenCalledWith(0);
  });

  it('disables retry unless the download failed', async () => {
    const actions = props();
    const { container } = render(TrackTable, { props: actions });
    await fireEvent.contextMenu(container.querySelectorAll('.row')[0]);
    expect(screen.getByRole('menuitem', { name: 'Retry download' })).toBeDisabled();
  });

  it('retries a failed download from the badge', async () => {
    const actions = props([makeTrack('v1')]);
    actions.downloadStatus = () => downloadStatus('failed');
    const { container } = render(TrackTable, { props: actions });

    const retry = screen.getByRole('button', { name: 'Retry download for Track v1' });
    expect(retry).toHaveTextContent('failed');
    await fireEvent.click(retry);
    expect(actions.onretry).toHaveBeenCalledWith(makeTrack('v1'));
    expect(container.querySelector('.row')).not.toBeNull();
  });

  it('windows large playlists', () => {
    const tracks = Array.from({ length: 300 }, (_, index) => makeTrack(`v${index}`));
    const { container } = render(TrackTable, {
      props: { ...props(tracks), selection: 0 },
    });
    expect(container.querySelectorAll('.row').length).toBeLessThan(60);
  });

  it('greys out and explains tracks that are not cached offline', async () => {
    const tracks = [makeTrack('v1'), makeTrack('v2', { cached: true })];
    const actions = {
      ...props(tracks),
      offline: true,
      onunavailable: vi.fn(),
    };
    const { container } = render(TrackTable, { props: actions });

    const rows = container.querySelectorAll('.row');
    expect(rows[0]).toHaveClass('unavailable');
    expect(rows[1]).not.toHaveClass('unavailable');
    await fireEvent.click(
      screen.getByRole('button', { name: 'Why Track v1 is unavailable offline' }),
    );
    expect(actions.onunavailable).toHaveBeenCalledWith(tracks[0]);
  });
});
