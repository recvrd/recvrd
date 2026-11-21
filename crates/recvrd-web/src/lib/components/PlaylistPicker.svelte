<script lang="ts">
  import { tick } from 'svelte';
  import type { AppStore } from '../store.svelte';

  let { store }: { store: AppStore } = $props();

  let dialog: HTMLElement | undefined = $state();

  $effect(() => {
    if (store.pickerOpen) {
      void tick().then(() => dialog?.focus());
    }
  });

  function onKeydown(event: KeyboardEvent): void {
    switch (event.key) {
      case 'ArrowDown':
      case 'j':
        event.preventDefault();
        store.selectPicker(store.pickerSelection + 1);
        scrollActiveIntoView();
        break;
      case 'ArrowUp':
      case 'k':
        event.preventDefault();
        store.selectPicker(store.pickerSelection - 1);
        scrollActiveIntoView();
        break;
      case 'Enter':
        event.preventDefault();
        void store.confirmPicker();
        break;
      case 'Escape':
        event.preventDefault();
        event.stopPropagation();
        store.closePlaylistPicker();
        break;
    }
  }

  function scrollActiveIntoView(): void {
    document
      .getElementById(`picker-entry-${store.pickerSelection}`)
      ?.scrollIntoView?.({ block: 'nearest' });
  }
</script>

{#if store.pickerOpen}
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="backdrop"
    role="presentation"
    onclick={(event) => {
      if (event.target === event.currentTarget) store.closePlaylistPicker();
    }}
  >
    <div
      class="picker"
      role="dialog"
      aria-modal="true"
      aria-label="Add track to a playlist"
      tabindex="-1"
      bind:this={dialog}
      onkeydown={onKeydown}
    >
      <p class="mode muted">
        Add “{store.pickerTrack?.title ?? 'track'}” to a playlist
      </p>
      <div class="results" role="listbox" aria-label="Playlists">
        {#if store.playlists.length === 0}
          <p class="hint muted">No playlists yet.</p>
        {:else}
          {#each store.playlists as playlist, index (playlist.id)}
            <div class="result" class:active={index === store.pickerSelection}>
              <button
                id={`picker-entry-${index}`}
                class="result-main"
                role="option"
                aria-selected={index === store.pickerSelection}
                onmouseenter={() => store.selectPicker(index)}
                onclick={() => {
                  store.selectPicker(index);
                  void store.confirmPicker();
                }}
              >
                <span class="kind" aria-hidden="true">▤</span>
                <span class="text">
                  <span class="title">{playlist.name}</span>
                  <span class="sub muted">{playlist.subtitle}</span>
                </span>
              </button>
            </div>
          {/each}
        {/if}
      </div>
      <footer>
        <span class="muted">↑ ↓ select · Enter add · Esc cancel</span>
        <button onclick={() => store.closePlaylistPicker()}>Cancel</button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 45;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: 8vh 1rem 2rem;
    background: rgb(9 24 51 / 78%);
    backdrop-filter: blur(3px);
  }

  .picker {
    width: min(32rem, 100%);
    max-height: min(30rem, 82vh);
    display: flex;
    flex-direction: column;
    border: 1px dashed var(--border);
    border-radius: 8px;
    background: color-mix(in srgb, var(--bg-raised) 80%, transparent);
    backdrop-filter: blur(6px);
    box-shadow:
      0 24px 80px rgb(3 8 18 / 70%),
      0 0 24px color-mix(in srgb, var(--accent) 18%, transparent);
    overflow: hidden;
  }

  .picker:focus-visible {
    outline: none;
  }

  .mode {
    margin: 0;
    padding: 0.55rem 0.9rem 0;
    font-size: 0.72rem;
  }

  .results {
    overflow-y: auto;
    min-height: 0;
    padding: 0.35rem;
  }

  .result {
    border-radius: 4px;
  }

  .result.active {
    background: var(--selection);
    box-shadow: inset 2px 0 0 var(--accent-2);
  }

  .result-main {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    border: none;
    background: transparent;
    text-align: left;
    padding: 0.5rem 0.6rem;
    min-width: 0;
    width: 100%;
  }

  .kind {
    color: var(--accent);
    width: 1rem;
    text-align: center;
    flex: none;
  }

  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }

  .title {
    color: var(--accent);
    font-size: 0.86rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sub {
    font-size: 0.72rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hint {
    padding: 1rem;
    text-align: center;
    font-size: 0.82rem;
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.55rem 0.9rem;
    border-top: 1px dotted var(--accent);
    font-size: 0.72rem;
  }

  footer button {
    padding: 0.18rem 0.55rem;
    font-size: 0.74rem;
  }
</style>
