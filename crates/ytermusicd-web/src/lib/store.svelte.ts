import type { CacheScope } from '@api/CacheScope';
import type { CacheSizes } from '@api/CacheSizes';
import type { DownloadStatus } from '@api/DownloadStatus';
import type { EventKind } from '@api/EventKind';
import type { Playlist } from '@api/Playlist';
import type { PlaylistDetail } from '@api/PlaylistDetail';
import type { ProviderId } from '@api/ProviderId';
import type { ProviderInfo } from '@api/ProviderInfo';
import type { Queue } from '@api/Queue';
import type { RepeatMode } from '@api/RepeatMode';
import type { RssFeed } from '@api/RssFeed';
import type { SearchResults } from '@api/SearchResults';
import type { Status } from '@api/Status';
import type { Track } from '@api/Track';
import { ApiError, createApi, describeError, formatBytes, type YtermusicdApi } from './api';
import { EventStream, type ConnectionState, type EventSourceLike } from './sse';

export type View =
  | { kind: 'library' }
  | { kind: 'playlist'; id: string; name?: string }
  | { kind: 'queue' };

export type ToastSeverity = 'info' | 'success' | 'warning' | 'error';

export type PlayerLayout = 'compact' | 'terminal';

export type SearchMode = 'normal' | 'add-to-playlist';

export const PLAYER_LAYOUT_KEY = 'ytermusicd.player-layout';
export const PROVIDER_KEY = 'ytermusicd.provider';

export function readPlayerLayout(): PlayerLayout {
  try {
    return globalThis.localStorage?.getItem(PLAYER_LAYOUT_KEY) === 'terminal'
      ? 'terminal'
      : 'compact';
  } catch {
    return 'compact';
  }
}

/// Read the stored provider. An unknown or missing value means the
/// default provider.
export function readProvider(providers: ProviderId[] = ['youtube']): ProviderId {
  try {
    const stored = globalThis.localStorage?.getItem(PROVIDER_KEY);
    if (stored && (providers as string[]).includes(stored)) return stored as ProviderId;
  } catch {
    // Ignore storage failures.
  }
  return 'youtube';
}

export interface Toast {
  id: number;
  severity: ToastSeverity;
  message: string;
  action?: { label: string; run: () => void };
}

export interface StoreOptions {
  createEventSource?: (url: string) => EventSourceLike;
  toastTtlMs?: number;
  searchDebounceMs?: number;
  setTimeoutFn?: typeof setTimeout;
  clearTimeoutFn?: typeof clearTimeout;
}

const DEFAULT_TOAST_TTL = 6000;
const DEFAULT_SEARCH_DEBOUNCE = 250;

export function reorderQueue(queue: Queue, from: number, to: number): Queue {
  const items = [...queue.items];
  if (from === to || from < 0 || to < 0 || from >= items.length || to >= items.length) {
    return queue;
  }
  const [moved] = items.splice(from, 1);
  items.splice(to, 0, moved);
  let current = queue.current;
  if (current !== null) {
    if (current === from) current = to;
    else if (from < current && current <= to) current -= 1;
    else if (to <= current && current < from) current += 1;
  }
  return { ...queue, items, current };
}

export function removeQueueItem(queue: Queue, index: number): Queue {
  const items = [...queue.items];
  if (index < 0 || index >= items.length) return queue;
  items.splice(index, 1);
  let current = queue.current;
  if (current !== null) {
    if (current === index) {
      current = items.length === 0 ? null : Math.min(index, items.length - 1);
    } else if (current > index) {
      current -= 1;
    }
  }
  return { ...queue, items, current };
}

export function looksLikeAuthError(message: string): boolean {
  const text = message.toLowerCase();
  return (
    text.includes('401') ||
    text.includes('403') ||
    text.includes('cookie') ||
    text.includes('sign in') ||
    text.includes('auth')
  );
}

export class AppStore {
  readonly api: YtermusicdApi;

