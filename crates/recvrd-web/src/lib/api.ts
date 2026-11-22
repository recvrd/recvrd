import type { CacheCleanResult } from '@api/CacheCleanResult';
import type { CacheScope } from '@api/CacheScope';
import type { CacheSizes } from '@api/CacheSizes';
import type { CreatePlaylistRequest } from '@api/CreatePlaylistRequest';
import type { DownloadStatus } from '@api/DownloadStatus';
import type { ErrorEnvelope } from '@api/ErrorEnvelope';
import type { OkResponse } from '@api/OkResponse';
import type { OfflineRequest } from '@api/OfflineRequest';
import type { PlaybackMode } from '@api/PlaybackMode';
import type { PlaybackSnapshot } from '@api/PlaybackSnapshot';
import type { Playlist } from '@api/Playlist';
import type { PlaylistDetail } from '@api/PlaylistDetail';
import type { ProviderCapabilities } from '@api/ProviderCapabilities';
import type { ProviderId } from '@api/ProviderId';
import type { ProviderInfo } from '@api/ProviderInfo';
import type { Queue } from '@api/Queue';
import type { RepeatMode } from '@api/RepeatMode';
import type { RssFeed } from '@api/RssFeed';
import type { SearchResults } from '@api/SearchResults';
import type { Status } from '@api/Status';
import type { Track } from '@api/Track';
import type { TrackDetail } from '@api/TrackDetail';
import type { UpdatePlaylistRequest } from '@api/UpdatePlaylistRequest';

export class ApiError extends Error {
  readonly code: string;
  readonly status: number | null;

  constructor(message: string, code = 'network_error', status: number | null = null) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.status = status;
  }
}

export interface RecvrdApi {
  status(): Promise<Status>;
  providers(): Promise<ProviderInfo[]>;
  setPlaybackMode(provider: ProviderId, mode: PlaybackMode): Promise<ProviderInfo>;
  offline(): Promise<OfflineRequest>;
  setOffline(enabled: boolean): Promise<Status>;
  playlists(provider?: ProviderId): Promise<Playlist[]>;
  playlist(id: string, provider?: ProviderId): Promise<PlaylistDetail>;
  createPlaylist(request: CreatePlaylistRequest): Promise<Playlist>;
  updatePlaylist(id: string, request: UpdatePlaylistRequest): Promise<Playlist>;
  deletePlaylist(id: string): Promise<OkResponse>;
  addPlaylistTracks(id: string, provider: ProviderId, trackIds: string[]): Promise<Track[]>;
  removePlaylistTracks(id: string, indices: number[]): Promise<Track[]>;
  movePlaylistTrack(id: string, from: number, to: number): Promise<Track[]>;
  search(query: string, provider?: ProviderId): Promise<SearchResults>;
  trackDetail(provider: ProviderId, trackId: string): Promise<TrackDetail>;
  queue(): Promise<Queue>;
  playPlaylist(playlistId: string, provider?: ProviderId): Promise<Status>;
  playTrack(track: Track): Promise<Status>;
  queueAdd(track: Track): Promise<Queue>;
  queueRemove(index: number): Promise<Queue>;
  queueReorder(from: number, to: number): Promise<Queue>;
  queuePlayNext(index: number): Promise<Queue>;
  queueClear(): Promise<Queue>;
  pause(): Promise<Status>;
  resume(): Promise<Status>;
  next(): Promise<Status>;
  previous(): Promise<Status>;
  setVolume(volume: number): Promise<Status>;
  seek(positionSecs: number): Promise<Status>;
  setShuffle(enabled: boolean): Promise<Status>;
  setRepeat(repeat: RepeatMode): Promise<Status>;
  retryDownload(track: Track): Promise<void>;
  cache(): Promise<CacheSizes>;
  cleanCache(scope: CacheScope): Promise<CacheCleanResult>;
  rssFeeds(): Promise<RssFeed[]>;
  addRssFeed(url: string): Promise<RssFeed>;
  removeRssFeed(id: string): Promise<OkResponse>;
  refreshRssFeed(id: string): Promise<RssFeed>;
  refreshRssFeeds(): Promise<RssFeed[]>;
}

