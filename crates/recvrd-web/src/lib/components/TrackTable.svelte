<script lang="ts">
  import type { DownloadStatus } from '@api/DownloadStatus';
  import type { Track } from '@api/Track';
  import { downloadLabel, formatDuration, durationSeconds, coverUrl, trackMetadataFacts } from '../api';
  import type { MenuItem } from '../types';
  import ContextMenu from './ContextMenu.svelte';

  let {
    tracks,
    selection,
    currentTrack,
    downloadStatus,
    onselect,
    onplay,
    onqueue,
    onqueuenext,
    ongeekstats,
    onretry,
    onlike,
    onremove,
    onmoveup,
    onmovedown,
    offline = false,
    onunavailable,
    emptyMessage = 'No tracks here.',
  }: {
    tracks: Track[];
    selection: number;
    currentTrack: Track | null;
    downloadStatus: (track: Track) => DownloadStatus;
    onselect: (index: number) => void;
    onplay: (track: Track) => void;
    onqueue: (track: Track) => void;
    onqueuenext: (track: Track) => void;
    ongeekstats: (track: Track) => void;
    onretry: (track: Track) => void;
    onlike?: (track: Track) => void;
    onremove?: (index: number) => void;
    onmoveup?: (index: number) => void;
    onmovedown?: (index: number) => void;
    offline?: boolean;
    onunavailable?: (track: Track) => void;
    emptyMessage?: string;
  } = $props();

  const ROW_HEIGHT = 44;
  const OVERSCAN = 5;

  let scrollEl: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportHeight = $state(480);
  let menu = $state<{ x: number; y: number; index: number; track: Track } | null>(null);

  const start = $derived(Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN));
  const end = $derived(
    Math.min(tracks.length, Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + OVERSCAN),
  );
  const visible = $derived(tracks.slice(start, end));

  $effect(() => {
    if (!scrollEl) return;
    viewportHeight = scrollEl.clientHeight || 480;
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      viewportHeight = scrollEl?.clientHeight || 480;
    });
    observer.observe(scrollEl);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (!scrollEl || selection < 0) return;
    const top = selection * ROW_HEIGHT;
    const bottom = top + ROW_HEIGHT;
    if (top < scrollEl.scrollTop) {
      scrollEl.scrollTop = top;
    } else if (bottom > scrollEl.scrollTop + viewportHeight) {
      scrollEl.scrollTop = bottom - viewportHeight;
    }
  });

  function buildMenu(): MenuItem[] {
    if (!menu) return [];
    const items: MenuItem[] = [
      { label: 'Play now', action: () => onplay(menu!.track) },
      { label: 'Queue next', action: () => onqueuenext(menu!.track) },
      { label: 'Queue last', action: () => onqueue(menu!.track) },
      { label: 'Geek stats', action: () => ongeekstats(menu!.track) },
      {
        label: 'Retry download',
        disabled: downloadStatus(menu.track).state !== 'failed',
        action: () => onretry(menu!.track),
      },
    ];
    if (onlike) {
      items.push({
        label: menu.track.liked === true ? 'Unlike' : 'Like',
        action: () => onlike(menu!.track),
      });
    }
    if (onmoveup) {
      items.push({
        label: 'Move up',
        disabled: menu.index === 0,
        action: () => onmoveup(menu!.index),
      });
    }
    if (onmovedown) {
      items.push({
        label: 'Move down',
        disabled: menu.index === tracks.length - 1,
        action: () => onmovedown(menu!.index),
      });
    }
    if (onremove) {
      items.push({
        label: 'Remove from playlist',
        action: () => onremove(menu!.index),
      });
    }
    return items;
  }

  const menuItems = $derived(buildMenu());

  function openMenu(event: MouseEvent, index: number, track: Track): void {
    event.preventDefault();
    onselect(index);
    menu = { x: event.clientX, y: event.clientY, index, track };
  }

  function openMenuAtButton(event: MouseEvent, index: number, track: Track): void {
    event.stopPropagation();
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    onselect(index);
    menu = { x: rect.left, y: rect.bottom + 4, index, track };
  }

  function badgeClass(status: DownloadStatus): string {
    return status.state;
  }
</script>

