# Changelog

## Unreleased

### Added

- Save the playback position of each track, up to 1000 entries, and resume each
  track at its saved position.
- Add a Like action for YouTube Music tracks. The web UI track menu sets the
  like state, and the track list shows a liked marker.
- Add opt-in Last.fm scrobbling for music tracks. The daemon sends a
  now-playing update and a scrobble after half the track or four minutes.
  Plays made while offline are queued and sent later.
- Add opt-in ListenBrainz scrobbling for music tracks. The daemon sends a
  now-playing update and a single listen after half the track or four
  minutes. Plays made while offline are queued and sent later.
- Add a configurable PipeWire sample rate and format. The default stays
  48 kHz f32, and `/api/status` reports the output format.
- Add a local-files provider. Set `local.music_dir` to play audio files from
  that directory. Local files come first in the provider order.
- Read tag metadata and cover art for local files. The file name and folder
  remain the fallback.
- Add Queue next and Queue last actions. Queue next inserts a track after the
  current track. Queue last appends it. The web UI, TUI and MCP offer both.
- Add a Geek stats track action and `GET /api/tracks/{provider}/{track_id}/stats`.
  The track menu and full player open it. It shows the provider key, source,
  local file, audio header, playback mode and listen history. URL queries and
  tokens are removed.
- Add a full-player visualizer. The daemon streams 64 spectrum bands from the
  PipeWire output at `/api/visualizer/events`. The web toggle is off by default
  and stored in `localStorage`.

### Removed

- Remove the legacy `ytermusic` TUI. `recvc` is the terminal client.

### Fixed

- Keep a playlist bound to its provider. Changing the provider closes an open
  playlist, and the playlist URL includes the provider.
- Keep a YouTube playlist on YouTube when a legacy URL or a stale list names
  another provider.

## 0.7.0 — 2026-11-22

### Added

- Add a Podverse provider with channel search, channel items and item detail.
- Add `TrackSource`, `TrackMetadata` and `MediaKind` to the API and queue
  state. Populate media sources for RSS.
- Add `GET /api/tracks/{provider}/{track_id}` for full track detail.
- Add per-provider playback modes: `download`, `stream` and
  `stream_then_download`. RSS and Podverse can stream; YouTube stays on
  download. Set the mode with `PUT /api/providers/{provider}/playback`.
- Add `PlaybackSourceKind` to `Status` as `playback.source_kind`.
- Save playback modes in `$XDG_CONFIG_HOME/recvrd/settings.toml`.
- Add an offline mode that plays cached content. The switch is exposed in the
  API, the web UI, the TUI and MCP. While it is on, library, playlist, search
  and track detail use cached data only, and the queue skips uncached tracks.
- Add the cached-library index and a `cached` flag on list responses.
- Persist the offline switch in `settings.toml`. A restart in offline mode
  skips RSS refresh, restored downloads and YouTube client creation.
- Add the `offline_unavailable` error code and an in-app explanation for the
  web UI.
- Add a `cache_limit_mb` download cache limit (default 10240 MiB, 0 disables).
  The daemon evicts the oldest complete downloads first and keeps the current
  queue.

### Changed

- Rename the project to `recvrd`. The daemon is `recvrd`, the terminal client
  is `recvc`, and the crates are `recvrd-*`. The old `ytermusicd` and `ytmc`
  binaries, `YTERMUSICD_*` variables and Nix options remain aliases for one
  release.
- Move the XDG directories from `ytermusicd` to `recvrd` on first start.
- Change the MPRIS name, API name, web title and `localStorage` keys to
  `recvrd`.
- Add `offline` to `Status` and an `offline` SSE event.
- Call the daemon-unreachable state "disconnected" in the web UI and TUI, so it
  is distinct from the manual offline mode.

### Fixed

- Seek a stream when no file track played first. Seeking a stream no longer
  requires a current file and never replays a stale file track.
- Fail fast in `stream` mode when the provider supplies no remote source,
  instead of waiting for a download that never starts.
- Prefer an existing cache file over a stream after a restart.
- Report a provider network failure as `upstream_error`, not `auth_expired`,
  when no live client is available to confirm the session state.
