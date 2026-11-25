<script lang="ts">
  import type { AppStore } from '../store.svelte';
  import { durationSeconds, formatDuration, formatPlayedAt } from '../api';

  let { store }: { store: AppStore } = $props();
</script>

<section class="history" aria-label="Recently played">
  <header>
    <div>
      <h2>Recently played</h2>
      <span class="muted">
        {store.history?.length ?? 0} plays · plays shorter than 30 seconds are not recorded
      </span>
    </div>
    <button
      aria-label="Reload recently played"
      title="Reload recently played"
      onclick={() => void store.loadHistory()}
    >
      ↻
    </button>
  </header>

  {#if store.historyLoading && store.history === null}
    <div class="rows" aria-label="Loading recently played">
      {#each [1, 2, 3] as item (item)}
        <div class="skeleton row-skeleton" style:width={`${80 + item * 4}%`}></div>
      {/each}
    </div>
  {:else if !store.history || store.history.length === 0}
    <div class="empty-state">
      <p>No plays recorded yet.</p>
      <p class="muted">Play a track for 30 seconds or more to record it.</p>
    </div>
  {:else}
    <div class="rows" role="list" aria-label="Recently played tracks">
      {#each store.history as entry (`${entry.provider}:${entry.track_id}:${entry.started_at}`)}
        <div class="row" role="listitem">
          <span class="track">
            <span class="title" title={entry.title}>{entry.title}</span>
            <span class="artist" title={entry.artist}>{entry.artist}</span>
            {#if entry.album}
              <span class="extra" title={entry.album}>{entry.album}</span>
            {/if}
          </span>
          <span class="played" title={new Date(entry.started_at * 1000).toLocaleString()}>
            {formatPlayedAt(entry.started_at)}
          </span>
          <span class="listened">
            {formatDuration(entry.played_secs)} of {formatDuration(
              durationSeconds(entry.duration),
            )}
            {#if !entry.completed}
              <span class="muted">· left early</span>
            {/if}
          </span>
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .history {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    padding: 1rem 1.25rem 0.8rem;
    border-bottom: 1px dotted var(--accent);
  }

  h2 {
    margin: 0 0 0.2rem;
    font-size: 1.15rem;
    color: var(--accent);
    text-shadow: 0 0 8px color-mix(in srgb, var(--accent) 40%, transparent);
  }

  header .muted {
    font-size: 0.78rem;
  }

  .rows {
    overflow-y: auto;
    min-height: 0;
  }

  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 7rem 11rem;
    align-items: center;
    gap: 0.6rem;
    padding: 0.5rem 1rem;
    border-bottom: 1px dotted color-mix(in srgb, var(--border) 35%, transparent);
    font-size: 0.84rem;
  }

  .row:hover {
    background: color-mix(in srgb, var(--selection) 55%, transparent);
  }

  .track {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .title {
    color: var(--fg-bright);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .artist {
    color: var(--fg-muted);
    font-size: 0.75rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .extra {
    color: var(--accent-2);
    font-size: 0.68rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    opacity: 0.85;
  }

  .played {
    color: var(--fg-muted);
    font-size: 0.78rem;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .listened {
    color: var(--fg-muted);
    font-size: 0.78rem;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .row-skeleton {
    height: 2.4rem;
    margin: 0.5rem 1rem;
  }

  @media (max-width: 760px) {
    .row {
      grid-template-columns: minmax(0, 1fr) 6rem;
    }

    .listened {
      display: none;
    }
  }
</style>
