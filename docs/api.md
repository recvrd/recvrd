# HTTP API

`recvrd serve` exposes a JSON API plus a Server-Sent Events (SSE) stream.
All request and response bodies use the DTOs from
[`crates/recvrd-api`](../crates/recvrd-api/src/lib.rs); TypeScript
bindings are generated from the same definitions in
[`crates/recvrd-api/bindings`](../crates/recvrd-api/bindings).

## Base URL and trust model

The default address is `http://127.0.0.1:8080`. Override it with `--bind`,
`--port`, `RECVRD_BIND` or `RECVRD_PORT`. The daemon serves the web UI
at `/` and the JSON API under `/api`. Client-side routes fall back to
`index.html`; unknown `/api` paths return 404.

The API is loopback-only by default and intentionally has no authentication:
any local process that can reach the port can control playback. Do not expose
it to a network without a reverse proxy that adds authentication and TLS. The
daemon does not enable permissive CORS.

## Errors

Every non-2xx response uses one envelope:

```json
{
  "error": {
    "code": "bad_request",
    "message": "position must be >= 0"
  }
}
```

| Code | HTTP status | Meaning |
| --- | --- | --- |
| `bad_request` | 400 | Invalid or out-of-range input |
| `auth_expired` | 401 | The YouTube Music session expired |
| `not_found` | 404 | Unknown playlist or route |
| `offline_unavailable` | 409 | The request needs a provider or uncached track while offline |
| `upstream_error` | 502 | Upstream provider request failed |
| `internal_error` | 500 | Local audio/daemon failure |

`auth_expired` describes the YouTube Music session, not the local API. The
local API has no authentication. Search and playback still work with an
expired session. Library reads and playlist edits fail until you refresh
`headers.txt` and restart the daemon.

When the YouTube client is unavailable, the daemon has no live auth state.
The headers file state still appears in `GET /api/status` and
`GET /api/providers`, but a provider error returns 502 `upstream_error`
rather than guessing that a network failure is an expired session.
Only the provider's own auth error returns 401 `auth_expired`. RSS has no
auth state, so RSS failures always return 502.

A public playlist still loads and plays without a session. The daemon
reports an expired session for an empty signed-out response that has
no playlist or album header, or that shows a page-level sign-in
prompt. A session that expires during pagination ends the load with
401 `auth_expired` instead of a cut list.

`GET /api/status` itself returns 200 with an `auth.state` of `ok`, `missing`,
`invalid` or `expired`, so clients can render an actionable auth message.

## Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/status` | Daemon, auth, playback and queue summary |
| GET | `/api/providers` | Providers with capabilities, auth state and playback mode |
| PUT | `/api/providers/{provider}/playback` | Set a provider's playback mode (`{mode}`) |
| GET | `/api/offline` | Report the manual offline switch |
| PUT | `/api/offline` | Turn offline mode on or off (`{enabled}`) |
| GET | `/api/playlists?provider=...` | Library playlists |
| POST | `/api/playlists` | Create a playlist |
| GET | `/api/playlists/{id}?provider=...` | Playlist metadata and tracks |
| PATCH | `/api/playlists/{id}` | Rename a playlist |
| DELETE | `/api/playlists/{id}` | Delete a playlist |
| POST | `/api/playlists/{id}/tracks` | Add tracks (`{provider, track_ids}`) |
| POST | `/api/playlists/{id}/tracks/remove` | Remove tracks (`{indices}`) |
| POST | `/api/playlists/{id}/tracks/move` | Move a track (`{from,to}`) |
| GET | `/api/search?provider=...&q=...` | Track and playlist search |
| GET | `/api/tracks/{provider}/{track_id}` | Full track detail and description |
| GET | `/api/tracks/{provider}/{track_id}/cover` | Cached cover art |
| PUT | `/api/tracks/{provider}/{track_id}/like` | Set the track like state (`{liked}`) |
| GET | `/api/queue` | Queue snapshot |
| POST | `/api/queue` | Add a track (`{track, position}`; position is `next` or `last`, default `last`; alias: `/api/queue/add`) |
| DELETE | `/api/queue` | Clear the queue |
| POST | `/api/queue/reorder` | Move an item (`{from,to}`) |
| POST | `/api/queue/remove` | Remove an item (`{index}`) |
| POST | `/api/queue/play-next` | Move an item to play next (`{index}`) |
| GET | `/api/history` | Finished plays, newest first |
| POST | `/api/play/playlist` | Replace the queue with a playlist and play (`{playlist_id, provider}`) |
| POST | `/api/play/track` | Play a track immediately |
| POST | `/api/pause` | Pause (idempotent) |
| POST | `/api/resume` | Resume or start the current track (idempotent) |
| POST | `/api/next` | Next track, respecting repeat/shuffle |
| POST | `/api/previous` | Previous track, respecting repeat/shuffle |
| POST | `/api/volume` | Set volume 0–100 (clamped) |
| POST | `/api/seek` | Seek to `position_secs` |
| POST | `/api/shuffle` | Enable/disable shuffle |
| POST | `/api/repeat` | Set repeat mode (`off`, `one`, `all`) |
| POST | `/api/downloads/retry` | Retry a failed download (`{provider, track_id}`) |
| GET | `/api/cache` | Download and cover cache sizes |
| POST | `/api/cache/clean` | Delete cached files (`{scope}`) |
| GET | `/api/library/cached?provider=...` | Cached tracks for a provider |
| GET | `/api/library/cached/playlists?provider=...` | Cached playlists for a provider |
| GET | `/api/rss/feeds` | RSS feeds the daemon follows |
| POST | `/api/rss/feeds` | Add an RSS feed (`{url}`) |
| POST | `/api/rss/feeds/refresh` | Refresh every RSS feed |
| DELETE | `/api/rss/feeds/{id}` | Remove an RSS feed and its episodes |
| POST | `/api/rss/feeds/{id}/refresh` | Refresh one RSS feed |
| GET | `/api/events` | SSE snapshot and live events |

Command endpoints return the updated `Status` or `Queue` so clients can render
immediately; they do not need to wait for the matching SSE event.

### Status

```sh
curl http://127.0.0.1:8080/api/status
```

```json
{
  "name": "recvrd",
  "version": "0.7.0",
  "status": "ok",
  "uptime_seconds": 42,
  "auth": {
    "state": "ok",
    "source": "/home/me/.config/recvrd/headers.txt",
    "message": null
  },
  "playback": {
    "state": "stopped",
    "current": null,
    "position_secs": 0.0,
    "volume": 50,
    "repeat": "off",
    "shuffle": false
  },
  "offline": false,
  "queue_length": 0,
  "downloads": {
    "queued": 0,
    "active": 0,
    "completed": 0,
    "retrying": 0,
    "failed": 0
  },
  "providers": ["youtube", "rss"],
  "default_provider": "youtube"
}
```

### Providers

```sh
curl http://127.0.0.1:8080/api/providers
```

```json
[
  {
    "id": "youtube",
    "name": "YouTube Music",
    "default": true,
    "capabilities": {
      "search": true,
      "library": true,
      "playlists": true,
      "playlist_create": true,
      "playlist_edit": true,
      "track_like": true,
      "downloads": true,
      "covers": true
    },
    "auth": {
      "state": "ok",
      "source": "/home/me/.config/recvrd/headers.txt",
      "message": null
    },
    "playback": "download",
    "playback_modes": ["download"]
  }
]
```

`GET /api/providers` lists every configured provider. Each entry names the
provider, marks the default, lists the actions it supports, reports its auth
state and its playback mode. Clients hide actions a provider does not support.
RSS needs no auth, so its auth state is always `ok` with no source. The
Podverse provider is omitted until `podverse.base_url` or
`RECVRD_PODVERSE_URL` is set.

### Playback modes

A provider plays in one of three modes:

- `download` — wait for the cache download, then play the file.
- `stream` — play the remote source without keeping a cache.
- `stream_then_download` — play the remote source and download the cache in
  the background.

