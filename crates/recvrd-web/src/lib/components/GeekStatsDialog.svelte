<script lang="ts">
  import type { AppStore } from '../store.svelte';
  import { formatBytes, formatDuration } from '../api';

  let { store }: { store: AppStore } = $props();

  const stats = $derived(store.geekStats);
  const track = $derived(stats?.track ?? store.geekStatsTrack);

  function stamp(seconds: number | null | undefined): string {
    if (seconds == null || seconds === 0) return '—';
    return new Date(seconds * 1000).toLocaleString();
  }

  async function copy(value: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      store.notify('success', 'Copied.');
    } catch {
      // Clipboard access can be denied.
    }
  }
</script>

{#if store.geekStatsOpen}
  <div class="overlay">
    <div class="dialog" role="dialog" aria-modal="true" aria-label="Geek stats">
      <header>
        <div>
          <h2>Geek stats</h2>
          {#if track}
            <p class="kind">{track.provider} · {track.track_id}</p>
          {/if}
        </div>
        <button class="close" aria-label="Close geek stats" onclick={() => store.closeGeekStats()}>
          ×
        </button>
      </header>

      {#if store.geekStatsLoading}
        <p class="muted">Loading…</p>
      {:else if stats}
        {#if track}
          <section>
            <h3>Track</h3>
            <dl>
              <dt>Title</dt><dd>{track.title}</dd>
              <dt>Artist</dt><dd>{track.artist || '—'}</dd>
              <dt>Album</dt><dd>{track.album || '—'}</dd>
              <dt>Duration</dt><dd>{track.duration || '—'}</dd>
              <dt>Key</dt>
              <dd>
                <code>{track.provider}:{track.track_id}</code>
                <button class="copy" onclick={() => copy(`${track.provider}:${track.track_id}`)}>
                  copy
                </button>
              </dd>
            </dl>
          </section>
        {/if}

        {#if stats.source}
          <section>
            <h3>Source</h3>
            <dl>
              <dt>URL</dt>
              <dd>
                <code>{stats.source.url}</code>
                <button class="copy" onclick={() => copy(stats.source!.url)}>copy</button>
              </dd>
              {#if stats.source.mime}<dt>MIME</dt><dd>{stats.source.mime}</dd>{/if}
              {#if stats.source.length != null}
                <dt>Length</dt><dd>{formatBytes(stats.source.length)}</dd>
              {/if}
              <dt>Seekable</dt><dd>{stats.source.seekable ? 'yes' : 'no'}</dd>
              <dt>Kind</dt><dd>{stats.source.kind}</dd>
            </dl>
          </section>
        {/if}

        {#if stats.file}
          <section>
            <h3>File</h3>
            <dl>
              <dt>Path</dt>
              <dd>
                <code>{stats.file.path}</code>
                <button class="copy" onclick={() => copy(stats.file!.path)}>copy</button>
              </dd>
              {#if stats.file.sidecar_path}
                <dt>Sidecar</dt><dd><code>{stats.file.sidecar_path}</code></dd>
              {/if}
              <dt>Size</dt><dd>{formatBytes(stats.file.size_bytes)}</dd>
              {#if stats.file.modified_unix != null}
                <dt>Modified</dt><dd>{stamp(stats.file.modified_unix)}</dd>
              {/if}
              <dt>Cached</dt><dd>{stats.file.cached ? 'yes' : 'no'}</dd>
            </dl>
          </section>
        {/if}

        {#if stats.audio}
          <section>
            <h3>Audio</h3>
            <dl>
              {#if stats.audio.container}<dt>Container</dt><dd>{stats.audio.container}</dd>{/if}
              {#if stats.audio.codec}<dt>Codec</dt><dd>{stats.audio.codec}</dd>{/if}
              {#if stats.audio.bitrate_kbps != null}
                <dt>Bitrate</dt><dd>{stats.audio.bitrate_kbps} kbps</dd>
              {/if}
              {#if stats.audio.sample_rate != null}
                <dt>Sample rate</dt><dd>{stats.audio.sample_rate} Hz</dd>
              {/if}
              {#if stats.audio.channels != null}
                <dt>Channels</dt>
                <dd>{stats.audio.channels} {stats.audio.channels === 1 ? 'channel' : 'channels'}</dd>
              {/if}
              {#if stats.audio.bits_per_sample != null}
                <dt>Bits per sample</dt><dd>{stats.audio.bits_per_sample}</dd>
              {/if}
              {#if stats.audio.duration_secs != null}
                <dt>Decoded length</dt><dd>{formatDuration(stats.audio.duration_secs)}</dd>
              {/if}
            </dl>
          </section>
        {/if}

        <section>
          <h3>Playback</h3>
          <dl>
            <dt>Mode</dt><dd>{stats.playback.mode}</dd>
            <dt>State</dt><dd>{stats.playback.state}</dd>
            {#if stats.playback.source_kind}
              <dt>Source kind</dt><dd>{stats.playback.source_kind}</dd>
            {/if}
            <dt>Position</dt><dd>{formatDuration(stats.playback.position_secs)}</dd>
            {#if stats.playback.saved_position_secs != null}
              <dt>Saved position</dt><dd>{formatDuration(stats.playback.saved_position_secs)}</dd>
            {/if}
            {#if stats.playback.queue_index != null}
              <dt>Queue index</dt><dd>{stats.playback.queue_index + 1}</dd>
            {/if}
          </dl>
        </section>

        <section>
          <h3>History</h3>
          <dl>
            <dt>Play count</dt><dd>{stats.history.play_count}</dd>
            {#if stats.history.last_played_at != null}
              <dt>Last played</dt><dd>{stamp(stats.history.last_played_at)}</dd>
            {/if}
            {#if stats.history.last_completed != null}
              <dt>Last completed</dt><dd>{stats.history.last_completed ? 'yes' : 'no'}</dd>
            {/if}
            {#if stats.history.last_played_secs != null}
              <dt>Last played time</dt><dd>{formatDuration(stats.history.last_played_secs)}</dd>
            {/if}
          </dl>
        </section>
      {:else}
        <p class="muted">No stats.</p>
      {/if}
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: grid;
    place-items: center;
    padding: 1rem;
    background: rgb(0 0 0 / 0.65);
  }

  .dialog {
    width: min(42rem, 100%);
    max-height: 85vh;
    overflow-y: auto;
    padding: 1rem 1.2rem 1.4rem;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: 0 1rem 3rem rgb(0 0 0 / 0.5);
  }

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
  }

  h2 {
    margin: 0;
    font-size: 1rem;
  }

  h3 {
    margin: 1.2rem 0 0.4rem;
    color: var(--fg-muted);
    font-size: 0.72rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .kind {
    margin: 0.2rem 0 0;
    color: var(--fg-muted);
    font-size: 0.78rem;
  }

  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.25rem 1rem;
    margin: 0;
    font-size: 0.84rem;
  }

  dt {
    color: var(--fg-muted);
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  code {
    font-size: 0.8rem;
  }

  .copy {
    margin-left: 0.4rem;
    padding: 0 0.35rem;
    border-color: var(--border);
    color: var(--fg-muted);
    font-size: 0.68rem;
  }

  .copy:hover {
    border-color: var(--accent-2);
    color: var(--accent-2);
  }

  .close {
    padding: 0 0.4rem;
    border: 0;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.4rem;
    line-height: 1;
  }

  .close:hover {
    color: var(--fg);
  }

  .muted {
    color: var(--fg-muted);
  }
</style>
