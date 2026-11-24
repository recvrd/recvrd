# 0005 — Terminal client is a daemon client

**Status:** Superseded by #209

**Date:** 2026-11-15

## Context

The upstream TUI in `crates/ytermusic` is a standalone client. It owns the
queue, playback sink, downloads and auth in its own process. The daemon, web UI
and MPRIS use a different state. A rename of the legacy TUI would not make the
two states agree.

The project name is `recvrd`. Short names such as `ytc` and `recvc` collide
with other YouTube and YouTube Music tools. `ytc` is an active YouTube TUI
client. `recvc` is used for a converter app and a music-control plugin.

## Decision

- Add `crates/recvc` with the binary `recvc`.
- The terminal client uses only the REST and SSE API. It does not read cookies,
  the database or the player.
- Share the HTTP and SSE client with the MCP proxy in
  `crates/recvrd-client`.
- Keep the legacy `ytermusic` binary unchanged. Remove it only after the new
  client reaches parity and a release ships.
- The user requested `ytc` or `recvc`. `ytc` has a direct collision. The
  project uses `recvc` and keeps `ytmc` as an alias for one release. v0.3.0 used
  `ytermusic-tui`; v0.4.0 renamed it to `ytmc`.

## Consequences

- All clients show one queue, one playback state and one download state.
- The new client needs the daemon to run.
- The workspace gains two crates and one shared client layer.
- The legacy TUI stays available during the transition.
- The new client uses half-block cover art. Terminal graphics protocols remain
  a later change.