The defaults are `download` for YouTube and RSS and `stream_then_download` for
Podverse. YouTube supports only `download`. RSS and Podverse support all three.
A provider without a remote source falls back to a download in `download` and
`stream_then_download` modes. In `stream` mode, the track fails with an error.

```sh
curl -X PUT http://127.0.0.1:8080/api/providers/rss/playback \
  -H 'content-type: application/json' \
  -d '{"mode": "stream"}'
```

The response is the updated `ProviderInfo`. An unsupported mode returns 400;
an unknown or unconfigured provider returns 404. The choice is saved in
`$XDG_CONFIG_HOME/recvrd/settings.toml`. The daemon owns that file: it is
written atomically and replaced by defaults when it cannot be parsed. Keep
user-managed settings in `config.toml`.

`playback.source_kind` in `Status` is `file` or `stream` while a track plays.
It is omitted when nothing is loaded.

### Offline mode

Offline mode is a manual switch. The daemon does not probe connectivity.
While it is on, the daemon makes no provider calls:

- library and playlist requests return cached playlists only
- search matches cached tracks and playlists locally
- track detail returns a cached track only
- queue playback skips uncached tracks
- cover art uses cached files only
- playback mode and cache state stay local

```sh
curl http://127.0.0.1:8080/api/offline
curl -X PUT http://127.0.0.1:8080/api/offline \
  -H 'content-type: application/json' \
  -d '{"enabled": true}'
```

`PUT /api/offline` returns the updated `Status`. Enabling it cancels
in-flight downloads and resets their queue status. The switch is saved in
`settings.toml` and survives a daemon restart. A restart in offline mode
does not refresh RSS feeds, queue restored downloads, or create the YouTube
client. Turning the switch off creates the client without restarting the
daemon.

An action that needs a provider, or an uncached track, returns 409
`offline_unavailable`. A cached track plays normally through
`/api/play/track`. Cached list responses carry `"cached": true` on each
track.

Library and search requests take an optional `provider` parameter. A missing
value means the default provider. Providers are ordered Local files, RSS,
Podverse, then YouTube Music. The default is the first configured provider.
Local files need a music directory, and Podverse needs a base URL. RSS is
always available. An unknown name returns 400 with
`unknown provider {name}`. Upstream errors name the provider, for example
`youtube: search failed: ...`.

```sh
curl 'http://127.0.0.1:8080/api/playlists?provider=youtube'
curl 'http://127.0.0.1:8080/api/search?provider=youtube&q=civilization'
```

### RSS feeds

The RSS provider follows RSS 2.0 and Atom feeds. A feed is a playlist and
its episodes are tracks, so feeds work through the library, search, queue
and playback endpoints with `provider=rss`. Feed URLs persist in
`rss-feeds.json` in the data directory. A feed refresh uses a conditional
GET (ETag and Last-Modified) and never duplicates episodes: an episode
keeps its track ID across refreshes.

```sh
curl http://127.0.0.1:8080/api/rss/feeds
curl -X POST http://127.0.0.1:8080/api/rss/feeds \
  -H 'content-type: application/json' \
  -d '{"url": "https://example.com/feed.xml"}'
curl -X POST http://127.0.0.1:8080/api/rss/feeds/0123456789abcdef/refresh
curl -X DELETE http://127.0.0.1:8080/api/rss/feeds/0123456789abcdef
```

An added feed returns 201 with the feed, including its episode count:

```json
{
  "id": "0123456789abcdef",
  "url": "https://example.com/feed.xml",
  "title": "Test Tones",
  "episode_count": 2,
  "last_fetched": 1763400000
}
```

A bad feed URL returns 400. A feed that cannot be fetched or parsed
returns 502. An unknown feed ID returns 404 on refresh and remove.

### Podverse

Set `podverse.base_url` in `config.toml` or `RECVRD_PODVERSE_URL` to enable
the provider. The library lists recently updated music channels. Search
returns music channels as playlists. Open a channel to list its items.

