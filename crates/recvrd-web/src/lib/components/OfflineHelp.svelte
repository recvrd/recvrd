<script lang="ts">
  import type { AppStore } from '../store.svelte';

  let { store }: { store: AppStore } = $props();
</script>

{#if store.offlineHelpOpen}
  <div
    class="backdrop"
    role="presentation"
    onclick={(event) => {
      if (event.target === event.currentTarget) store.closeOfflineHelp();
    }}
  >
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="offline-help-title">
      <header>
        <h2 id="offline-help-title">Unavailable offline</h2>
        <button aria-label="Close offline help" onclick={() => store.closeOfflineHelp()}>×</button>
      </header>
      <p>
        Offline mode is on. This item has no local cache file, so the daemon will not
        call a music provider to fetch it.
      </p>
      <p class="code"><code>offline_unavailable</code></p>
      <p class="muted">
        Play or download the item while online, or turn offline mode off. Cached items
        remain available while the switch is on.
      </p>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: center;
    padding: 1rem;
    background: rgb(9 24 51 / 78%);
    backdrop-filter: blur(3px);
  }

  .dialog {
    width: min(30rem, 100%);
    border: 1px dashed var(--border);
    border-radius: 8px;
    background: color-mix(in srgb, var(--bg-raised) 92%, transparent);
    padding: 1rem 1.2rem;
    box-shadow: 0 24px 80px rgb(3 8 18 / 70%);
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }

  h2 {
    margin: 0;
    font-size: 1.05rem;
    color: var(--warning);
  }

  header button {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.3rem;
  }

  p {
    margin: 0.8rem 0 0;
    font-size: 0.86rem;
    line-height: 1.5;
  }

  .code code {
    border: 1px dashed var(--border);
    border-radius: 4px;
    padding: 0.1rem 0.35rem;
    color: var(--warning);
  }
</style>
