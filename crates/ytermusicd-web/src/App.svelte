<script lang="ts">
  import { onMount } from 'svelte';
  import type { AppStore } from './lib/store.svelte';
  import { appStore } from './lib/app-store';
  import { Keymap } from './lib/keymap';
  import AuthBanner from './lib/components/AuthBanner.svelte';
  import ConnectionBanner from './lib/components/ConnectionBanner.svelte';
  import FullPlayer from './lib/components/FullPlayer.svelte';
  import HelpOverlay from './lib/components/HelpOverlay.svelte';
  import LibraryView from './lib/components/LibraryView.svelte';
  import NowPlaying from './lib/components/NowPlaying.svelte';
  import PlaylistPicker from './lib/components/PlaylistPicker.svelte';
  import QueueView from './lib/components/QueueView.svelte';
  import SearchPalette from './lib/components/SearchPalette.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import ToastHost from './lib/components/ToastHost.svelte';

  let { store = appStore }: { store?: AppStore } = $props();

  const title = $derived(
    store.view.kind === 'library'
      ? 'Library'
      : store.view.kind === 'queue'
        ? 'Queue'
        : (store.playlist?.playlist.name ?? store.view.name ?? 'Playlist'),
  );

  const auth = $derived.by(() => {
    const providerAuth = store.activeProviderInfo?.auth ?? store.status?.auth ?? null;
    if (!providerAuth) return null;
    if (store.authExpired) {
      return {
        ...providerAuth,
        state: 'invalid' as const,
        message: `${store.activeProviderName} rejected the request. Refresh headers.txt.`,
      };
    }
    return providerAuth;
  });

  onMount(() => {
    void store.init();

    const keymap = new Keymap({
      move: (delta) => store.moveSelection(delta),
      first: () => store.selectFirst(),
      last: () => store.selectLast(),
      halfPage: (direction) => store.halfPage(direction),
      openSearch: () => store.openSearch(),
      playSelected: () => void store.playSelected(),
      togglePlayback: () => void store.togglePlayback(),
      queueSelected: () => void store.queueSelected(),
      deleteSelected: () => {
        if (store.view.kind === 'queue') void store.removeQueueIndex(store.selection);
      },
      openHelp: () => store.toggleHelp(),
      closeOverlays: () => store.closeOverlays(),
      showLibrary: () => store.openLibrary(),
      showQueue: () => store.openQueue(),
      moveFocus: () => store.toggleFocus(),
      moveQueueItem: (delta) => void store.moveSelectedQueueItem(delta),
      requestClearQueue: () => store.requestClearQueue(),
      createPlaylist: () => store.openCreatePlaylist(),
      renamePlaylist: () => store.openRenamePlaylist(),
      playSelectedPlaylist: () => void store.playSelectedPlaylist(),
      addSelectedToPlaylist: () => store.openPlaylistPicker(),
      retrySelected: () => void store.retrySelected(),
      toggleShuffle: () => void store.toggleShuffle(),
      cycleRepeat: () => void store.cycleRepeat(),
      cycleProvider: () => void store.cycleProvider(),
      adjustVolume: (delta) => void store.adjustVolume(delta),
      seekRelative: (delta) => void store.seekRelative(delta),
      previousTrack: () => void store.previous(),
      nextTrack: () => void store.next(),
    });

    const onKeydown = (event: KeyboardEvent) => {
      keymap.handle(event, {
        overlayOpen:
          store.searchOpen || store.helpOpen || store.fullPlayerOpen || store.pickerOpen,
      });
    };
    window.addEventListener('keydown', onKeydown);

    return () => {
      window.removeEventListener('keydown', onKeydown);
      keymap.dispose();
      store.dispose();
    };
  });
</script>