```sh
curl 'http://127.0.0.1:8080/api/search?provider=podverse&q=lofi'
curl 'http://127.0.0.1:8080/api/playlists/LJ24O9e4mv?provider=podverse'
curl http://127.0.0.1:8080/api/tracks/podverse/YvIRJrjZVe
```

A Podverse track carries a media source and provider metadata:

```json
{
  "provider": "podverse",
  "track_id": "YvIRJrjZVe",
  "title": "New Year",
  "artist": "Jon Ross",
  "album": "Rock The Halls",
  "duration": "4:03",
  "source": {
    "url": "https://cdn.example/track.mp3",
    "mime": "audio/mpeg",
    "length": 5845483,
    "seekable": true,
    "kind": "audio"
  },
  "metadata": {
    "image_url": "https://cdn.example/cover.jpg",
    "published_at": "2026-11-10T18:49:40.000Z",
    "channel_id": "2984",
    "channel_title": "Rock The Halls",
    "feed_url": "https://wavlake.com/feed/music/1",
    "website_url": "https://www.wavlake.com/album/1",
    "season_number": 1,
    "year": 2026
  }
}
```

The detail route returns the same track in a `track` object and the full item
description in `description`. Podverse plays in `stream_then_download` by
default; see [Playback modes](#playback-modes). Podverse channels are
read-only.

#### Podverse position sync

Set `podverse.sync = true` in `config.toml` or `RECVRD_PODVERSE_SYNC=true`
to sync per-track positions with the Podverse queue. Sync also needs a
token (`podverse.token` or `RECVRD_PODVERSE_TOKEN`) and a signed-in
account with an unexpired membership. The default is off.

Local storage stays the source of truth. Sync works only for Podverse
and RSS tracks. YouTube tracks never sync. Sync writes the position
on pause, seek, and stop, moves a finished track to history, and
reads the remote position when a track starts. It never blocks
playback. A failed sync leaves local state alone.

The daemon selects the active account queue. It never creates a
queue. Podverse tracks sync by item ID. RSS tracks sync by feed URL
and enclosure URL. A 401 or 403 stops sync for the rest of the run.
A 429 pauses sync until the server allows it again.

Conflict policy: the newer write wins. The daemon adopts the remote
position only when the remote write is newer than the last local
write for the track, with a 5 second tolerance for clock skew. When
the remote row has no write time, the daemon adopts it only when the
local map has no entry for the track. A remote position at the end of
a track clears the local entry. A local rewind to zero clears the
local entry and the remote position stays, because the server keeps
the maximum position. The daemon never switches tracks from a remote
read.

The deployed API version is unconfirmed. The client uses the paths
and bodies from the public Podverse API source. The live OpenAPI
document was not readable from this machine. Verify the paths
against a release before you trust the sync on a new instance.

### Library and search

```sh
curl http://127.0.0.1:8080/api/playlists
curl http://127.0.0.1:8080/api/playlists/VLPL123
curl 'http://127.0.0.1:8080/api/search?q=civilization'
```

`GET /api/playlists/{id}` returns:

```json
{
  "playlist": {
    "id": "VLPL123",
    "name": "Focus",
    "subtitle": "YouTube Music"
  },
  "tracks": [
    {
      "provider": "youtube",
      "track_id": "abc123",
      "title": "Song",
      "artist": "Artist",
      "album": "Album",
      "duration": "3:24"
    }
  ]
}
```

Search returns `{ "tracks": [...], "playlists": [...] }`. An empty `q` is
rejected with 400. Upstream failures return 502 with an actionable message.

Each track in a playlist detail, search, or track detail response carries
a `cached` flag. The flag is true when a non-empty audio file exists for
the track in the download cache. The field is omitted when false. Clients
use the flag to show a track as cached or unavailable without a check per
track. The queue does not carry the flag.

### Cover art

```sh
curl -o cover.jpg http://127.0.0.1:8080/api/tracks/youtube/abcdefghijk/cover
```

The daemon returns a JPEG with `Cache-Control: public, max-age=604800,
immutable` and an `ETag`. A request with a matching `If-None-Match` header
returns 304.

The daemon fetches the image from YouTube and stores it in
`$XDG_CACHE_HOME/recvrd/covers/youtube/<track_id>.jpg`. It tries
`maxresdefault.jpg` first and falls back to `hqdefault.jpg`. An invalid track
ID or a failed fetch returns 404 with the normal error envelope.

Set `cover_cache_mb` in `config.toml` to limit the cache size. The default is
100 MiB. A value of 0 disables the limit. This endpoint does not change the
JSON `Track` shape.

### Track likes

```sh
curl -X PUT http://127.0.0.1:8080/api/tracks/youtube/abcdefghijk/like \
  -H 'Content-Type: application/json' -d '{"liked":true}'
```

`PUT /api/tracks/{provider}/{track_id}/like` sets the like state of one
YouTube Music track. A `liked` value of true adds the track to Liked
Music. A false value removes it. The call needs a signed-in session.
The endpoint returns 401 when the session expired. Other providers
return 400 because they do not support likes. The response is
`{"ok":true}`.

A track object carries a `liked` field when the provider reports the
state. The field is omitted when the provider has no state. The web UI
shows a liked marker on those tracks.

### Cache

```sh
curl http://127.0.0.1:8080/api/cache
curl -X POST http://127.0.0.1:8080/api/cache/clean -d '{"scope": "all"}'
```

`GET /api/cache` reports the sizes of the download and cover caches from
disk, in bytes and files. `POST /api/cache/clean` deletes cached files for
one scope: `downloads`, `covers`, or `all` (the default). The response
reports the freed bytes and deleted file counts per scope.

Cleaning downloads stops active downloads first, then deletes the files and
resets the queue download statuses to not downloaded. Playlists, the queue,
the playback state, and the settings are kept. The file of the current track
is kept, and the track is queued again when its file is missing, so a
download in progress resumes and the current track keeps playing.

The download cache has a size limit from `cache_limit_mb` in `config.toml`
(default 10240 MiB, 0 disables). When the cache exceeds it, the daemon evicts
the oldest complete download pairs (audio plus sidecar) until it is back under
the limit. The current track and the current queue are kept. `.part` files
and incomplete downloads are never evicted, so an active download is safe.

The headless command does the same clean:

```sh
recvrd cache clean --scope downloads
```

When the daemon answers, the command uses the API. When the daemon does not
answer, the command cleans the files directly from disk and keeps the
current track from the saved playback state. Active downloads are not
stopped in that case, so stop the daemon first for a full clean.

### History

```sh
curl http://127.0.0.1:8080/api/history
```

`GET /api/history` lists finished plays, newest first. The daemon records
a play when a track plays for 30 seconds or more. Each entry names the
provider, track, title, artist, album, duration, start time as Unix seconds,
seconds played, and whether the track played to the end. A play that ends
early still appears, with `completed` false. The daemon keeps the last 1000
plays or 90 days in `history.jsonl` in the state directory, and the list
survives a daemon restart.

### Cached library

```sh
curl 'http://127.0.0.1:8080/api/library/cached?provider=podverse'
curl 'http://127.0.0.1:8080/api/library/cached/playlists?provider=podverse'
```

`GET /api/library/cached` lists the complete downloads for one provider.
The daemon reads the metadata sidecars below `downloads/<provider>/` and
returns the tracks whose audio file exists and is not empty. The endpoint
makes no provider call, so it works with no network. A track that is still
downloading, or whose sidecar does not parse, is not listed.

`GET /api/library/cached/playlists` groups the same tracks into playlists
by channel. A track joins the playlist of its channel ID when it has one,
otherwise the playlist of its album. A track with neither a channel nor an
album appears in the track list but in no playlist. Each playlist carries
its tracks, so one call returns the full cached library for the provider.

A track from the cached list plays through `POST /api/play/track`, like any
other track. The daemon plays the cached file.

### Playlist edits

Playlist IDs are browse IDs. They start with `VL`:

```sh
# Create a playlist. The response is 201 with the new Playlist.
curl -X POST http://127.0.0.1:8080/api/playlists \
  -H 'content-type: application/json' \
  -d '{"title":"Mix","description":"","privacy":"private"}'

# Rename a playlist. The response is the updated Playlist.
curl -X PATCH http://127.0.0.1:8080/api/playlists/VLPL123 \
  -H 'content-type: application/json' \
  -d '{"title":"New name"}'

# Delete a playlist. The response is {"ok":true}.
curl -X DELETE http://127.0.0.1:8080/api/playlists/VLPL123

# Add tracks. The response is the updated track list.
curl -X POST http://127.0.0.1:8080/api/playlists/VLPL123/tracks \
  -H 'content-type: application/json' \
  -d '{"provider":"youtube","track_ids":["abcdefghijk"]}'

# Remove tracks by index. The response is the updated track list.
curl -X POST http://127.0.0.1:8080/api/playlists/VLPL123/tracks/remove \
  -H 'content-type: application/json' \
  -d '{"indices":[0]}'

# Move a track. The response is the updated track list.
curl -X POST http://127.0.0.1:8080/api/playlists/VLPL123/tracks/move \
  -H 'content-type: application/json' \
  -d '{"from":0,"to":1}'
```

The daemon validates these values before it sends an upstream request:

- The title has 1 to 150 characters. It cannot contain `<`, `>` or control
  characters.
- The description has 5000 characters or fewer.
- Each track ID has 11 characters from `A-Z`, `a-z`, `0-9`, `-` and `_`.
- Playlist edits run through YouTube Music, so `provider` must be `youtube`.
- One request adds or removes 100 tracks or fewer.
- Indices and track IDs are unique in one request.

An out-of-range track index or move position returns 400. An expired session
returns 401 with code `auth_expired` and a message that says to refresh
`headers.txt` and restart the daemon.

The YouTube provider sends the edits to the authenticated `playlist/create`,
`browse/edit_playlist` and `playlist/delete` endpoints. Removal and reordering
need each item's `setVideoId`. The daemon fetches the playlist before each edit.

### Playback and queue

```sh
# Replace the queue and start playing
curl -X POST http://127.0.0.1:8080/api/play/playlist \
  -H 'content-type: application/json' \
  -d '{"playlist_id":"VLPL123"}'

# Play one track now
curl -X POST http://127.0.0.1:8080/api/play/track \
  -H 'content-type: application/json' \
  -d '{"track":{"provider":"youtube","track_id":"abc123","title":"Song","artist":"Artist","album":"Album","duration":"3:24"}}'

# Queue management
curl http://127.0.0.1:8080/api/queue
curl -X POST http://127.0.0.1:8080/api/queue \
  -H 'content-type: application/json' \
  -d '{"track":{"provider":"youtube","track_id":"abc123","title":"Song","artist":"Artist","album":"Album","duration":"3:24"}}'
curl -X POST http://127.0.0.1:8080/api/queue \
  -H 'content-type: application/json' \
  -d '{"track":{"provider":"youtube","track_id":"abc123","title":"Song","artist":"Artist","album":"Album","duration":"3:24"},"position":"next"}'
curl -X POST http://127.0.0.1:8080/api/queue/play-next \
  -H 'content-type: application/json' -d '{"index":4}'
curl -X POST http://127.0.0.1:8080/api/queue/reorder \
  -H 'content-type: application/json' -d '{"from":0,"to":2}'
curl -X POST http://127.0.0.1:8080/api/queue/remove \
  -H 'content-type: application/json' -d '{"index":1}'
curl -X DELETE http://127.0.0.1:8080/api/queue

# Transport and modes
curl -X POST http://127.0.0.1:8080/api/pause
curl -X POST http://127.0.0.1:8080/api/resume
curl -X POST http://127.0.0.1:8080/api/next
curl -X POST http://127.0.0.1:8080/api/previous
curl -X POST http://127.0.0.1:8080/api/volume \
  -H 'content-type: application/json' -d '{"volume":75}'
curl -X POST http://127.0.0.1:8080/api/seek \
  -H 'content-type: application/json' -d '{"position_secs":42.5}'
curl -X POST http://127.0.0.1:8080/api/shuffle \
  -H 'content-type: application/json' -d '{"enabled":true}'
curl -X POST http://127.0.0.1:8080/api/repeat \
  -H 'content-type: application/json' -d '{"repeat":"all"}'

# Retry a failed download
curl -X POST http://127.0.0.1:8080/api/downloads/retry \
  -H 'content-type: application/json' -d '{"provider":"youtube","track_id":"abc123"}'
```

Queue snapshots look like:

```json
{
  "items": [
    {
      "track": {
        "provider": "youtube",
        "track_id": "abc123",
        "title": "Song",
        "artist": "Artist",
        "album": "Album",
        "duration": "3:24"
      },
      "download": { "state": "not_downloaded" }
    }
  ],
  "current": 0,
  "repeat": "off",
  "shuffle": false
}
```

Invalid indices, negative seek positions and empty searches return 400. Volume
values above 100 are clamped to 100. Pause does nothing when the daemon is
stopped. Resume starts the current track when the daemon is stopped, at the
saved position when the daemon restored one. Switching tracks saves the
position of each track. Starting a track again resumes that position.

Playing a playlist or track, and enqueueing a track, automatically queue the
audio for download, except while offline mode is on. If the file is not
cached yet, the track stays selected but playback waits for the `download`
event with `state: "downloaded"`; it then starts automatically. Failed
downloads emit an `error` event and expose the `failed` count in
`/api/status`. `POST /api/downloads/retry` returns `202` for a track the
daemon has seen, or `404` for an unknown track ID.

## SSE: `GET /api/events`

```sh
curl -N http://127.0.0.1:8080/api/events
```

The response is `text/event-stream` with `Cache-Control: no-cache` and
`X-Accel-Buffering: no`. A fresh connection receives a named `snapshot` event
followed by incremental events:

```
event: snapshot
id: 0
data: {"id":0,"event":{"type":"snapshot","status":{...},"queue":{...}}}

id: 1
data: {"id":1,"event":{"type":"playback","playback":{...}}}

id: 2
data: {"id":2,"event":{"type":"queue","queue":{...}}}
```

Event kinds (`EventKind` in the bindings):

| `type` | Payload |
| --- | --- |
| `snapshot` | Full `Status` and `Queue` on connect |
| `playback` | Playback snapshot (state, position, volume, modes) |
| `queue` | Full queue snapshot |
| `download` | `provider`, `track_id` and download status |
| `track_ended` | `provider` and `track_id` of the finished track |
| `error` | Shared `ApiError` envelope |
| `offline` | `enabled` after the manual switch changes |
| `library_changed` | Library needs refreshing |

The daemon also sends a named `heartbeat` event every 15 seconds:

```
event: heartbeat
data: {}
```

The heartbeat carries no id. It does not change `Last-Event-ID` and does not
change replay.

### Reconnect and replay

Event ids are monotonically increasing. The daemon retains the last 256 events.
To resume, send the last seen id in the standard `Last-Event-ID` header or the
`last_event_id` query parameter:

```sh
curl -N -H 'Last-Event-ID: 2' http://127.0.0.1:8080/api/events
curl -N 'http://127.0.0.1:8080/api/events?last_event_id=2'
```

- If the requested id is still in the replay window, the daemon replays only
  newer events and does not resend the snapshot.
- If the id is unknown, older than the window, or from the future, the daemon
  sends a fresh `snapshot` (with the current newest id) so the client can
  resynchronise.
- If a client cannot keep up, the daemon drops the oldest events for that
  connection and logs a warning. The daemon then sends a fresh `snapshot` with
  the newest id, so the client resynchronises without a reconnect.

Keepalive comments are also sent every 15 seconds so proxies and idle
connections do not time out. `EventSource` does not deliver comments to
JavaScript, so clients use the `heartbeat` event to detect a live stream.

## TypeScript and other clients

Build the bindings with:

```sh
cargo test -p recvrd-api
```

The generated files in `crates/recvrd-api/bindings/` are checked by CI for
drift. The web UI and MCP proxy consume these DTOs; see
[docs/architecture.md](architecture.md).
