<script lang="ts">
  import { tick } from 'svelte';
  import type { AppStore } from '../store.svelte';
  import { coverUrl } from '../api';
  import { playlistTitle } from '../playlist-title';
  import TrackTable from './TrackTable.svelte';

  let { store }: { store: AppStore } = $props();

  let createInput: HTMLInputElement | undefined = $state();
  let renameInput: HTMLInputElement | undefined = $state();
  let confirmDelete = $state(false);
  let removal = $state<{ index: number; title: string } | null>(null);
  let addingFeed = $state(false);
  let feedUrl = $state('');
  let removingFeed = $state<string | null>(null);

  const playlistView = $derived(store.view.kind === 'playlist' ? store.view : null);
  const tracks = $derived(playlistView ? (store.playlist?.tracks ?? []) : []);
  const name = $derived(
    playlistView
      ? playlistTitle(
          playlistView.id,
          store.playlist?.playlist.name,
          playlistView.name,
        )
      : 'Playlist',
  );
  const currentTrack = $derived(store.currentTrack);
  const isPlaying = $derived(store.status?.playback.state === 'playing');

  $effect(() => {
    if (store.createPlaylistOpen) {
      void tick().then(() => createInput?.focus());
    }
  });

  $effect(() => {
    if (store.renamePlaylistOpen) {
      void tick().then(() => renameInput?.focus());
    }
  });

  async function create(): Promise<void> {
    const value = store.createPlaylistName.trim();
    if (!value) return;
    if (await store.createPlaylist(value)) {
      store.closeCreatePlaylist();
    }
  }

  async function rename(): Promise<void> {
    if (!playlistView || !store.renamePlaylistId) return;
    const value = store.renamePlaylistName.trim();
    if (!value) return;
    if (await store.renamePlaylist(store.renamePlaylistId, value)) {
      store.closeRenamePlaylist();
    }
  }

  async function removePlaylist(): Promise<void> {
    if (!playlistView) return;
    if (await store.deletePlaylist(playlistView.id)) {
      confirmDelete = false;
    }
  }

  function requestRemove(index: number): void {
    const track = tracks[index];
    if (track) removal = { index, title: track.title };
  }

  async function confirmRemoval(): Promise<void> {
    if (!playlistView || !removal) return;
    if (await store.removePlaylistTrack(playlistView.id, removal.index)) {
      removal = null;
    }
  }

  function move(index: number, delta: number): void {
    if (!playlistView) return;
    const target = index + delta;
    if (target < 0 || target >= tracks.length) return;
    void store.movePlaylistTrack(playlistView.id, index, target);
  }

  async function addFeed(): Promise<void> {
    const url = feedUrl.trim();
    if (!url) return;
    if (await store.addRssFeed(url)) {
      feedUrl = '';
      addingFeed = false;
    }
  }

  async function removeFeed(id: string): Promise<void> {
    if (await store.removeRssFeed(id)) {
      removingFeed = null;
    }
  }
</script>

