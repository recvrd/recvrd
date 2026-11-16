# HTTP API

`ytermusicd serve` exposes a JSON API plus a Server-Sent Events (SSE) stream.
All request and response bodies use the DTOs from
[`crates/ytermusicd-api`](../crates/ytermusicd-api/src/lib.rs); TypeScript
bindings are generated from the same definitions in
[`crates/ytermusicd-api/bindings`](../crates/ytermusicd-api/bindings).

## Base URL and trust model

The default address is `http://127.0.0.1:8080`. Override it with `--bind`,
`--port`, `YTERMUSICD_BIND` or `YTERMUSICD_PORT`. The daemon serves the web UI
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
| `not_found` | 404 | Unknown playlist or route |
| `upstream_error` | 502 | YouTube Music request failed |
| `internal_error` | 500 | Local audio/daemon failure |

`GET /api/status` itself returns 200 with an `auth.state` of `ok`, `missing` or
`invalid`, so clients can render an actionable auth message.

## Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/status` | Daemon, auth, playback and queue summary |
| GET | `/api/playlists` | Library playlists |
| POST | `/api/playlists` | Create a playlist |
| GET | `/api/playlists/{id}` | Playlist metadata and tracks |
| PATCH | `/api/playlists/{id}` | Rename a playlist |
| DELETE | `/api/playlists/{id}` | Delete a playlist |
| POST | `/api/playlists/{id}/tracks` | Add tracks (`{video_ids}`) |
| POST | `/api/playlists/{id}/tracks/remove` | Remove tracks (`{indices}`) |
| POST | `/api/playlists/{id}/tracks/move` | Move a track (`{from,to}`) |
| GET | `/api/search?q=...` | Track and playlist search |
| GET | `/api/tracks/{video_id}/cover` | Cached cover art (JPEG) |
| GET | `/api/queue` | Queue snapshot |
| POST | `/api/queue` | Enqueue a track (alias: `/api/queue/add`) |
| DELETE | `/api/queue` | Clear the queue |
| POST | `/api/queue/reorder` | Move an item (`{from,to}`) |
| POST | `/api/queue/remove` | Remove an item (`{index}`) |
| POST | `/api/queue/play-next` | Move an item to play next (`{index}`) |
| POST | `/api/play/playlist` | Replace the queue with a playlist and play |
| POST | `/api/play/track` | Play a track immediately |
| POST | `/api/pause` | Pause (idempotent) |
| POST | `/api/resume` | Resume (idempotent) |
| POST | `/api/next` | Next track, respecting repeat/shuffle |
| POST | `/api/previous` | Previous track, respecting repeat/shuffle |
| POST | `/api/volume` | Set volume 0–100 (clamped) |
| POST | `/api/seek` | Seek to `position_secs` |
| POST | `/api/shuffle` | Enable/disable shuffle |
| POST | `/api/repeat` | Set repeat mode (`off`, `one`, `all`) |
| POST | `/api/downloads/retry` | Retry a failed download (`{video_id}`) |
| GET | `/api/events` | SSE snapshot and live events |

Command endpoints return the updated `Status` or `Queue` so clients can render
immediately; they do not need to wait for the matching SSE event.

### Status

```sh
curl http://127.0.0.1:8080/api/status
```

```json
{
  "name": "ytermusicd",
  "version": "0.4.0",
  "status": "ok",
  "uptime_seconds": 42,
  "auth": {
    "state": "ok",
    "source": "/home/me/.config/ytermusicd/headers.txt",
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
  "queue_length": 0,
  "downloads": {
    "queued": 0,
    "active": 0,
    "completed": 0,
    "retrying": 0,
    "failed": 0
  }
}
```

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
      "video_id": "abc123",
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

### Cover art

```sh
curl -o cover.jpg http://127.0.0.1:8080/api/tracks/abcdefghijk/cover
```

The daemon returns a JPEG with `Cache-Control: public, max-age=604800,
immutable` and an `ETag`. A request with a matching `If-None-Match` header
returns 304.

The daemon fetches the image from YouTube and stores it in
`$XDG_CACHE_HOME/ytermusicd/covers/<video_id>.jpg`. It tries
`maxresdefault.jpg` first and falls back to `hqdefault.jpg`. An invalid video
ID or a failed fetch returns 404 with the normal error envelope.

Set `cover_cache_mb` in `config.toml` to limit the cache size. The default is
100 MiB. A value of 0 disables the limit. This endpoint does not change the
JSON `Track` shape.

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
  -d '{"video_ids":["abcdefghijk"]}'

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
- Each video ID has 11 characters from `A-Z`, `a-z`, `0-9`, `-` and `_`.
- One request adds or removes 100 tracks or fewer.
- Indices and video IDs are unique in one request.

An out-of-range track index or move position returns 400. An expired session
returns 502 with a message that asks the user to sign in again.

`ytpapi2` sends the edits to the authenticated `playlist/create`,
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
  -d '{"track":{"video_id":"abc123","title":"Song","artist":"Artist","album":"Album","duration":"3:24"}}'

# Queue management
curl http://127.0.0.1:8080/api/queue
curl -X POST http://127.0.0.1:8080/api/queue \
  -H 'content-type: application/json' \
  -d '{"track":{"video_id":"abc123","title":"Song","artist":"Artist","album":"Album","duration":"3:24"}}'
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
  -H 'content-type: application/json' -d '{"video_id":"abc123"}'
```

Queue snapshots look like:

```json
{
  "items": [
    {
      "track": {
        "video_id": "abc123",
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
values above 100 are clamped to 100. Pause and resume are idempotent when the
daemon is stopped.

Playing a playlist or track, and enqueueing a track, automatically queue the
audio for download. If the file is not cached yet, the track stays selected but
playback waits for the `download` event with `state: "downloaded"`; it then
starts automatically. Failed downloads emit an `error` event and expose the
`failed` count in `/api/status`. `POST /api/downloads/retry` returns `202`
for a track the daemon has seen, or `404` for an unknown video id.

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
| `download` | `video_id` and download status |
| `track_ended` | `video_id` of the finished track |
| `error` | Shared `ApiError` envelope |
| `library_changed` | Library needs refreshing |

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
- A client that cannot keep up may lag; the daemon drops the oldest events for
  that connection and logs a warning. Reconnecting with `Last-Event-ID` (or
  accepting the new snapshot) resynchronises.

Keepalive comments are sent every 15 seconds so proxies and idle connections do
not time out.

## TypeScript and other clients

Build the bindings with:

```sh
cargo test -p ytermusicd-api
```

The generated files in `crates/ytermusicd-api/bindings/` are checked by CI for
drift. The web UI and MCP proxy consume these DTOs; see
[docs/architecture.md](architecture.md).