  status = $state<Status | null>(null);
  queue = $state<Queue | null>(null);
  providers = $state<ProviderInfo[]>([]);
  activeProvider = $state<ProviderId>('youtube');
  cache = $state<CacheSizes | null>(null);
  cacheLoading = $state(false);
  playlists = $state<Playlist[]>([]);
  rssFeeds = $state<RssFeed[]>([]);
  playlist = $state<PlaylistDetail | null>(null);
  playlistsLoading = $state(false);
  playlistLoading = $state(false);
  error = $state<string | null>(null);
  authExpired = $state(false);
  connection = $state<ConnectionState>('connecting');
  view = $state<View>({ kind: 'library' });
  selection = $state(0);
  filter = $state('');
  searchOpen = $state(false);
  searchQuery = $state('');
  searchResults = $state<SearchResults | null>(null);
  searchLoading = $state(false);
  searchMode = $state<SearchMode>('normal');
  searchPlaylistId = $state<string | null>(null);
  helpOpen = $state(false);
  toasts = $state<Toast[]>([]);
  volume = $state(50);
  playerLayout = $state<PlayerLayout>('compact');
  fullPlayerOpen = $state(false);

  private readonly events: EventStream;
  private readonly options: Required<Pick<StoreOptions, 'toastTtlMs' | 'searchDebounceMs'>> &
    StoreOptions;
  private readonly liveToasts = new Set<ReturnType<typeof setTimeout>>();
  private toastSeq = 0;
  private searchTimer: ReturnType<typeof setTimeout> | null = null;
  private statusRefreshTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(api: YtermusicdApi = createApi(), options: StoreOptions = {}) {
    this.api = api;
    this.options = {
      toastTtlMs: DEFAULT_TOAST_TTL,
      searchDebounceMs: DEFAULT_SEARCH_DEBOUNCE,
      ...options,
    };
    this.playerLayout = readPlayerLayout();
    this.activeProvider = readProvider();
    this.events = new EventStream({
      create: options.createEventSource,
      onEvent: (event) => this.handleEvent(event),
      onState: (state) => {
        this.connection = state;
        if (state === 'online' && this.error === 'Daemon is unreachable. Is ytermusicd running?') {
          this.error = null;
        }
      },
      onParseError: () => this.notify('warning', 'Received a malformed event from the daemon.'),
    });
  }

  async init(): Promise<void> {
    this.parseLocation();
    this.events.connect();
    await Promise.allSettled([
      this.loadProviders(),
      this.refreshStatus(),
      this.loadPlaylists(),
      this.loadCache(),
    ]);
    if (this.view.kind === 'playlist') {
      await this.openPlaylist(this.view.id, this.view.name);
    }
  }

  dispose(): void {
    this.events.close();
    this.clearSearchTimer();
    this.clearStatusRefreshTimer();
    for (const id of this.liveToasts) {
      const clear = this.options.clearTimeoutFn ?? clearTimeout;
      clear(id);
    }
    this.liveToasts.clear();
  }

  get currentTrack(): Track | null {
    return this.status?.playback.current ?? null;
  }

  get activeProviderInfo(): ProviderInfo | null {
    return this.providers.find((provider) => provider.id === this.activeProvider) ?? null;
  }

  get activeProviderName(): string {
    return this.activeProviderInfo?.name ?? 'YouTube Music';
  }

  get canCreatePlaylist(): boolean {
    return this.activeProviderInfo?.capabilities.playlist_create ?? true;
  }

  get isRssProvider(): boolean {
    return this.activeProvider === 'rss';
  }

  get canEditPlaylist(): boolean {
    return this.activeProviderInfo?.capabilities.playlist_edit ?? true;
  }

  get canSearch(): boolean {
    return this.activeProviderInfo?.capabilities.search ?? true;
  }

  get filteredPlaylists(): Playlist[] {
    const filter = this.filter.trim().toLowerCase();
    if (!filter) return this.playlists;
    return this.playlists.filter(
      (playlist) =>
        playlist.name.toLowerCase().includes(filter) ||
        playlist.subtitle.toLowerCase().includes(filter),
    );
  }

  get viewTracks(): Track[] {
    if (this.view.kind === 'playlist') return this.playlist?.tracks ?? [];
    if (this.view.kind === 'queue') return this.queue?.items.map((item) => item.track) ?? [];
    return [];
  }

  get selectedTrack(): Track | null {
    return this.viewTracks[this.selection] ?? null;
  }