{#if tracks.length === 0}
  <div class="empty-state">{emptyMessage}</div>
{:else}
  <div class="table-header" role="row">
    <span class="index">#</span>
    <span class="cover-header" aria-hidden="true"></span>
    <span>Title</span>
    <span>Artist</span>
    <span class="optional">Album</span>
    <span class="time">Time</span>
    <span class="status">Status</span>
    <span></span>
  </div>
  <div
    class="scroll"
    bind:this={scrollEl}
    onscroll={() => {
      scrollTop = scrollEl?.scrollTop ?? 0;
    }}
  >
    <div class="spacer" style:height={`${tracks.length * ROW_HEIGHT}px`}>
      {#each visible as track, offset (start + offset)}
        {@const index = start + offset}
        {@const download = downloadStatus(track)}
        {@const playing = currentTrack?.track_id === track.track_id}
        {@const unavailable = offline && track.cached !== true}
        <div
          class="row"
          class:selected={index === selection}
          class:playing
          class:unavailable
          role="row"
          tabindex={index === selection ? 0 : -1}
          aria-selected={index === selection}
          style:top={`${index * ROW_HEIGHT}px`}
          style:height={`${ROW_HEIGHT}px`}
          onclick={() => onselect(index)}
          ondblclick={() => onplay(track)}
          oncontextmenu={(event) => openMenu(event, index, track)}
          onkeydown={(event) => {
            if (event.key === 'Enter' || event.key === ' ') event.preventDefault();
          }}
        >
          <span class="index">
            {#if playing}<span class="play-marker" aria-label="Now playing">▶</span>{/if}
            {index + 1}
          </span>
          <span class="cover">
            <img
              src={coverUrl(track)}
              alt=""
              loading="lazy"
              onload={(event) => {
                (event.currentTarget as HTMLImageElement).style.display = '';
              }}
              onerror={(event) => {
                (event.currentTarget as HTMLImageElement).style.display = 'none';
              }}
            />
          </span>
          <span class="title" title={track.title}>
            {track.title}
            {#if track.liked === true}
              <span class="liked" role="img" aria-label="Liked" title="Liked">♥</span>
            {/if}
            {#if unavailable}
              <button
                class="info"
                title="Not cached for offline mode"
                aria-label={`Why ${track.title} is unavailable offline`}
                onclick={(event) => {
                  event.stopPropagation();
                  onunavailable?.(track);
                }}
              >
                i
              </button>
            {/if}
          </span>
          <span class="artist" title={track.artist}>{track.artist}</span>
          <span class="optional album" title={track.album}>
            {[track.album, ...trackMetadataFacts(track)].filter(Boolean).join(' · ')}
          </span>
          <span class="time">{formatDuration(durationSeconds(track.duration))}</span>
          <span class="status">
            {#if download.state === 'failed'}
              <button
                class="badge failed"
                title="Retry download"
                aria-label={`Retry download for ${track.title}`}
                onclick={(event) => {
                  event.stopPropagation();
                  onretry(track);
                }}
              >
                {downloadLabel(download)} ↻
              </button>
            {:else}
              <span class="badge {badgeClass(download)}">{downloadLabel(download)}</span>
            {/if}
          </span>
          <button
            class="more"
            aria-label={`Actions for ${track.title}`}
            title="Actions"
            onclick={(event) => openMenuAtButton(event, index, track)}
          >
            ⋯
          </button>
        </div>
      {/each}
    </div>
  </div>
{/if}

{#if menu}
  <ContextMenu x={menu.x} y={menu.y} items={menuItems} onclose={() => (menu = null)} />
{/if}

<style>
  .table-header,
  .row {
    display: grid;
    grid-template-columns:
      3rem 2.25rem minmax(0, 2.2fr) minmax(0, 1.4fr) minmax(0, 1.2fr) 4rem 5.5rem
      2rem;
    align-items: center;
    gap: 0.6rem;
    padding: 0 0.75rem;
  }

  .table-header {
    height: 2rem;
    border-bottom: 1px dashed var(--border);
    color: var(--fg-muted);
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    flex: none;
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .spacer {
    position: relative;
  }

  .row {
    position: absolute;
    left: 0;
    right: 0;
    border-bottom: 1px dotted color-mix(in srgb, var(--border) 35%, transparent);
    font-size: 0.84rem;
    cursor: default;
  }

  .row:hover {
    background: color-mix(in srgb, var(--selection) 55%, transparent);
  }

  .row.selected {
    background: var(--selection);
    box-shadow: inset 3px 0 0 var(--accent-2);
  }

  .row.playing .title,
  .row.playing .index {
    color: var(--accent);
  }

  .row.unavailable {
    opacity: 0.5;
  }

  .info {
    width: 1.05rem;
    height: 1.05rem;
    margin-left: 0.35rem;
    padding: 0;
    border: 1px solid var(--warning);
    border-radius: 50%;
    background: transparent;
    color: var(--warning);
    font-size: 0.66rem;
    line-height: 1;
    vertical-align: middle;
  }

  .index,
  .time,
  .status {
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }

  .title {
    color: var(--fg-bright);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .liked {
    margin-left: 0.4rem;
    color: var(--accent);
    font-weight: 400;
  }

  .cover {
    width: 2rem;
    height: 2rem;
    overflow: hidden;
    border: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
    border-radius: 2px;
    background:
      linear-gradient(135deg, color-mix(in srgb, var(--border) 18%, transparent), transparent),
      color-mix(in srgb, var(--bg-raised) 70%, transparent);
  }

  .cover img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .artist,
  .album {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-muted);
  }

  .time {
    text-align: right;
  }

  .play-marker {
    color: var(--accent);
    margin-right: 0.2rem;
    font-size: 0.65rem;
  }

  .badge {
    font-size: 0.68rem;
    padding: 0.05rem 0.4rem;
  }

  button.badge {
    background: transparent;
  }

  .more {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    padding: 0 0.2rem;
    opacity: 0;
  }

  .row:hover .more,
  .row.selected .more,
  .more:focus-visible {
    opacity: 1;
  }

  @media (max-width: 900px) {
    .table-header,
    .row {
      grid-template-columns:
        2.5rem 2rem minmax(0, 2fr) minmax(0, 1.2fr) 3.5rem 4.8rem 2rem;
    }

    .optional {
      display: none;
    }
  }
</style>