<div class="app">
  <header class="topbar">
    <div class="view-title">
      <span class="slash" aria-hidden="true">$</span>
      {title}
    </div>
    <label class="provider-select" title="Music provider">
      <span class="visually-hidden">Music provider</span>
      <select
        aria-label="Music provider"
        value={store.activeProvider}
        onchange={(event) => void store.setProvider(event.currentTarget.value as 'youtube')}
      >
        {#each store.providers as provider (provider.id)}
          <option value={provider.id}>{provider.name}</option>
        {/each}
      </select>
    </label>
    <button class="search-button" onclick={() => store.openSearch()}>
      <span aria-hidden="true">⌕</span>
      <span>Search</span>
      <kbd>/</kbd>
    </button>
    <button class="help-button" aria-label="Show help" title="Show help (?)" onclick={() => store.toggleHelp()}>
      ?
    </button>
    <span
      class="connection"
      class:online={store.connection === 'online'}
      title={`Daemon connection: ${store.connection}`}
      aria-label={`Daemon connection: ${store.connection}`}
    >
      ●
    </span>
  </header>

  <ConnectionBanner
    connection={store.connection}
    error={store.error}
    onretry={() => store.retryConnection()}
  />
  <AuthBanner {auth} providerName={store.activeProviderName} />

  {#if store.error && store.connection === 'online'}
    <div class="error-banner" role="alert">
      <span>{store.error}</span>
      <button
        onclick={() => {
          store.error = null;
          void store.refreshStatus();
          void store.loadPlaylists();
        }}
      >
        Retry
      </button>
      <button aria-label="Dismiss error" onclick={() => (store.error = null)}>×</button>
    </div>
  {/if}

  <div class="workspace">
    <Sidebar {store} />
    <main>
      <svelte:boundary
        onerror={(error) => store.notify('error', `Interface error: ${String(error)}`)}
      >
        {#if store.view.kind === 'queue'}
          <QueueView {store} />
        {:else}
          <LibraryView {store} />
        {/if}

        {#snippet failed(error, reset)}
          <div class="empty-state">
            <p>The interface hit an error.</p>
            <p class="muted">{String(error)}</p>
            <button onclick={reset}>Try again</button>
          </div>
        {/snippet}
      </svelte:boundary>
    </main>
  </div>

  <NowPlaying {store} />
  <FullPlayer {store} />
  <SearchPalette {store} />
  <PlaylistPicker {store} />
  <HelpOverlay open={store.helpOpen} onclose={() => (store.helpOpen = false)} />
  <ToastHost {store} />
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .topbar {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.5rem 1rem;
    border-bottom: 2px solid transparent;
    border-image: linear-gradient(to right, var(--accent), transparent) 1;
    background: var(--bg);
    flex: none;
  }

  .view-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.86rem;
    font-weight: 600;
    color: var(--accent);
    text-shadow: 0 0 8px color-mix(in srgb, var(--accent) 45%, transparent);
    letter-spacing: 0.03em;
  }

  .slash {
    color: var(--fg-muted);
    margin-right: 0.35rem;
  }

  .provider-select select {
    background: color-mix(in srgb, var(--bg-raised) 33%, transparent);
    color: var(--fg-muted);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.3rem 0.4rem;
    font-size: 0.78rem;
  }

  .provider-select select:hover {
    color: var(--accent);
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }

  .search-button {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 11rem;
    justify-content: space-between;
    background: color-mix(in srgb, var(--bg-raised) 33%, transparent);
    color: var(--fg-muted);
    font-size: 0.78rem;
  }

  .search-button:hover {
    color: var(--accent);
  }

  kbd {
    border: 1px dashed var(--border);
    border-radius: 4px;
    padding: 0 0.3rem;
    background: color-mix(in srgb, var(--bg-raised) 33%, transparent);
    font-size: 0.68rem;
  }

  .help-button {
    width: 1.9rem;
    height: 1.9rem;
    display: grid;
    place-items: center;
    border-radius: 4px;
    font-size: 0.9rem;
    color: var(--fg-muted);
  }

  .connection {
    color: var(--danger);
    font-size: 0.72rem;
  }

  .connection.online {
    color: var(--success);
  }

  .error-banner {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.4rem 1rem;
    border-bottom: 1px solid var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, var(--bg-raised));
    font-size: 0.8rem;
  }

  .error-banner span {
    flex: 1;
  }

  .error-banner button {
    padding: 0.15rem 0.5rem;
    font-size: 0.75rem;
  }

  .workspace {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  main {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
