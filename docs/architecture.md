# Architecture

## Goals

- A single long-running daemon owns playback, queue, downloads and auth state.
- Clients (web UI, terminal client, MCP proxy, MPRIS) are thin and stateless.
- The HTTP/JSON/SSE API is the contract between the daemon and clients.
- The daemon works headless: no terminal, window or interactive prompt required.
- Media controls work through MPRIS; deployment works through systemd/Nix.

## Process model

`recvrd serve` starts one process with:

- a multi-threaded Tokio runtime and one HTTP listener;
- one audio sink and one queue service;
- one download manager;
- one event bus;
- one MPRIS/D-Bus client when enabled.

The daemon binds to `127.0.0.1:8080` by default and shuts down gracefully on
SIGINT/SIGTERM. State is owned by the daemon, not by an individual client.

## Components

| Component | Status | Responsibility |
| --- | --- | --- |
| `crates/recvrd` | Current | CLI, HTTP/SSE server, MPRIS integration, MCP proxy entrypoint |
| `recvrd-core` | Current (#6/#7) | Auth, config, paths, queue/playback and event bus |
| `recvrd-api` | Current (#11) | Shared DTOs, event types and generated TypeScript |
| `crates/recvrd-web` | Current (#14–#17) | Svelte 5 web UI (embedded at build time) |
| `crates/recvrd-client` | Current | HTTP and SSE client shared by the terminal client and MCP |
| `crates/recvc` | Current | Terminal client (`recvc`) |
| `crates/ytpapi2` | Current | YouTube Music API client |
| `crates/provider-youtube` | Current | YouTube Music provider: API client, cover fetcher, downloads |
| `crates/provider-rss` | Current | RSS provider: feed parser, store, fetcher, downloader |
| `crates/provider-podverse` | Current | Podverse provider: channel search, item metadata, fetcher, downloader |
| `crates/player` | Current | Audio decoding/playback |
| `crates/database` | Current | Local download database |
| `crates/download-manager` | Current | Download queue |

| `crates/common-structs` | Current | Shared status types |

## Scrobbling

The daemon sends music plays to Last.fm when `lastfm.enabled` is true. The
playback service emits a now-playing event when a music track starts. It
emits a scrobble after half the track or four minutes, whichever comes
first. RSS episodes do not scrobble. The worker keeps failed and offline
scrobbles in memory and retries them.

## Data flow

```
client ──REST──▶ daemon ──command──▶ core service ──▶ event bus
                    ▲                                  │
                    └──────── snapshot ◀───────────────┘
                                                       │
web / TUI / MCP ◀───────── SSE ─────────────────────────┘
                                                       │
                                                     MPRIS
```

Commands are validated, applied to the core state, and then published as typed
events. New SSE clients receive a snapshot followed by incremental events.
Clients render from the snapshot/event stream and never poll for the current
track.

The web UI is a Svelte 5 app. It is built with Vite and embedded into the
daemon with `rust-embed`. The daemon serves it at `/` with a client-side
routing fallback. See [docs/web.md](web.md).

## State and storage

- **Config**: `config.toml` under the XDG config directory.
- **Auth**: `headers.txt` (`Cookie` + `User-Agent`) and optional
  `account_id.txt`; never committed or logged.
- **Cache**: downloaded audio and `<id>.json` files, `covers/<id>.jpg` cover
  art and `db.bin`. Audio files are provider-scoped: `downloads/<provider>/`
  and `covers/<provider>/`. A download keeps the extension its backend
  reports: `mp4` for YouTube, `mp3`, `m4a`, `ogg` or `flac` for RSS.
- **RSS feeds**: `rss-feeds.json` in the data directory. It holds feed URLs,
  ETags, Last-Modified values and the episode index.
- Runtime state: queue, current index, current position, per-track positions,
  volume, shuffle/repeat, written atomically after commands, periodically while
  the daemon runs and on shutdown, then restored on startup (invalid state is
  quarantined as `state.json.bad`).

The daemon uses the `com/suorcd/recvrd` directories. On the first start it
moves the `com/suorcd/ytermusicd` directories to the new paths. It falls back
to the upstream `com/ccgauche/ytermusic` headers file with a warning.

## Downloads

The daemon owns a `DownloadManager` with `parallel_downloads` workers. Playing
a playlist or track and enqueueing a track records the metadata and adds the
audio to the download queue. Playback waits for the cache file to appear and
starts automatically when the manager emits `Downloaded`; failures skip to the
next track and surface an error event. `/api/status` reports queued/active/
completed/retrying/failed counts, and `/api/downloads/retry` re-queues a failed
track. The web UI and MCP proxy never download directly.

## Playlist edits

The daemon sends playlist changes through the authenticated YouTube Music
`playlist/create`, `browse/edit_playlist` and `playlist/delete` endpoints.
Remove and move actions need each playlist item's `setVideoId`. The daemon
fetches the playlist before an edit, maps the requested index to that ID and
returns the new track list.

The API rejects invalid titles, video IDs and indices before it sends an
upstream request. An expired session or a rate limit returns an actionable
message. A failed upstream edit does not change local playback state.

## RSS provider

`crates/provider-rss` follows RSS 2.0 and Atom feeds with the `rss`
provider ID. A feed is a playlist and its episodes are tracks. The
provider parses feeds with `quick-xml`, stores feed URLs with ETags and
Last-Modified values in `rss-feeds.json`, and refreshes with conditional
GETs. An episode ID is the first 32 hex characters of the SHA-256 hash
of its GUID or enclosure URL, so a refresh never duplicates an episode.
Enclosures download through the same manager as YouTube tracks, with
the extension from the enclosure MIME type or URL suffix: `mp3`, `m4a`,
`ogg` or `flac`. Playback decodes them with rodio. RSS feeds are
read-only: playlist edits stay on YouTube. The daemon refreshes all
feeds at startup and exposes feed management at `/api/rss/feeds`.

## Podverse provider

`crates/provider-podverse` follows a configured Podverse API with the
`podverse` provider ID. A channel is a playlist and its items are tracks.
Search uses the instance's Podcast Index proxy and keeps only `music` feeds.
A search result carries a Podcast Index ID, so the provider resolves it to
the Podverse channel ID before it reads the items. The provider adds a
`TrackSource` with the enclosure URL, MIME type, byte length and media kind.
It also adds `TrackMetadata` with the channel, feed, publication and season
fields. `GET /api/tracks/podverse/{track_id}` returns the full item
description. The provider is hidden unless `podverse.base_url` is set.
Podverse channels are read-only.

## Playback

The daemon plays one track at a time through a dedicated audio thread. A
cache file is played when one exists. Otherwise the provider's playback mode
decides what happens:

- `download` waits for the download manager and starts the file when it is
  ready.
- `stream` opens the remote source through `stream-download` and keeps no
  cache.
- `stream_then_download` opens the remote source and queues the cache
  download. If the stream cannot open, it falls back to the download queue.

The stream reader is opened on the async runtime; the audio thread receives an
open blocking reader and performs no network I/O. Modes and their defaults are
recorded in
[0006 — Playback modes and the settings file](decisions/0006-playback-modes.md).
Runtime choices live in `$XDG_CONFIG_HOME/recvrd/settings.toml`.

Offline mode changes queue behavior. While it is on, the service only starts
cached tracks. It skips an uncached track like a failed download. When the mode
turns on, it stops an uncached current track and advances to the next cached
track. When no cached track remains, playback stops. Next and previous land on
cached tracks only. Explicit play of an uncached track fails. Repeat-one never
restarts an uncached track. A track counts as cached with a valid key and a
non-empty audio file.

The switch lives in `settings.toml` and survives a restart. Enabling it stops
and clears in-flight downloads and resets their queue status. The daemon skips
the RSS refresh and restored download queue at startup while offline. The
YouTube client is created lazily: startup does not fetch the homepage, and
turning the switch off creates the client without a restart. Cover art uses
only cached files while offline. Server routing returns cached library,
playlist, search and track data without provider calls; anything that needs a
provider returns 409 `offline_unavailable`.

## API

The full contract and curl examples live in [docs/api.md](api.md). DTOs come
from `recvrd-api`, which also generates the TypeScript bindings consumed by
the web UI and MCP proxy.

- REST: status, providers, playlists, playlist edits, playlist detail, search,
  track detail, cover art, queue CRUD/reorder, play playlist/track,
  pause/resume, next/previous, volume, seek, shuffle, repeat.
- Every command returns the updated `Status` or `Queue`, so clients can render
  without waiting for the event stream.
- SSE: an initial snapshot followed by playback, queue, download, track-ended,
  error and library events. Event ids are monotonic and a 256-event ring
  supports `Last-Event-ID` replay; stale clients get a fresh snapshot.
- Errors use one JSON envelope. The API is loopback-only by default and does not
  enable wildcard CORS.

## Terminal client

`recvc` is a thin client of the daemon. It uses `recvrd-api` DTOs
and `recvrd-client` for REST and SSE. It does not read cookies, the database
or the player. The daemon owns all state.

The client shows the library, one playlist, the queue, search, the now-playing
bar and cover art. It supports vim keys and mouse actions. It reconnects with
backoff and sends `Last-Event-ID` to replay missed events. See [tui.md](tui.md).

The MCP proxy uses the same `recvrd-client` layer. A new daemon endpoint
needs one client method, and both clients can use it.

## Media controls (MPRIS)

On Linux the daemon starts a `souvlaki` MPRIS client on a dedicated thread when
`player.dbus = true` (the default). Clients see the identity and display name
`recvrd`. `playerctl -l` lists it while the daemon runs.

- Supported: play/pause/toggle, next, previous, stop, seek forward/backward by
  the requested amount, set position and set volume.
- Metadata and playback progress come from the event bus, so shell widgets and
  the HTTP API always agree.
- Cover art comes from the cover cache. The daemon fetches the image in the
  background and sends a `file://` URI in `mpris:artUrl`.
- `OpenUri` and `Raise` are explicitly ignored (logged at info/debug level)
  instead of panicking: the daemon has no window to raise and does not accept
  arbitrary URIs.
- If D-Bus is unavailable, startup logs a warning and the daemon continues
  headless; the HTTP API and playback are unaffected. Set `player.dbus = false`
  to skip MPRIS entirely.
- Non-Linux platforms compile a no-op MPRIS stub; no winit window is required.

## MCP

`recvrd mcp` is a stdio proxy to the REST API. It uses `rmcp` and holds no
playback state. The proxy has 36 tools: search, channel and playlist metadata,
track detail, playback modes, playlist edits, playback transport, seek,
shuffle, repeat, queue management, queue add, now playing, volume, download
status and RSS feed management. It reads the daemon URL from `--url` or
`RECVRD_URL`, applies a 30-second timeout, and exits when stdin closes. See
[docs/mcp.md](mcp.md). Lyrics and HTTP transport are post-v1.

## Security

- Cookies/SAPISID must never appear in logs or API responses.
- The API binds to loopback unless explicitly configured otherwise.
- The systemd unit runs as a user service with hardening.
- MCP is a local stdio proxy; it holds no playback state.

## Decisions

Initial decisions are recorded in [docs/decisions](decisions/):

- the daemon owns playback/queue state;
- the API is loopback-only by default;
- the web UI uses Svelte + Vite + TypeScript by default;
- the terminal client is a daemon client, not a standalone player.

## Open questions

- v1 playlist editing scope (#19).
- Upstream fork/PR strategy (#5).