{#if store.view.kind === 'library'}
  <section class="library" aria-label="Library">
    <header class="hero">
      <div>
        <p class="eyebrow">{store.activeProviderName}</p>
        <h1>Library</h1>
        <p class="muted">
          {store.filteredPlaylists.length} playlists
          {#if store.filter}
            matching “{store.filter}”
          {/if}
        </p>
      </div>
      <div class="hero-actions">
        {#if store.isRssProvider}
          {#if addingFeed}
            <form
              class="inline-form"
              onsubmit={(event) => {
                event.preventDefault();
                void addFeed();
              }}
            >
              <input
                bind:value={feedUrl}
                placeholder="Feed URL"
                aria-label="Feed URL"
                inputmode="url"
              />
              <button type="submit" class="accent" disabled={!feedUrl.trim()}>Add</button>
              <button
                type="button"
                onclick={() => {
                  addingFeed = false;
                  feedUrl = '';
                }}
              >
                Cancel
              </button>
            </form>
          {:else}
            <button class="accent" onclick={() => (addingFeed = true)}>Add feed</button>
          {/if}
          <button class="refresh" onclick={() => store.refreshRssFeeds()}>Refresh feeds</button>
        {/if}
        {#if store.createPlaylistOpen}
          <form
            class="inline-form"
            onsubmit={(event) => {
              event.preventDefault();
              void create();
            }}
          >
            <input
              bind:this={createInput}
              value={store.createPlaylistName}
              oninput={(event) => (store.createPlaylistName = event.currentTarget.value)}
              placeholder="Playlist name"
              aria-label="New playlist name"
              maxlength="150"
            />
            <button type="submit" class="accent" disabled={!store.createPlaylistName.trim()}>
              Create
            </button>
            <button type="button" onclick={() => store.closeCreatePlaylist()}>Cancel</button>
          </form>
        {:else if store.canCreatePlaylist}
          <button class="accent" onclick={() => store.openCreatePlaylist()}>New playlist</button>
        {/if}
        <button class="refresh" onclick={() => store.loadPlaylists()}>Reload</button>
      </div>
    </header>

    {#if currentTrack}
      <section class="now-playing-panel" aria-label="Now playing">
        <div class="np-art" aria-hidden="true">
          <span>{currentTrack.title.slice(0, 1).toUpperCase()}</span>
          <img
            src={coverUrl(currentTrack)}
            alt=""
            loading="lazy"
            onload={(event) => {
              (event.currentTarget as HTMLImageElement).style.display = '';
            }}
            onerror={(event) => {
              (event.currentTarget as HTMLImageElement).style.display = 'none';
            }}
          />
        </div>
        <div class="np-info">
          <p class="eyebrow">Now playing</p>
          <h2>{currentTrack.title}</h2>
          <p class="muted">
            {currentTrack.artist}
            {#if currentTrack.album}<span> · {currentTrack.album}</span>{/if}
          </p>
        </div>
        <div class="np-actions">
          <button class="accent" onclick={() => store.togglePlayback()}>
            {isPlaying ? 'Pause' : 'Play'}
          </button>
          <button onclick={() => store.openFullPlayer()}>Full player</button>
        </div>
      </section>
    {/if}

    {#if store.playlistsLoading && store.playlists.length === 0}
      <div class="cards">
        {#each [1, 2, 3, 4, 5, 6] as item (item)}
          <div class="skeleton card-skeleton"></div>
        {/each}
      </div>
    {:else if store.filteredPlaylists.length === 0}
      <div class="empty-state">
        {#if store.isRssProvider}
          <p>No feeds yet. Add a feed URL above to see its episodes.</p>
        {:else}
          <p>No playlists found.</p>
        {/if}
        {#if store.filter}
          <button onclick={() => store.setFilter('')}>Clear filter</button>
        {/if}
      </div>
    {:else}
      <div class="cards">
        {#each store.filteredPlaylists as playlist (playlist.id)}
          <article class="card">
            <div class="art" aria-hidden="true">
              <span>{playlist.name.slice(0, 1).toUpperCase()}</span>
            </div>
            <div class="info">
              <h2 title={playlist.name}>{playlist.name}</h2>
              <p class="muted" title={playlist.subtitle}>{playlist.subtitle}</p>
            </div>
            <div class="actions">
              <button onclick={() => store.openPlaylist(playlist.id, playlist.name)}>
                Open
              </button>
              <button class="accent" onclick={() => store.playPlaylist(playlist.id)}>
                Play
              </button>
              {#if store.isRssProvider}
                <button onclick={() => store.refreshRssFeed(playlist.id)}>Refresh</button>
                {#if removingFeed === playlist.id}
                  <button class="danger" onclick={() => removeFeed(playlist.id)}>
                    Remove?
                  </button>
                  <button onclick={() => (removingFeed = null)}>Cancel</button>
                {:else}
                  <button class="danger" onclick={() => (removingFeed = playlist.id)}>
                    Remove
                  </button>
                {/if}
              {/if}
            </div>
          </article>
        {/each}
      </div>
    {/if}
  </section>
{:else if playlistView}
  <section class="playlist-view" aria-label={`Playlist ${name}`}>
    <header class="playlist-header">
      <div class="art large" aria-hidden="true">{name.slice(0, 1).toUpperCase()}</div>
      <div class="heading">
        <p class="eyebrow">Playlist</p>
        {#if store.renamePlaylistOpen && store.renamePlaylistId === playlistView.id}
          <form
            class="inline-form"
            onsubmit={(event) => {
              event.preventDefault();
              void rename();
            }}
          >
            <input
              bind:this={renameInput}
              value={store.renamePlaylistName}
              oninput={(event) => (store.renamePlaylistName = event.currentTarget.value)}
              aria-label="Playlist name"
              maxlength="150"
            />
            <button type="submit" class="accent" disabled={!store.renamePlaylistName.trim()}>
              Save
            </button>
            <button type="button" onclick={() => store.closeRenamePlaylist()}>Cancel</button>
          </form>
        {:else}
          <h1>{name}</h1>
        {/if}
        <p class="muted">
          {store.playlist?.playlist.subtitle ?? ''}
          {#if store.playlist}
            · {store.playlist.tracks.length} tracks
          {/if}
        </p>
      </div>
      <div class="header-actions">
        {#if confirmDelete}
          <span class="confirm-text">Delete “{name}”?</span>
          <button class="danger" onclick={removePlaylist}>Delete</button>
          <button onclick={() => (confirmDelete = false)}>Cancel</button>
        {:else if store.playlist}
          <button
            class="accent"
            disabled={store.playlist.tracks.length === 0}
            onclick={() => store.playPlaylist(playlistView.id, playlistView.provider)}
          >
            Play all
          </button>
          <button disabled={!store.selectedTrack} onclick={() => store.queueSelected()}>
            Queue selected
          </button>
          {#if store.canEditPlaylist}
            <button onclick={() => store.openSearchForPlaylist(playlistView.id)}>Add tracks</button>
            <button onclick={() => store.openRenamePlaylist()}>Rename</button>
            <button class="danger" onclick={() => (confirmDelete = true)}>Delete</button>
          {/if}
        {/if}
      </div>
    </header>

    {#if removal}
      <div class="confirm-bar" role="alert">
        <span>Remove “{removal.title}” from this playlist?</span>
        <button class="danger" onclick={confirmRemoval}>Remove</button>
        <button onclick={() => (removal = null)}>Cancel</button>
      </div>
    {/if}

    <div class="table-wrap">
      {#if store.playlistLoading}
        <div class="loading">
          {#each [1, 2, 3, 4, 5, 6, 7, 8] as item (item)}
            <div class="skeleton row-skeleton"></div>
          {/each}
        </div>
      {:else if !store.playlist}
        <div class="empty-state">
          <p>This playlist did not load.</p>
          {#if store.error}<p class="muted">{store.error}</p>{/if}
          <button
            onclick={() =>
              store.openPlaylist(
                playlistView.id,
                playlistView.name,
                playlistView.provider,
              )}
          >
            Try again
          </button>
        </div>
      {:else}
        <TrackTable
          {tracks}
          selection={store.selection}
          currentTrack={store.currentTrack}
          downloadStatus={(track) => store.downloadStatusFor(track)}
          onselect={(index) => store.select(index)}
          onplay={(track) => store.playTrack(track)}
          onqueue={(track) => store.queueTrack(track)}
          onqueuenext={(track) => store.queueNextTrack(track)}
          ongeekstats={(track) => store.openGeekStats(track)}
          onretry={(track) => store.retryDownload(track)}
          onlike={store.canLikeTracks
            ? (track) => store.setTrackLike(track, track.liked !== true)
            : undefined}
          onremove={store.canEditPlaylist ? requestRemove : undefined}
          onmoveup={store.canEditPlaylist ? (index) => move(index, -1) : undefined}
          onmovedown={store.canEditPlaylist ? (index) => move(index, 1) : undefined}
          offline={store.offline}
          onunavailable={() => store.openOfflineHelp()}
          emptyMessage="This playlist has no tracks."
        />
      {/if}
    </div>
  </section>
{/if}

<style>
  .library,
  .playlist-view {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }

  .hero,
  .playlist-header {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 1.1rem 1.25rem;
    border-bottom: 1px dotted var(--accent);
    flex: none;
  }

  .hero {
    justify-content: space-between;
    background: color-mix(in srgb, var(--bg-raised) 33%, transparent);
    backdrop-filter: blur(4px);
  }

  .eyebrow {
    margin: 0 0 0.15rem;
    font-size: 0.68rem;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--accent);
  }

  h1 {
    margin: 0;
    font-size: 1.5rem;
    color: var(--accent);
    text-shadow: 0 0 10px color-mix(in srgb, var(--accent) 45%, transparent);
  }

  .hero .muted,
  .playlist-header .muted {
    margin: 0.2rem 0 0;
    font-size: 0.78rem;
  }

  .refresh {
    align-self: center;
    font-size: 0.78rem;
  }

  .hero-actions {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .inline-form {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    flex-wrap: wrap;
  }

  .inline-form input {
    min-width: 12rem;
  }

  .inline-form button {
    padding: 0.18rem 0.55rem;
    font-size: 0.74rem;
  }

  .now-playing-panel {
    display: flex;
    align-items: center;
    gap: 0.9rem;
    flex: none;
    margin: 1rem 1.25rem 0;
    padding: 0.8rem;
    border: 1px dashed var(--accent);
    border-radius: 8px;
    background:
      linear-gradient(
        90deg,
        color-mix(in srgb, var(--accent) 10%, transparent),
        color-mix(in srgb, var(--bg-raised) 45%, transparent)
      );
  }

  .np-art {
    position: relative;
    width: 5rem;
    height: 5rem;
    flex: none;
    overflow: hidden;
    border-radius: 6px;
    background: linear-gradient(135deg, var(--accent), var(--accent-2));
  }

  .np-art span {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--bg);
    font-size: 1.8rem;
    font-weight: 800;
  }

  .np-art img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .np-info {
    flex: 1;
    min-width: 0;
  }

  .np-info .eyebrow {
    margin: 0 0 0.1rem;
  }

  .np-info h2 {
    margin: 0;
    overflow: hidden;
    color: var(--accent);
    font-size: 1.05rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .np-info p {
    margin: 0.15rem 0 0;
    overflow: hidden;
    font-size: 0.78rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .np-actions {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .np-actions button {
    padding: 0.2rem 0.6rem;
    font-size: 0.76rem;
  }

  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(17rem, 1fr));
    gap: 0.8rem;
    padding: 1rem 1.25rem;
    overflow-y: auto;
    min-height: 0;
  }

  .card {
    display: grid;
    grid-template-columns: 3.2rem minmax(0, 1fr);
    grid-template-rows: auto auto;
    gap: 0.35rem 0.7rem;
    align-items: center;
    padding: 0.75rem;
    border: 1px dashed var(--border);
    border-radius: 8px;
    background: color-mix(in srgb, var(--bg-raised) 33%, transparent);
    backdrop-filter: blur(4px);
  }

  .card:hover {
    border-color: var(--accent);
    box-shadow: 0 0 18px color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .art {
    display: grid;
    place-items: center;
    width: 3.2rem;
    height: 3.2rem;
    border-radius: 6px;
    background: linear-gradient(135deg, var(--accent), var(--accent-2));
    color: var(--bg);
    font-size: 1.4rem;
    font-weight: 800;
    grid-row: span 2;
  }

  .art.large {
    width: 4.6rem;
    height: 4.6rem;
    font-size: 2rem;
    flex: none;
  }

  .info {
    min-width: 0;
  }

  .info h2 {
    margin: 0;
    font-size: 0.88rem;
    color: var(--accent);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .info p {
    margin: 0.1rem 0 0;
    font-size: 0.7rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    gap: 0.35rem;
    flex-wrap: wrap;
    grid-column: 2;
  }

  .actions button,
  .header-actions button {
    padding: 0.18rem 0.55rem;
    font-size: 0.74rem;
  }

  .accent {
    border-color: var(--accent);
    color: var(--accent);
  }

  .heading {
    flex: 1;
    min-width: 0;
  }

  .heading h1 {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex: none;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .confirm-text {
    font-size: 0.75rem;
    color: var(--danger);
  }

  .confirm-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.45rem 1.25rem;
    border-bottom: 1px solid var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, var(--bg-raised));
    font-size: 0.8rem;
  }

  .confirm-bar span {
    flex: 1;
  }

  .confirm-bar button {
    padding: 0.15rem 0.5rem;
    font-size: 0.75rem;
  }

  .table-wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .loading {
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .row-skeleton {
    height: 2.1rem;
  }

  .card-skeleton {
    height: 5rem;
  }

  @media (max-width: 700px) {
    .playlist-header {
      flex-wrap: wrap;
    }

    .now-playing-panel {
      flex-wrap: wrap;
    }

    .np-info {
      flex-basis: calc(100% - 6rem);
    }

    .np-actions {
      margin-left: auto;
    }
  }
</style>