  get searchTargetName(): string | null {
    if (!this.searchPlaylistId) return null;
    return (
      this.playlists.find((playlist) => playlist.id === this.searchPlaylistId)?.name ??
      (this.playlist?.playlist.id === this.searchPlaylistId
        ? this.playlist.playlist.name
        : null)
    );
  }

  get selectionCount(): number {
    return this.viewTracks.length;
  }

  async refreshStatus(): Promise<void> {
    try {
      this.applyStatus(await this.api.status());
    } catch (error) {
      this.error = describeError(error, this.activeProviderName);
    }
  }

  async loadProviders(): Promise<void> {
    try {
      this.providers = await this.api.providers();
      const known = this.providers.map((provider) => provider.id);
      let stored: string | null = null;
      try {
        stored = globalThis.localStorage?.getItem(PROVIDER_KEY) ?? null;
      } catch {
        // Ignore storage failures.
      }
      if (stored && (known as string[]).includes(stored)) {
        this.activeProvider = stored as ProviderId;
      } else {
        this.activeProvider =
          this.providers.find((provider) => provider.default)?.id ?? 'youtube';
      }
    } catch (error) {
      this.error = describeError(error, this.activeProviderName);
    }
  }

  /// Switch the active provider and reload the library for it.
  async setProvider(provider: ProviderId): Promise<void> {
    if (provider === this.activeProvider) return;
    this.activeProvider = provider;
    try {
      globalThis.localStorage?.setItem(PROVIDER_KEY, provider);
    } catch {
      // Ignore storage failures.
    }
    this.searchResults = null;
    this.searchQuery = '';
    await this.loadPlaylists();
    if (this.view.kind === 'playlist') {
      await this.openPlaylist(this.view.id, this.view.name);
    }
  }

  async refreshQueue(): Promise<void> {
    try {
      this.queue = await this.api.queue();
    } catch (error) {
      this.report(error);
    }
  }

  async loadPlaylists(): Promise<void> {
    this.playlistsLoading = true;
    try {
      this.playlists = await this.api.playlists(this.activeProvider);
      if (this.activeProvider === 'rss') {
        this.rssFeeds = await this.api.rssFeeds();
      }
    } catch (error) {
      this.error = describeError(error, this.activeProviderName);
    } finally {
      this.playlistsLoading = false;
    }
  }

  async loadRssFeeds(): Promise<void> {
    try {
      this.rssFeeds = await this.api.rssFeeds();
    } catch (error) {
      this.report(error);
    }
  }

