<script lang="ts">
  import type { AppStore } from '../store.svelte';
  import { downloadLabel, durationSeconds, formatDuration, coverUrl } from '../api';

  let { store }: { store: AppStore } = $props();

  let scrub = $state<number | null>(null);

  const playback = $derived(store.status?.playback ?? null);
  const track = $derived(playback?.current ?? null);
  const duration = $derived(track ? durationSeconds(track.duration) : 0);
  const position = $derived(scrub ?? playback?.position_secs ?? 0);
  const download = $derived(track ? store.downloadStatusFor(track) : null);
  const isPlaying = $derived(playback?.state === 'playing');

  function commitSeek(value: number): void {
    scrub = null;
    void store.seek(value);
  }
</script>

<footer class="now-playing {store.playerLayout}" aria-label="Now playing">
  <div class="seek-container">
    <span class="time">{formatDuration(position)}</span>
    <input
      type="range"
      min="0"
      max={Math.max(duration, 1)}
      step="1"
      value={position}
      disabled={!track || duration === 0}
      aria-label="Seek"
      aria-valuetext={`${formatDuration(position)} of ${formatDuration(duration)}`}
      onpointerdown={(event) => {
        scrub = Number(event.currentTarget.value);
      }}
      oninput={(event) => {
        scrub = Number(event.currentTarget.value);
      }}
      onchange={(event) => commitSeek(Number(event.currentTarget.value))}
    />
    <span class="time">{formatDuration(duration)}</span>
  </div>

  <div class="track">
    {#if track}
      <img
        class="cover"
        src={coverUrl(track.video_id)}
        alt=""
        loading="lazy"
        onload={(event) => {
          (event.currentTarget as HTMLImageElement).style.display = '';
        }}
        onerror={(event) => {
          (event.currentTarget as HTMLImageElement).style.display = 'none';
        }}
      />
      <div class="track-text">
        <div class="title" title={track.title}>{track.title}</div>
        <div class="meta" title={`${track.artist} — ${track.album}`}>
          {track.artist}
          {#if track.album}<span class="muted"> · {track.album}</span>{/if}
        </div>
        {#if store.playerLayout === 'terminal' && store.queue?.current != null}
          <div class="queue-position">
            Track {store.queue.current + 1} of {store.queue.items.length}
          </div>
        {/if}
        {#if download && download.state === 'failed'}
          <button class="retry" onclick={() => store.retryDownload(track.video_id)}>
            Download failed — retry
          </button>
        {:else if download}
          <span class="badge {download.state}">{downloadLabel(download)}</span>
        {/if}
      </div>
    {:else}
      <div class="track-text">
        <div class="title muted">Nothing playing</div>
        <div class="meta muted">Select a track and press Enter.</div>
      </div>
    {/if}
  </div>

  <div class="transport">
    <button
      class="icon"
      class:active={playback?.shuffle}
      aria-label="Shuffle"
      aria-pressed={playback?.shuffle ?? false}
      onclick={() => store.toggleShuffle()}
    >
      ⤨
    </button>
    <button class="icon nav" aria-label="Previous track" onclick={() => store.previous()}>
      {store.playerLayout === 'terminal' ? '<' : '⏮'}
    </button>
    <button
      class="icon primary"
      aria-label={isPlaying ? 'Pause' : 'Play'}
      onclick={() => store.togglePlayback()}
    >
      {#if store.playerLayout === 'terminal'}
        <span class="play-text">{isPlaying ? 'pause' : 'play'}</span>
      {/if}
      <span class="play-icon" aria-hidden="true">{isPlaying ? '⏸' : '▶'}</span>
    </button>
    <button class="icon nav" aria-label="Next track" onclick={() => store.next()}>
      {store.playerLayout === 'terminal' ? '>' : '⏭'}
    </button>
    <button
      class="icon repeat"
      class:active={playback && playback.repeat !== 'off'}
      aria-label={`Repeat mode: ${playback?.repeat ?? 'off'}`}
      onclick={() => store.cycleRepeat()}
    >
      ↻{#if playback?.repeat === 'one'}<sup>1</sup>{/if}
    </button>
  </div>

  <div class="volume">
    <span aria-hidden="true">🔊</span>
    <input
      type="range"
      min="0"
      max="100"
      step="1"
      value={store.volume}
      aria-label="Volume"
      oninput={(event) => {
        store.volume = Number(event.currentTarget.value);
      }}
      onchange={(event) => store.setVolume(Number(event.currentTarget.value))}
    />
    <span class="percent">{store.volume}%</span>
    <button
      class="layout-toggle"
      aria-label="Switch player layout"
      title="Switch player layout"
      onclick={() => store.togglePlayerLayout()}
    >
      {store.playerLayout === 'compact' ? 'terminal' : 'bar'}
    </button>
  </div>
</footer>

<style>
  .now-playing {
    display: grid;
    grid-template-columns: minmax(12rem, 1fr) minmax(18rem, 1.6fr) minmax(9rem, 0.8fr);
    grid-template-rows: auto auto;
    grid-template-areas:
      'track transport volume'
      'track seek volume';
    align-items: center;
    gap: 0.2rem 1rem;
    padding: 0.6rem 1rem;
    border-top: 2px dashed var(--border);
    background:
      linear-gradient(180deg, color-mix(in srgb, var(--accent) 6%, transparent), transparent),
      color-mix(in srgb, var(--bg-raised) 55%, transparent);
    backdrop-filter: blur(4px);
    flex: none;
    min-height: 5.2rem;
  }

  .now-playing.terminal {
    grid-template-columns: minmax(12rem, 1fr) auto minmax(10rem, 0.8fr);
    grid-template-areas:
      'seek seek seek'
      'track transport volume';
    gap: 0.35rem 1rem;
  }

  .track {
    grid-area: track;
    min-width: 0;
    display: flex;
    flex-direction: row;
    align-items: center;
    gap: 0.65rem;
  }

  .track-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    align-items: flex-start;
  }

  .cover {
    width: 2.75rem;
    height: 2.75rem;
    flex: none;
    object-fit: cover;
    border: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
    border-radius: 3px;
    background: color-mix(in srgb, var(--bg-raised) 70%, transparent);
  }

  .title {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--accent);
    text-shadow: 0 0 8px color-mix(in srgb, var(--accent) 35%, transparent);
    font-weight: 600;
    font-size: 0.92rem;
  }

  .meta {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.76rem;
    color: var(--fg-muted);
  }

  .queue-position {
    font-size: 0.68rem;
    color: var(--accent-2);
    opacity: 0.8;
  }

  .seek-container {
    grid-area: seek;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .seek-container input[type='range'] {
    flex: 1;
    height: 4px;
  }

  .transport {
    grid-area: transport;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
  }

  .icon {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1rem;
    padding: 0.25rem 0.45rem;
    border-radius: 4px;
  }

  .icon:hover {
    color: var(--accent);
    text-shadow: 0 0 8px var(--accent);
  }

  .icon.primary {
    color: var(--bg);
    background: var(--accent);
    box-shadow: 0 0 14px color-mix(in srgb, var(--accent) 45%, transparent);
    width: 2.2rem;
    height: 2.2rem;
    display: grid;
    place-items: center;
  }

  .icon.primary:hover {
    background: var(--accent-2);
    color: var(--bg);
    box-shadow: 0 0 14px color-mix(in srgb, var(--accent-2) 55%, transparent);
  }

  .icon.active {
    color: var(--accent);
  }

  .now-playing.terminal .icon.primary {
    width: auto;
    min-width: 6.5rem;
    height: 2.2rem;
    padding: 0.35rem 0.8rem;
    display: flex;
    gap: 0.45rem;
    font-size: 0.85rem;
  }

  .now-playing.terminal .icon.nav {
    border: 1px dashed var(--border);
    background: color-mix(in srgb, var(--bg-raised) 33%, transparent);
    min-width: 2.1rem;
  }

  .play-text {
    text-transform: lowercase;
  }

  .time {
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
    min-width: 2.4rem;
    text-align: center;
  }

  .volume {
    grid-area: volume;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.45rem;
    flex-wrap: wrap;
  }

  .volume input[type='range'] {
    width: 6rem;
    height: 4px;
  }

  .percent {
    font-size: 0.72rem;
    color: var(--fg-muted);
    min-width: 2.4rem;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .layout-toggle {
    padding: 0.08rem 0.4rem;
    font-size: 0.65rem;
    color: var(--fg-muted);
  }

  .layout-toggle:hover {
    color: var(--accent);
  }

  .retry {
    padding: 0.08rem 0.45rem;
    font-size: 0.7rem;
    border-color: var(--danger);
    color: var(--danger);
    background: transparent;
  }

  @media (max-width: 860px) {
    .now-playing {
      grid-template-columns: 1fr;
      grid-template-areas:
        'track'
        'transport'
        'seek'
        'volume';
      gap: 0.35rem;
      padding: 0.5rem 0.75rem;
    }

    .now-playing.terminal {
      grid-template-areas:
        'seek'
        'track'
        'transport'
        'volume';
    }

    .volume {
      justify-content: flex-start;
    }
  }
</style>
