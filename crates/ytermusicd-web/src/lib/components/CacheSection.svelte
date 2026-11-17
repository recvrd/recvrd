<script lang="ts">
  import type { CacheScope } from '@api/CacheScope';
  import { formatBytes } from '../api';
  import type { AppStore } from '../store.svelte';

  let { store }: { store: AppStore } = $props();

  let scope = $state<CacheScope>('all');
  let armed = $state(false);

  async function clean(): Promise<void> {
    if (!armed) {
      armed = true;
      return;
    }
    armed = false;
    await store.cleanCache(scope);
  }
</script>

<div class="cache" aria-label="Cache">
  <span>Cache</span>
  <span class="muted">
    {#if store.cache}
      Downloads {formatBytes(store.cache.downloads_bytes)} ·
      Covers {formatBytes(store.cache.covers_bytes)}
    {:else if store.cacheLoading}
      Loading…
    {:else}
      Sizes unavailable
    {/if}
  </span>
  <div class="controls">
    <select
      aria-label="Cache scope"
      bind:value={scope}
      disabled={store.cacheLoading}
    >
      <option value="all">All</option>
      <option value="downloads">Downloads</option>
      <option value="covers">Covers</option>
    </select>
    <button
      class="clean"
      class:armed
      disabled={store.cacheLoading}
      onclick={() => void clean()}
    >
      {armed ? 'Confirm clean?' : 'Clean cache'}
    </button>
    {#if armed}
      <button
        class="cancel"
        onclick={() => {
          armed = false;
        }}
      >
        Cancel
      </button>
    {/if}
  </div>
</div>

<style>
  .cache {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.55rem 0.8rem;
    border-top: 1px dashed var(--border);
    font-size: 0.68rem;
    color: var(--fg);
  }

  .cache .muted {
    font-size: 0.68rem;
  }

  .controls {
    display: flex;
    gap: 0.35rem;
    align-items: center;
  }

  select {
    font-size: 0.72rem;
    padding: 0.2rem 0.3rem;
    min-width: 0;
  }

  button {
    font-size: 0.72rem;
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    color: var(--accent);
  }

  button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .clean.armed {
    border-color: var(--danger);
    color: var(--danger);
  }

  .cancel {
    border: none;
    color: var(--fg-muted);
  }
</style>
