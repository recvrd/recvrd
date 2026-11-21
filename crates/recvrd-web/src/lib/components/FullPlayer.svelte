<script lang="ts">
  import type { AppStore } from '../store.svelte';
  import { coverUrl, durationSeconds, formatDuration } from '../api';

  let { store }: { store: AppStore } = $props();

  let scrub = $state<number | null>(null);

  const playback = $derived(store.status?.playback ?? null);
  const track = $derived(playback?.current ?? null);
  const duration = $derived(track ? durationSeconds(track.duration) : 0);
  const position = $derived(scrub ?? playback?.position_secs ?? 0);
  const isPlaying = $derived(playback?.state === 'playing');
  const queueIndex = $derived(store.queue?.current ?? null);
  const queueLength = $derived(store.queue?.items.length ?? 0);

  function commitSeek(value: number): void {
    scrub = null;
    void store.seek(value);
  }
</script>

{#if store.fullPlayerOpen}
  <div class="full-player" role="dialog" aria-modal="true" aria-label="Full player">
    {#if track}
      <div
        class="backdrop"
        style={`background-image: url(${coverUrl(track)})`}
        aria-hidden="true"
      ></div>
    {/if}

    <header class="full-header">
      <span class="prompt">$ now_playing</span>
      <button
        class="close"
        aria-label="Close full player"
        title="Close full player"
        onclick={() => store.closeFullPlayer()}
      >
        ×
      </button>
    </header>

    <div class="stage">
      <div class="art-wrap" aria-hidden="true">
        <span class="art-fallback">{track ? track.title.slice(0, 1).toUpperCase() : '♪'}</span>
        {#if track}
          <img
            class="art"
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
        {/if}
      </div>

      <div class="details">
        {#if track}
          <p class="eyebrow">now playing</p>
          <h1>{track.title}</h1>
          <p class="artist">{track.artist}</p>
          {#if track.album}
            <p class="album">{track.album}</p>
          {/if}
          {#if queueIndex !== null && queueLength > 0}
            <p class="queue">Track {queueIndex + 1} of {queueLength}</p>
          {/if}
        {:else}
          <p class="eyebrow">now playing</p>
          <h1 class="muted">Nothing playing</h1>
          <p class="artist muted">Select a track and press Enter.</p>
        {/if}
      </div>
    </div>

    <div class="timeline">
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

    <div class="transport">
      <button
        class="big"
        class:active={playback?.shuffle}
        aria-label="Shuffle"
        aria-pressed={playback?.shuffle ?? false}
        onclick={() => store.toggleShuffle()}
      >
        ⤨
      </button>
      <button class="big" aria-label="Previous track" onclick={() => store.previous()}>
        ⏮
      </button>
      <button
        class="big primary"
        aria-label={isPlaying ? 'Pause' : 'Play'}
        onclick={() => store.togglePlayback()}
      >
        <span class="prompt-sign" aria-hidden="true">$</span>
        <span class="play-text">{isPlaying ? 'pause' : 'play'}</span>
        <span class="play-icon" aria-hidden="true">{isPlaying ? '⏸' : '▶'}</span>
      </button>
      <button class="big" aria-label="Next track" onclick={() => store.next()}>
        ⏭
      </button>
      <button
        class="big"
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
    </div>
  </div>
{/if}

<style>
  .full-player {
    position: fixed;
    inset: 0;
    z-index: 30;
    display: flex;
    flex-direction: column;
    gap: clamp(0.8rem, 2vh, 1.6rem);
    padding: clamp(1rem, 3vw, 2.5rem);
    overflow-y: auto;
    background: var(--bg);
    animation: full-player-in 180ms ease;
  }

  @keyframes full-player-in {
    from {
      opacity: 0;
      transform: translateY(12px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .backdrop {
    position: absolute;
    inset: -5%;
    background-position: center;
    background-size: cover;
    filter: blur(55px) saturate(1.2);
    opacity: 0.32;
    pointer-events: none;
  }

  .full-header,
  .stage,
  .timeline,
  .transport,
  .volume {
    position: relative;
    z-index: 1;
  }

  .full-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }

  .prompt {
    font-size: 0.78rem;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--accent-2);
  }

  .close {
    padding: 0.25rem 0.7rem;
    font-size: 1.2rem;
    line-height: 1;
  }

  .stage {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr);
    align-items: center;
    gap: clamp(1.5rem, 4vw, 4rem);
    width: 100%;
    max-width: 1200px;
    margin: 0 auto;
    min-height: 0;
  }

  .art-wrap {
    position: relative;
    justify-self: center;
    width: min(38vh, 100%);
    aspect-ratio: 1;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: linear-gradient(135deg, var(--accent), var(--accent-2));
    box-shadow: 0 0 70px color-mix(in srgb, var(--accent) 30%, transparent);
  }

  .art-fallback {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--bg);
    font-size: clamp(4rem, 12vh, 9rem);
    font-weight: 800;
  }

  .art {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .details {
    min-width: 0;
  }

  .eyebrow {
    margin: 0;
    font-size: 0.72rem;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--accent-2);
  }

  .details h1 {
    margin: 0.25rem 0 0.6rem;
    font-size: clamp(1.7rem, 3.2vw, 3rem);
    line-height: 1.1;
    color: var(--accent);
    text-shadow: 0 0 24px color-mix(in srgb, var(--accent) 40%, transparent);
    overflow-wrap: anywhere;
  }

  .artist {
    margin: 0;
    font-size: clamp(1rem, 2vw, 1.5rem);
    color: var(--fg);
  }

  .album {
    margin: 0.25rem 0 0;
    font-size: clamp(0.8rem, 1.5vw, 1.1rem);
    color: var(--fg-muted);
    font-style: italic;
  }

  .queue {
    margin: 0.9rem 0 0;
    font-size: 0.78rem;
    color: var(--accent-2);
    opacity: 0.85;
  }

  .timeline {
    display: flex;
    align-items: center;
    gap: 0.8rem;
    width: 100%;
    max-width: 1200px;
    margin: 0 auto;
  }

  .timeline input {
    flex: 1;
    height: 8px;
    cursor: pointer;
  }

  .time {
    min-width: 3.6rem;
    text-align: center;
    font-size: 0.82rem;
    font-variant-numeric: tabular-nums;
    color: var(--fg-muted);
  }

  .transport {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: clamp(0.5rem, 2vw, 1.8rem);
    flex-wrap: wrap;
  }

  .big {
    display: grid;
    place-items: center;
    min-width: 4.5rem;
    min-height: 4.5rem;
    padding: 0.9rem 1.3rem;
    border-radius: 8px;
    font-size: 2rem;
    line-height: 1;
  }

  .big.active {
    color: var(--accent);
    border-color: var(--accent);
    box-shadow: 0 0 16px color-mix(in srgb, var(--accent) 25%, transparent);
  }

  .big.primary {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.7rem;
    min-width: min(13rem, 80vw);
    background: var(--accent);
    border-color: var(--accent);
    color: var(--bg);
    font-size: 1.6rem;
    font-weight: 700;
    text-transform: lowercase;
    box-shadow: 0 0 28px color-mix(in srgb, var(--accent) 45%, transparent);
  }

  .big.primary:hover {
    background: var(--accent-2);
    border-color: var(--accent-2);
    color: var(--bg);
    text-shadow: none;
  }

  .prompt-sign {
    opacity: 0.7;
  }

  .play-icon {
    font-size: 1.35rem;
  }

  .volume {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.7rem;
    padding-bottom: 0.2rem;
  }

  .volume input {
    width: min(22rem, 60vw);
    height: 6px;
  }

  .percent {
    min-width: 3rem;
    text-align: right;
    font-size: 0.78rem;
    font-variant-numeric: tabular-nums;
    color: var(--fg-muted);
  }

  @media (max-width: 800px) {
    .full-player {
      gap: 0.7rem;
      padding: 1rem;
    }

    .stage {
      grid-template-columns: 1fr;
      gap: 0.9rem;
      text-align: center;
    }

    .art-wrap {
      width: min(55vw, 38vh);
    }

    .details h1 {
      font-size: clamp(1.6rem, 8vw, 2.6rem);
    }

    .big {
      min-width: 3.4rem;
      min-height: 3.4rem;
      padding: 0.6rem 0.9rem;
      font-size: 1.5rem;
    }

    .big.primary {
      min-width: min(9rem, 70vw);
      font-size: 1.2rem;
    }

    .timeline input {
      height: 6px;
    }
  }
</style>