export function describeError(error: unknown, providerName?: string): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case 'auth_expired':
        return error.message;
      case 'upstream_error':
        return providerName
          ? `${providerName} rejected the request: ${error.message}`
          : error.message;
      case 'bad_request':
        return `Invalid request: ${error.message}`;
      case 'not_found':
        return `Not found: ${error.message}`;
      case 'internal_error':
        return `Daemon error: ${error.message}`;
      case 'network_error':
        return 'Daemon is unreachable. Is recvrd running?';
      default:
        return error.message;
    }
  }
  if (error instanceof Error) return error.message;
  return String(error);
}

async function parseError(response: Response): Promise<ApiError> {
  let code = 'http_error';
  let message = `${response.status} ${response.statusText}`;
  try {
    const body = (await response.json()) as ErrorEnvelope;
    if (body?.error) {
      code = body.error.code;
      message = body.error.message;
    }
  } catch {
    // Keep the status line as the message.
  }
  return new ApiError(message, code, response.status);
}

export function createApi(baseUrl = ''): RecvrdApi {
  async function request<T>(path: string, init?: RequestInit): Promise<T> {
    let response: Response;
    try {
      response = await fetch(`${baseUrl}${path}`, {
        ...init,
        headers: {
          'content-type': 'application/json',
          ...(init?.headers ?? {}),
        },
      });
    } catch (error) {
      throw new ApiError(
        error instanceof Error ? error.message : 'network failure',
        'network_error',
      );
    }
    if (!response.ok) throw await parseError(response);
    if (response.status === 202 || response.status === 204) {
      return undefined as T;
    }
    return (await response.json()) as T;
  }

  const post = <T>(path: string, body?: unknown) =>
    request<T>(path, {
      method: 'POST',
      body: body === undefined ? undefined : JSON.stringify(body),
    });

  const patch = <T>(path: string, body: unknown) =>
    request<T>(path, {
      method: 'PATCH',
      body: JSON.stringify(body),
    });

  const remove = <T>(path: string) => request<T>(path, { method: 'DELETE' });

  return {
    status: () => request<Status>('/api/status'),
    providers: () => request<ProviderInfo[]>('/api/providers'),
    setPlaybackMode: (provider, mode) =>
      request<ProviderInfo>(`/api/providers/${encodeURIComponent(provider)}/playback`, {
        method: 'PUT',
        body: JSON.stringify({ mode }),
      }),
    offline: () => request<OfflineRequest>('/api/offline'),
    setOffline: (enabled) =>
      request<Status>('/api/offline', {
        method: 'PUT',
        body: JSON.stringify({ enabled }),
      }),
    playlists: (provider: ProviderId = 'youtube') =>
      request<Playlist[]>(`/api/playlists?provider=${encodeURIComponent(provider)}`),
    playlist: (id, provider: ProviderId = 'youtube') =>
      request<PlaylistDetail>(
        `/api/playlists/${encodeURIComponent(id)}?provider=${encodeURIComponent(provider)}`,
      ),
    createPlaylist: (requestBody) => post<Playlist>('/api/playlists', requestBody),
    updatePlaylist: (id, requestBody) =>
      patch<Playlist>(`/api/playlists/${encodeURIComponent(id)}`, requestBody),
    deletePlaylist: (id) =>
      remove<OkResponse>(`/api/playlists/${encodeURIComponent(id)}`),
    addPlaylistTracks: (id, provider, trackIds) =>
      post<Track[]>(`/api/playlists/${encodeURIComponent(id)}/tracks`, {
        provider,
        track_ids: trackIds,
      }),
    removePlaylistTracks: (id, indices) =>
      post<Track[]>(`/api/playlists/${encodeURIComponent(id)}/tracks/remove`, {
        indices,
      }),
    movePlaylistTrack: (id, from, to) =>
      post<Track[]>(`/api/playlists/${encodeURIComponent(id)}/tracks/move`, {
        from,
        to,
      }),
    search: (query, provider: ProviderId = 'youtube') =>
      request<SearchResults>(
        `/api/search?provider=${encodeURIComponent(provider)}&q=${encodeURIComponent(query)}`,
      ),
    trackDetail: (provider, trackId) =>
      request<TrackDetail>(
        `/api/tracks/${encodeURIComponent(provider)}/${encodeURIComponent(trackId)}`,
      ),
    rssFeeds: () => request<RssFeed[]>('/api/rss/feeds'),
    addRssFeed: (url) => post<RssFeed>('/api/rss/feeds', { url }),
    removeRssFeed: (id) => remove<OkResponse>(`/api/rss/feeds/${encodeURIComponent(id)}`),
    refreshRssFeed: (id) =>
      post<RssFeed>(`/api/rss/feeds/${encodeURIComponent(id)}/refresh`, {}),
    refreshRssFeeds: () => post<RssFeed[]>('/api/rss/feeds/refresh', {}),
    queue: () => request<Queue>('/api/queue'),
    playPlaylist: (playlistId, provider: ProviderId = 'youtube') =>
      post<Status>('/api/play/playlist', { playlist_id: playlistId, provider }),
    playTrack: (track) => post<Status>('/api/play/track', { track }),
    queueAdd: (track) => post<Queue>('/api/queue', { track }),
    queueRemove: (index) => post<Queue>('/api/queue/remove', { index }),
    queueReorder: (from, to) => post<Queue>('/api/queue/reorder', { from, to }),
    queuePlayNext: (index) => post<Queue>('/api/queue/play-next', { index }),
    queueClear: () => request<Queue>('/api/queue', { method: 'DELETE' }),
    pause: () => post<Status>('/api/pause'),
    resume: () => post<Status>('/api/resume'),
    next: () => post<Status>('/api/next'),
    previous: () => post<Status>('/api/previous'),
    setVolume: (volume) => post<Status>('/api/volume', { volume }),
    seek: (positionSecs) => post<Status>('/api/seek', { position_secs: positionSecs }),
    setShuffle: (enabled) => post<Status>('/api/shuffle', { enabled }),
    setRepeat: (repeat) => post<Status>('/api/repeat', { repeat }),
    retryDownload: (track) =>
      post<void>('/api/downloads/retry', {
        provider: track.provider,
        track_id: track.track_id,
      }),
    cache: () => request<CacheSizes>('/api/cache'),
    cleanCache: (scope) =>
      post<CacheCleanResult>('/api/cache/clean', { scope }),
  };
}