- Cancel in-flight downloads when offline mode turns on and reset their queue
  status, so no track stays at `Downloading`.
- Do not initialize the YouTube client for a Podverse or RSS request, so those
  providers keep working when the YouTube session is invalid.

## 0.6.3 — 2026-11-20

### Fixed

- Detect an expired YouTube Music session from the homepage login flag and
  the InnerTube `logged_in` value.
- Report an expired session as HTTP 401 `auth_expired` for library reads and
  playlist actions. Show the state in the API, the web UI and `ytmc`.
- Keep search, single-track playback and signed-out public playlists
  available with an expired session.
- Load a public playlist detail directly. Read nested album headers.
- Clear the web auth banner after a daemon restart with a fresh cookie.
- Keep one auth banner for an expired library, search or queue load.
- Fix the cover path in the local smoke test.

### Changed

- Add `AuthState::Expired` to the API and the generated TypeScript bindings.
- Map `auth_expired` to an invalid-parameter error in MCP.

## 0.6.2 — 2026-11-19

### Fixed

- Fix browser timer calls in the web UI. The event stream now starts, and the
  `gg` and `dd` key sequences work.

### Changed

- Run CI on pull requests and manual dispatch. Remove six duplicate manual
  workflows.
- Run one build at a time for each release ref. Build release packages with
  the committed `Cargo.lock` file.

## 0.6.1 — 2026-11-19

### Fixed

- Fix the macOS and Windows builds. Limit the MPRIS volume update to Linux.
- Keep the terminal client connected when the SSE stream sends a heartbeat.

## 0.6.0 — 2026-11-19

### Added

- Add an RSS music provider. Add, refresh and remove feeds, and play episodes
  from the library, the API, the web UI and MCP.
- Add provider selection to the API, the web UI, `ytmc` and MCP. Requests can
  name a provider, and each provider has its own caches.
- Add a cache clean action to the API and the web UI.
- Add 8 MCP tools for seek, shuffle, repeat, queue edits and download retry.
- Render `ytmc` cover art with the Kitty, Sixel or iTerm2 protocol when the
  terminal supports one. Keep Unicode half blocks as the fallback, and use
  `--cover-mode halfblocks` or `YTMC_COVER_MODE` to force them.
- Extend the web UI keymap with the `ytmc` bindings. Add view switching,
  sidebar focus, queue edits, playlist actions, playback controls and
  relative seek.
- Send a named `heartbeat` event from the SSE endpoint, and recover the web UI
  event stream after suspend.

### Changed

- Use provider-neutral track IDs. The API DTOs, the MCP tools and the caches
  now carry a provider value and a track ID.
- Rename the default branch to `main`.

### Fixed

- Reject invalid video IDs at the play and queue endpoints.
- Reject seek positions that cannot become a `Duration`.
- Kill `yt-dlp` when a download is cancelled, and require the MP4 file and the
  sidecar before a track counts as downloaded.
- Build InnerTube request bodies with `serde_json`, and encode query values.
- Bound `ytpapi2` browse calls with a timeout and retries, and follow search
  continuations when the caller allows them.
- Keep the unshuffled queue order across a restart.
- Write `db.bin` atomically, return typed errors, and quarantine suspect files
  in `fix_db`.
- Report the daemon volume to MPRIS clients, and wire the initial volume into
  the sink and the service.
- Drain async player errors in the audio thread, and stop the legacy MPRIS
  callback from panicking.
- Retry the `ytmc` cover fetch after a failed fetch.
- Map daemon client errors to MCP invalid-params errors.
- Warn on a non-loopback bind, and validate the bind value as an IP address.

## 0.5.0 — 2026-11-16

### Added

- Add a full-window player with large controls and album art. Open it from the
  now-playing bar or the library dashboard.
- Show the current track album art on the library dashboard.

### Fixed

- Send live SSE events in the API event envelope. The web UI now receives
  playback, queue and download events.
- Fill the track duration from the media file when playback starts. The seek
  bar now shows the real duration and accepts seeks.

## 0.4.1 — 2026-11-15

### Fixed

- Install `ytmc` with the home-manager module. Set `YTERMUSICD_URL` to the
  configured bind address and port.

## 0.4.0 — 2026-11-15

