# Changelog

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