  async addRssFeed(url: string): Promise<boolean> {
    try {
      const feed = await this.api.addRssFeed(url);
      this.rssFeeds = [...this.rssFeeds, feed];
      await this.loadPlaylists();
      this.notify('success', `Added “${feed.title}”.`);
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async removeRssFeed(id: string): Promise<boolean> {
    try {
      await this.api.removeRssFeed(id);
      this.rssFeeds = this.rssFeeds.filter((feed) => feed.id !== id);
      if (this.view.kind === 'playlist' && this.view.id === id) {
        this.openLibrary();
      }
      await this.loadPlaylists();
      this.notify('success', 'Feed removed.');
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async refreshRssFeed(id: string): Promise<boolean> {
    try {
      const feed = await this.api.refreshRssFeed(id);
      this.rssFeeds = this.rssFeeds.map((known) => (known.id === id ? feed : known));
      await this.loadPlaylists();
      this.notify('success', `Refreshed “${feed.title}”.`);
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async refreshRssFeeds(): Promise<boolean> {
    try {
      this.rssFeeds = await this.api.refreshRssFeeds();
      await this.loadPlaylists();
      this.notify('success', 'Feeds refreshed.');
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async loadCache(): Promise<void> {
    this.cacheLoading = true;
    try {
      this.cache = await this.api.cache();
    } catch (error) {
      this.error = describeError(error, this.activeProviderName);
    } finally {
      this.cacheLoading = false;
    }
  }

  async cleanCache(scope: CacheScope): Promise<void> {
    try {
      const result = await this.api.cleanCache(scope);
      this.notify('success', `Cache cleaned: freed ${formatBytes(result.freed_bytes)}.`);
      await Promise.all([this.loadCache(), this.refreshStatus(), this.refreshQueue()]);
    } catch (error) {
      this.error = describeError(error, this.activeProviderName);
    }
  }

  openPlaylist(id: string, name?: string): Promise<void> {
    this.view = { kind: 'playlist', id, name };
    this.selection = 0;
    this.playlist = null;
    this.playlistLoading = true;
    this.syncLocation();
    return this.loadPlaylist(id);
  }

  private async loadPlaylist(id: string): Promise<void> {
    try {
      this.playlist = await this.api.playlist(id, this.activeProvider);
    } catch (error) {
      this.error = describeError(error, this.activeProviderName);
    } finally {
      this.playlistLoading = false;
    }
  }

  async createPlaylist(name: string): Promise<boolean> {
    try {
      const playlist = await this.api.createPlaylist({
        title: name,
        description: '',
        privacy: 'private',
      });
      await this.loadPlaylists();
      this.notify('success', `Created “${playlist.name}”.`);
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async renamePlaylist(id: string, name: string): Promise<boolean> {
    try {
      const current = this.playlists.find((playlist) => playlist.id === id);
      const updated = await this.api.updatePlaylist(id, { title: name });
      const subtitle = current?.subtitle ?? this.playlist?.playlist.subtitle ?? '';
      this.playlists = this.playlists.map((playlist) =>
        playlist.id === id ? { ...playlist, name: updated.name } : playlist,
      );
      if (this.playlist?.playlist.id === id) {
        this.playlist = {
          ...this.playlist,
          playlist: { ...this.playlist.playlist, name: updated.name, subtitle },
        };
      }
      if (this.view.kind === 'playlist' && this.view.id === id) {
        this.view = { ...this.view, name: updated.name };
      }
      this.notify('success', `Renamed to “${updated.name}”.`);
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async deletePlaylist(id: string): Promise<boolean> {
    try {
      await this.api.deletePlaylist(id);
      this.playlists = this.playlists.filter((playlist) => playlist.id !== id);
      if (this.view.kind === 'playlist' && this.view.id === id) {
        this.openLibrary();
      }
      this.notify('success', 'Playlist deleted.');
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async addPlaylistTracks(id: string, provider: ProviderId, trackIds: string[]): Promise<boolean> {
    try {
      const tracks = await this.api.addPlaylistTracks(id, provider, trackIds);
      if (this.playlist?.playlist.id === id) {
        this.playlist = { ...this.playlist, tracks };
      }
      this.notify(
        'success',
        trackIds.length === 1
          ? 'Track added to the playlist.'
          : `${trackIds.length} tracks added to the playlist.`,
      );
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async removePlaylistTrack(id: string, index: number): Promise<boolean> {
    try {
      const tracks = await this.api.removePlaylistTracks(id, [index]);
      if (this.playlist?.playlist.id === id) {
        this.playlist = { ...this.playlist, tracks };
        this.selection = Math.max(0, Math.min(this.selection, tracks.length - 1));
      }
      this.notify('success', 'Track removed from the playlist.');
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  async movePlaylistTrack(id: string, from: number, to: number): Promise<boolean> {
    try {
      const tracks = await this.api.movePlaylistTrack(id, from, to);
      if (this.playlist?.playlist.id === id) {
        this.playlist = { ...this.playlist, tracks };
        this.selection = to;
      }
      return true;
    } catch (error) {
      this.report(error);
      return false;
    }
  }

  openLibrary(): void {
    this.view = { kind: 'library' };
    this.selection = 0;
    this.playlist = null;
    this.syncLocation();
  }

  openQueue(): void {
    this.view = { kind: 'queue' };
    this.selection = 0;
    this.syncLocation();
  }

  setFilter(filter: string): void {
    this.filter = filter;
  }

  select(index: number): void {
    const count = this.selectionCount;
    if (count === 0) {
      this.selection = 0;
      return;
    }
    this.selection = Math.max(0, Math.min(index, count - 1));
  }

  moveSelection(delta: number): void {
    this.select(this.selection + delta);
  }

  selectFirst(): void {
    this.select(0);
  }

  selectLast(): void {
    this.select(this.selectionCount - 1);
  }

  halfPage(direction: 1 | -1, rows = 8): void {
    this.moveSelection(direction * rows);
  }

  async playSelected(): Promise<void> {
    if (this.view.kind === 'queue') {
      await this.playIndex(this.selection);
      return;
    }
    const track = this.selectedTrack;
    if (track) await this.playTrack(track);
  }

  async playTrack(track: Track): Promise<void> {
    try {
      this.applyStatus(await this.api.playTrack(track));
      await this.refreshQueue();
    } catch (error) {
      this.report(error);
    }
  }

  async playPlaylist(id: string): Promise<void> {
    try {
      this.applyStatus(await this.api.playPlaylist(id, this.activeProvider));
      await this.refreshQueue();
      this.notify('success', 'Playing playlist.');
    } catch (error) {
      this.report(error);
    }
  }

  async playIndex(index: number): Promise<void> {
    if (!this.queue) return;
    const track = this.queue.items[index]?.track;
    if (track) {
      await this.playTrack(track);
      this.selection = index;
    }
  }

  async queueTrack(track: Track): Promise<void> {
    try {
      this.queue = await this.api.queueAdd(track);
      this.notify('info', `Queued “${track.title}”.`);
    } catch (error) {
      this.report(error);
    }
  }

  async queueSelected(): Promise<void> {
    const track = this.selectedTrack;
    if (track) await this.queueTrack(track);
  }

  async togglePlayback(): Promise<void> {
    const playback = this.status?.playback;
    if (!playback) return;
    if (playback.state === 'stopped') {
      const track = playback.current;
      if (track) await this.playTrack(track);
      return;
    }
    const wasPlaying = playback.state === 'playing';
    this.patchPlayback({ state: wasPlaying ? 'paused' : 'playing' });
    try {
      this.applyStatus(wasPlaying ? await this.api.pause() : await this.api.resume());
    } catch (error) {
      this.patchPlayback({ state: playback.state });
      this.report(error);
    }
  }

  async next(): Promise<void> {
    try {
      this.applyStatus(await this.api.next());
      await this.refreshQueue();
    } catch (error) {
      this.report(error);
    }
  }

  async previous(): Promise<void> {
    try {
      this.applyStatus(await this.api.previous());
      await this.refreshQueue();
    } catch (error) {
      this.report(error);
    }
  }

  async setVolume(volume: number): Promise<void> {
    const previous = this.volume;
    const clamped = Math.max(0, Math.min(100, Math.round(volume)));
    this.volume = clamped;
    this.patchPlayback({ volume: clamped });
    try {
      this.applyStatus(await this.api.setVolume(clamped));
    } catch (error) {
      this.volume = previous;
      this.patchPlayback({ volume: previous });
      this.report(error);
    }
  }

  async seek(positionSecs: number): Promise<void> {
    const previous = this.status?.playback.position_secs ?? 0;
    this.patchPlayback({ position_secs: positionSecs });
    try {
      this.applyStatus(await this.api.seek(positionSecs));
    } catch (error) {
      this.patchPlayback({ position_secs: previous });
      this.report(error);
    }
  }

  async toggleShuffle(): Promise<void> {
    const current = this.status?.playback.shuffle ?? false;
    this.patchPlayback({ shuffle: !current });
    try {
      this.applyStatus(await this.api.setShuffle(!current));
    } catch (error) {
      this.patchPlayback({ shuffle: current });
      this.report(error);
    }
  }

  async cycleRepeat(): Promise<void> {
    const current = this.status?.playback.repeat ?? 'off';
    const next: RepeatMode = current === 'off' ? 'all' : current === 'all' ? 'one' : 'off';
    this.patchPlayback({ repeat: next });
    try {
      this.applyStatus(await this.api.setRepeat(next));
    } catch (error) {
      this.patchPlayback({ repeat: current });
      this.report(error);
    }
  }

  async reorderQueue(from: number, to: number): Promise<void> {
    if (!this.queue) return;
    const previous = this.queue;
    this.queue = reorderQueue(previous, from, to);
    try {
      this.queue = await this.api.queueReorder(from, to);
    } catch (error) {
      this.queue = previous;
      this.report(error);
    }
  }

  async removeQueueIndex(index: number): Promise<void> {
    if (!this.queue) return;
    const previous = this.queue;
    this.queue = removeQueueItem(previous, index);
    this.selection = Math.max(0, Math.min(this.selection, this.queue.items.length - 1));
    try {
      this.queue = await this.api.queueRemove(index);
    } catch (error) {
      this.queue = previous;
      this.report(error);
    }
  }

  async playNext(index: number): Promise<void> {
    if (!this.queue) return;
    const previous = this.queue;
    try {
      this.queue = await this.api.queuePlayNext(index);
    } catch (error) {
      this.queue = previous;
      this.report(error);
    }
  }

  async clearQueue(): Promise<void> {
    if (!this.queue) return;
    const previous = this.queue;
    this.queue = { ...previous, items: [], current: null };
    this.selection = 0;
    try {
      this.queue = await this.api.queueClear();
    } catch (error) {
      this.queue = previous;
      this.report(error);
    }
  }

  async retryDownload(track: Track): Promise<void> {
    try {
      await this.api.retryDownload(track);
      this.notify('success', 'Download queued for retry.');
    } catch (error) {
      this.report(error);
    }
  }

  openSearch(): void {
    this.searchMode = 'normal';
    this.searchPlaylistId = null;
    this.searchOpen = true;
  }

  openSearchForPlaylist(playlistId: string): void {
    this.clearSearchTimer();
    this.searchMode = 'add-to-playlist';
    this.searchPlaylistId = playlistId;
    this.searchQuery = '';
    this.searchResults = null;
    this.searchLoading = false;
    this.searchOpen = true;
  }

  closeSearch(): void {
    this.searchOpen = false;
    this.searchMode = 'normal';
    this.searchPlaylistId = null;
  }

  async addSearchSelection(track: Track): Promise<void> {
    if (this.searchMode === 'add-to-playlist' && this.searchPlaylistId) {
      await this.addPlaylistTracks(this.searchPlaylistId, track.provider, [track.track_id]);
      return;
    }
    this.closeSearch();
    await this.playTrack(track);
  }

  setSearchQuery(query: string): void {
    this.searchQuery = query;
    this.clearSearchTimer();
    if (!query.trim()) {
      this.searchResults = null;
      this.searchLoading = false;
      return;
    }
    const timeout = this.options.setTimeoutFn ?? setTimeout;
    this.searchTimer = timeout(() => {
      void this.searchNow();
    }, this.options.searchDebounceMs);
  }

  async searchNow(): Promise<void> {
    const query = this.searchQuery.trim();
    if (!query) return;
    this.searchLoading = true;
    try {
      const results = await this.api.search(query, this.activeProvider);
      if (this.searchQuery.trim() === query) this.searchResults = results;
    } catch (error) {
      this.searchResults = null;
      this.report(error);
    } finally {
      this.searchLoading = false;
    }
  }

  toggleHelp(): void {
    this.helpOpen = !this.helpOpen;
  }

  openFullPlayer(): void {
    this.fullPlayerOpen = true;
  }

  closeFullPlayer(): void {
    this.fullPlayerOpen = false;
  }

  toggleFullPlayer(): void {
    this.fullPlayerOpen = !this.fullPlayerOpen;
  }

  setPlayerLayout(layout: PlayerLayout): void {
    this.playerLayout = layout;
    try {
      globalThis.localStorage?.setItem(PLAYER_LAYOUT_KEY, layout);
    } catch {
      // Ignore storage failures.
    }
  }

  togglePlayerLayout(): void {
    this.setPlayerLayout(this.playerLayout === 'compact' ? 'terminal' : 'compact');
  }

  closeOverlays(): void {
    if (this.fullPlayerOpen) {
      this.fullPlayerOpen = false;
      return;
    }
    if (this.searchOpen) {
      this.searchOpen = false;
      return;
    }
    if (this.helpOpen) this.helpOpen = false;
  }

  retryConnection(): void {
    this.events.retry();
  }

  handleEvent(event: EventKind): void {
    switch (event.type) {
      case 'snapshot':
        this.status = event.status;
        this.queue = event.queue;
        this.volume = event.status.playback.volume;
        break;
      case 'playback':
        this.patchPlayback(event.playback);
        break;
      case 'queue':
        this.queue = event.queue;
        if (this.status) {
          this.status = { ...this.status, queue_length: event.queue.items.length };
        }
        break;
      case 'download':
        if (this.queue) {
          this.queue = {
            ...this.queue,
            items: this.queue.items.map((item) =>
              item.track.provider === event.provider && item.track.track_id === event.track_id
                ? { ...item, download: event.status }
                : item,
            ),
          };
        }
        this.scheduleStatusRefresh();
        break;
      case 'track_ended':
        this.scheduleStatusRefresh();
        break;
      case 'error':
        this.notify('error', event.error.message);
        break;
      case 'library_changed':
        void this.loadPlaylists();
        break;
    }
  }

  notify(
    severity: ToastSeverity,
    message: string,
    action?: Toast['action'],
  ): void {
    const toast: Toast = { id: ++this.toastSeq, severity, message, action };
    this.toasts = [...this.toasts, toast];
    const timeout = this.options.setTimeoutFn ?? setTimeout;
    const ttl = severity === 'error' ? this.options.toastTtlMs * 2 : this.options.toastTtlMs;
    const id = timeout(() => {
      this.liveToasts.delete(id);
      this.dismissToast(toast.id);
    }, ttl);
    this.liveToasts.add(id);
  }

  dismissToast(id: number): void {
    this.toasts = this.toasts.filter((toast) => toast.id !== id);
  }

  downloadStatusFor(track: Track): DownloadStatus {
    return (
      this.queue?.items.find(
        (item) => item.track.provider === track.provider && item.track.track_id === track.track_id,
      )?.download ?? {
        state: 'not_downloaded',
      }
    );
  }

  private applyStatus(status: Status): void {
    this.status = status;
    this.volume = status.playback.volume;
  }

  private patchPlayback(patch: Partial<Status['playback']>): void {
    if (!this.status) return;
    this.status = {
      ...this.status,
      playback: { ...this.status.playback, ...patch },
    };
    if (patch.volume !== undefined) this.volume = patch.volume;
  }

  private report(error: unknown): void {
    if (error instanceof ApiError && error.code === 'network_error') {
      this.connection = 'offline';
    }
    if (
      error instanceof ApiError &&
      error.code === 'upstream_error' &&
      looksLikeAuthError(error.message)
    ) {
      this.authExpired = true;
    }
    this.notify('error', describeError(error, this.activeProviderName));
  }

  private scheduleStatusRefresh(): void {
    this.clearStatusRefreshTimer();
    const timeout = this.options.setTimeoutFn ?? setTimeout;
    this.statusRefreshTimer = timeout(() => {
      this.statusRefreshTimer = null;
      void this.refreshStatus();
    }, 300);
  }

  private clearStatusRefreshTimer(): void {
    if (this.statusRefreshTimer !== null) {
      const clear = this.options.clearTimeoutFn ?? clearTimeout;
      clear(this.statusRefreshTimer);
      this.statusRefreshTimer = null;
    }
  }

  private clearSearchTimer(): void {
    if (this.searchTimer !== null) {
      const clear = this.options.clearTimeoutFn ?? clearTimeout;
      clear(this.searchTimer);
      this.searchTimer = null;
    }
  }

  private parseLocation(): void {
    const hash = window.location.hash;
    if (hash.startsWith('#/queue')) {
      this.view = { kind: 'queue' };
    } else if (hash.startsWith('#/playlist/')) {
      const id = decodeURIComponent(hash.slice('#/playlist/'.length));
      if (id) this.view = { kind: 'playlist', id };
    } else {
      this.view = { kind: 'library' };
    }
  }

  private syncLocation(): void {
    const hash =
      this.view.kind === 'library'
        ? '#/library'
        : this.view.kind === 'queue'
          ? '#/queue'
          : `#/playlist/${encodeURIComponent(this.view.id)}`;
    if (window.location.hash !== hash) {
      window.history.replaceState(null, '', hash);
    }
  }
}