### Changed

- Rename the terminal client from `ytermusic-tui` to `ytmc`. The crate, binary,
  Nix package, app and release archives use the new name. `ytc` collides with
  an active YouTube TUI client.

## 0.3.0 — 2026-11-15

### Added

- Add `ytermusic-tui`, a terminal client for the daemon. It shows the library,
  playlists, queue, search, now playing and cover art. It supports vim keys and
  mouse actions.
- Add `ytermusicd-client` for the REST and SSE API. The MCP proxy and the
  terminal client use this shared client.
- Add a Nix package and app for `ytermusic-tui`.
- Add a search palette with debounce, playlist editing, context menus and
  confirmations to the terminal client.

## 0.2.0 — 2026-11-15

### Added

- Cache YouTube cover art and serve it at `/api/tracks/{video_id}/cover`.
  The web UI shows thumbnails, and MPRIS sends `mpris:artUrl`.
- Add `cover_cache_mb` to limit the cover cache. Default: 100 MiB.
- Add playlist create, rename and delete support.
- Add playlist track add, remove and move support.
- Add an add-to-playlist mode to the search palette.
- Add playlist edit tools to the MCP proxy.
- Report auth expiry and rate limits during playlist edits.

## 0.1.3 — 2026-11-14

### Added

- Add a `terminal` now-playing layout based on pv-web-music. Use the
  `terminal`/`bar` button to swap layouts. The choice persists in
  `localStorage`.

### Fixed

- Return audio seek errors to the API and the web UI.
- Keep the seek slider stable during a drag and restore the old position when
  a seek fails.

## 0.1.2 — 2026-11-14

### Fixed

- Report MPRIS attach failures. The daemon no longer logs “enabled” when the
  D-Bus name or connection failed.
- Retry audio startup when the output device is not ready. Play errors return
  to the API and the web UI.
- Use the browser `EventSource` when no test factory is set. The web UI now
  shows the daemon as online.

### Changed

- Match the https://suorcd.us Cyberpunk Neon theme: navy background, magenta
  accents, cyan dashed borders and frosted blue panels.

## 0.1.1 — 2026-11-14

### Fixed

- Use the browser `EventSource` when no test factory is set. The web UI no
  longer stays on the connection banner when the daemon is up.

## 0.1.0 — 2026-11-13

First public release.

### Added

- Add the `ytermusicd` daemon skeleton with `serve` and `mcp` subcommands.
- Add `GET /api/status` with configurable bind/port and graceful shutdown.
- Include auth, playback and queue snapshots in `/api/status`.
- Add `ytermusicd-core` for paths, config, header validation and auth status.
- Add the headless queue/playback state machine, mock audio sink and typed event bus.
- Add shared API DTOs and generated TypeScript bindings in `ytermusicd-api`.
- Persist queue, playback position, volume and shuffle/repeat state.
- Harden downloads with atomic files, parsed progress, retries, cancellation-safe state and a status summary.
- Add an optional `config.toml` and `--headers`/`--config` startup options.
- Add a Nix flake with a reproducible development shell.
- Add the local dev/test workflow (`just`, offline tests, smoke script).
- Add `AGENTS.md`, `CONTRIBUTING.md` and architecture docs.
- Add a systemd user unit and deployment documentation.
- Add a Nix package, NixOS module and home-manager module.
- Add the full REST API, SSE replay and download retry endpoint.
- Wire downloads, persistence and live playback progress into the daemon.
- Add the Svelte 5 web UI with library, search, queue, now playing, vim keys,
  mouse actions, error banners and download retries.
- Add Linux MPRIS media controls.
- Add the `ytermusicd mcp` stdio proxy with 13 tools.
- Add CI, the release workflow with checksums, and offline test suites.

### Known limitations

- Playlist editing is post-v1. See
  [#51](https://github.com/suorcd/ytermusicd/issues/51).
- Lyrics and MCP HTTP transport are post-v1.
- The web UI needs a built bundle inside the daemon package.
- The daemon API trusts local clients. Use a reverse proxy for remote access.

Release history inherited from the upstream project is preserved in
[docs/upstream-changelog.md](docs/upstream-changelog.md).
