# 0006 — Playback modes and the settings file

**Status:** Accepted

**Date:** 2026-11-22

## Context

The daemon started as download-first. The Podverse provider serves remote
enclosures, so clients need to start playback before the cache download
finishes. The mode belongs to the provider, not the track: YouTube has no
streaming backend, while RSS and Podverse support both.

## Decision

- Add `PlaybackMode { Download, Stream, StreamThenDownload }` per provider.
- The defaults are:
  - YouTube: `Download`, and it is the only supported mode.
  - RSS: `Download`, with all three modes supported.
  - Podverse: `StreamThenDownload`, with all three modes supported.
- Store runtime choices in `settings.toml` under the config directory.
  The file is auto-generated, written atomically, and replaced by defaults
  when it cannot be parsed. `config.toml` stays user-managed.
- Report the current mode and supported modes in `ProviderInfo`.
  Change the mode with `PUT /api/providers/{id}/playback`.
- Play a cache file when one exists. Otherwise:
  - `Download` waits for the download manager.
  - `Stream` opens the source and keeps no cache.
  - `StreamThenDownload` opens the source and queues the cache download.
- On a stream failure, `StreamThenDownload` falls back to the download
  queue. `Stream` does not fall back.
- The stream reader is opened on the daemon runtime. The audio thread
  receives the open reader, so it performs no network I/O.
- Keep the provider duration when it is known. Use the decoder duration
  only when the provider duration is empty.

## Consequences

- The daemon needs the `stream-download` crate for range reads.
- `stream_then_download` fetches the URL twice in the first version.
  A shared cache is a later optimization.
- A stream cannot switch to a cache file mid-track. The cache is used on
  the next play.
- Clients can hide the mode control when `playback_modes` has one entry.