export function coverUrl(track: Track): string {
  return `/api/tracks/${encodeURIComponent(track.provider)}/${encodeURIComponent(track.track_id)}/cover`;
}

export function durationSeconds(duration: string): number {
  const parts = duration.split(':').map(Number);
  if (parts.some((part) => !Number.isFinite(part))) return 0;
  if (parts.length === 2) return parts[0] * 60 + parts[1];
  if (parts.length === 3) return parts[0] * 3600 + parts[1] * 60 + parts[2];
  return 0;
}

export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return '0:00';
  const total = Math.floor(seconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
  }
  return `${minutes}:${String(secs).padStart(2, '0')}`;
}

export function formatBytes(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${bytes} B`;
  return `${value.toFixed(1)} ${units[unit]}`;
}

export function downloadLabel(status: DownloadStatus): string {
  switch (status.state) {
    case 'downloading':
      return `${status.progress}%`;
    case 'not_downloaded':
      return 'not cached';
    case 'downloaded':
      return 'cached';
    case 'retrying':
      return `retry ${status.attempt}`;
    case 'failed':
      return 'failed';
  }
}

export function playbackModeLabel(mode: PlaybackMode): string {
  switch (mode) {
    case 'download':
      return 'Download';
    case 'stream':
      return 'Stream';
    case 'stream_then_download':
      return 'Stream + cache';
  }
}

/// The provider metadata that fits in a compact line. Empty when the
/// provider supplies no metadata or repeats the album.
export function trackMetadataFacts(track: Track): string[] {
  const metadata = track.metadata;
  if (!metadata) return [];
  const facts: string[] = [];
  if (metadata.channel_title && metadata.channel_title !== track.album) {
    facts.push(metadata.channel_title);
  }
  if (metadata.season_number !== null && metadata.season_number !== undefined) {
    const episode =
      metadata.episode_number !== null && metadata.episode_number !== undefined
        ? `E${metadata.episode_number}`
        : '';
    facts.push(`S${metadata.season_number}${episode}`);
  } else if (metadata.episode_number !== null && metadata.episode_number !== undefined) {
    facts.push(`E${metadata.episode_number}`);
  }
  if (metadata.year) facts.push(String(metadata.year));
  if (metadata.genre) facts.push(metadata.genre);
  return facts;
}

export type { CacheCleanResult, CacheScope, CacheSizes, OfflineRequest, PlaybackMode, PlaybackSnapshot, Playlist, PlaylistDetail, ProviderCapabilities, ProviderId, ProviderInfo, Queue, RepeatMode, RssFeed, SearchResults, Status, Track, TrackDetail };
