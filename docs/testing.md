# Testing and local development

This project uses a Nix dev shell for reproducible tooling. If Nix is not
available, install the tools listed in the README manually.

```sh
nix develop
```

## Common commands

| Command | What it does |
| --- | --- |
| `just check` | `cargo fmt --check`, clippy with `-D warnings`, and `cargo check` |
| `just test` | Unit tests that need no credentials, network, audio device or DBus |
| `just test-all` | Includes ignored tests that need real YouTube Music credentials/network |
| `just fmt` | Format the workspace |
| `just clippy` | Clippy only |
| `just dev` | Run the daemon with the default bind/port |
| `just tui` | Run the terminal client against the default daemon URL |
| `just web-install` | Install web UI dependencies (`npm ci`) |
| `just web` | Run the Vite dev server with an `/api` proxy |
| `just web-check` | Type-check and build the web UI |
| `just smoke` | Smoke-test a running daemon (`scripts/smoke.sh`) |
| `just smoke-local` | Start a temporary daemon and smoke-test it (`scripts/smoke-local.sh`) |

`just` is provided by the Nix dev shell and listed in the README prerequisites
for non-Nix setups.

## Test layers

1. **Unit tests** live next to the code (`#[cfg(test)]`). They cover parsing,
   string helpers, queue/state transitions, persistence and other pure logic.
2. **Offline integration tests** use fixtures and mock implementations for the
   YouTube client, audio sink, downloader and database. They must not require
   cookies, a network connection, an audio device or DBus.
3. **Credentialed tests** are marked `#[ignore]`. The `ytpapi2` tests are the
   current example: they read a real `headers.txt` and call YouTube Music.
4. **Web tests** use Vitest and Testing Library in `crates/recvrd-web`.
5. **Terminal client tests** use `ratatui::backend::TestBackend`. They render at
   80x24 and 120x40, and they need no TTY or network.
6. **Smoke tests** run against a real daemon and are scripted in
   `scripts/smoke.sh`.

### Credentialed tests

By default, credentialed tests are skipped:

```sh
cargo test --workspace
```

Run them explicitly with a headers file:

```sh
RECVRD_HEADERS="$HOME/.config/recvrd/headers.txt" \
  cargo test -p ytpapi2 -- --include-ignored
```

Never commit `headers.txt`, `account_id.txt` or derived cookies. The repository
root `headers.txt` is already gitignored.

### Fixtures and mocks

Current offline coverage is in `crates/ytpapi2`, `crates/provider-youtube`,
`crates/provider-rss`, `crates/provider-podverse`, `crates/recvrd-core`,
`crates/recvrd` and `crates/recvrd-web`:

- `string_utils` tests cover cookie/header parsing helpers.
- `json_extractor` tests cover search, playlist, continuation, album and
  nested video-ID parsing with inline JSON fixtures. They also cover playlist
  item `setVideoId` parsing.
- `ytpapi2` tests cover the playlist edit request bodies, ID normalization and
  auth/rate-limit response mapping.
- `provider-youtube` tests cover the DTO conversions, playlist error mapping,
  cover URLs and the yt-dlp invocation with a fake script.
- `provider-rss` tests cover feed parsing, the store, downloads and covers.
- `provider-podverse` tests use JSON fixtures and a local HTTP server for
  search, channel items, item detail, cover art and downloads. They do not
  call a live instance.
- `recvrd-core` tests cover auth/config/paths, queue transitions,
  repeat/shuffle restoration, download-failure handling, playback position,
  streaming mode selection, stream-failure fallback, settings file loading and
  event-bus replay using a mock audio sink.
- `recvrd` integration tests start the router on an ephemeral port and
  cover every REST endpoint, playlist edit validation, SSE replay, cover art
  caching, download retry, playback-mode changes, stream-mode download
  queueing, static serving and upstream error envelopes.
- Web tests cover the API client, event stream, store, keymap, track table,
  queue, now playing, search, playlist editing, feedback banners and the app
  shell. The track table and now-playing tests check cover URLs and the
  missing-image fallback.

The test harness provides:

- a mock YouTube client and no-op download control for API tests;
- a mock audio sink (no device required);
- an in-memory event bus and a temp directory for persistence tests;
- an API/SSE test client;
- Vitest component tests with a fake EventSource.

## Regression-test policy

Every bug fix or behavior change must include a test that demonstrates the gap:

1. Add the test first and confirm it **fails on the current code**.
2. Implement the fix and confirm the test **passes**.
3. Keep the test as a regression guard.

If a deterministic test is impractical (real audio device, DBus/MPRIS, live
YouTube network), document the manual reproduction steps and add the closest
automated smoke test instead. Pull requests without this evidence are
incomplete.

Playlist mutations change the user's YouTube Music account. Use a disposable
playlist for manual checks:

1. Create a playlist in the web UI.
2. Add one track from the *Add tracks* search.
3. Move the track up or down, then remove it.
4. Rename the playlist.
5. Delete the playlist.
6. Repeat with invalid input. The API must return 400.
7. Expire or corrupt the cookie in a test account and check the auth message.

Never commit the test account's `headers.txt` or `account_id.txt`.

## CI

Each pull request to `main` runs `.github/workflows/ci.yml`. You can also run
it with `workflow_dispatch`. The `rust` job runs:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo check --workspace --all-targets`
- `cargo test --workspace`

The `web` job installs Node 22, runs `npm ci`, `npm run check`, `npm test` and
`npm run build`. The release workflow builds the web UI before the Rust binary,
embeds the assets, publishes SHA-256 checksums, and can be dry-run with
`workflow_dispatch`.

## Smoke tests

Start the daemon:

```sh
cargo run -p recvrd -- serve
```

Then, in another shell:

```sh
just smoke
# or
RECVRD_URL=http://127.0.0.1:8080 ./scripts/smoke.sh
```

The script checks `/api/status`, search, at least one SSE event, and MPRIS via
`playerctl` when available. The daemon must have valid headers for the search
check; without them it returns 502 and the smoke test fails.

For a complete local smoke test, use a temporary daemon and real headers:

```sh
RECVRD_HEADERS="$HOME/.config/recvrd/headers.txt" just smoke-local
```

`smoke-local` builds the binary, starts it on an ephemeral port with temporary
XDG directories, then checks status, the web page, search, SSE and the MCP
`tools/list` response. It stops the daemon and deletes the temporary files.

## Current gaps

The daemon, REST/SSE API, web UI, MPRIS, MCP, playlist editing and systemd/Nix
packaging exist. Live playlist mutation tests need a test account. See
[#51](https://github.com/recvrd/recvrd/issues/51) and
[decision 0005](decisions/0005-playlist-editing.md).
