# MCP proxy

`recvrd mcp` is a stdio MCP proxy. It connects to a running daemon and
exposes the tool set. It holds no playback state.

```sh
recvrd mcp
recvrd mcp --url http://127.0.0.1:9000
RECVRD_URL=http://127.0.0.1:9000 recvrd mcp
```

The default URL is `http://127.0.0.1:8080`. The proxy uses the same REST API as
the web UI. Every request has a 30 second timeout. The proxy exits when stdin
closes.

## Client configuration

Claude Desktop and other MCP clients use a JSON server entry:

```json
{
  "mcpServers": {
    "recvrd": {
      "command": "recvrd",
      "args": ["mcp"],
      "env": {
        "RECVRD_URL": "http://127.0.0.1:8080"
      }
    }
  }
}
```

For the MCP inspector:

```sh
npx @modelcontextprotocol/inspector recvrd mcp
```

For clients that accept a command tuple:

```sh
your-mcp-client --server "recvrd mcp"
```

## Tools

| Tool | Arguments | Result |
| --- | --- | --- |
| `list_providers` | none | Providers with capabilities and auth state |
| `search` | `query`, `provider` (default `youtube`) | Tracks and playlists |
| `list_playlists` | `provider` (default `youtube`) | Library playlists |
| `get_playlist` | `playlist_id`, `provider` (default `youtube`) | Playlist detail and tracks |
| `create_playlist` | `title`, `description`, `privacy` | New playlist |
| `rename_playlist` | `playlist_id`, `title` | Updated playlist |
| `delete_playlist` | `playlist_id` | `{ok: true}` |
| `add_playlist_tracks` | `playlist_id`, `provider`, `track_ids` | Updated track list |
| `remove_playlist_tracks` | `playlist_id`, `indices` | Updated track list |
| `move_playlist_track` | `playlist_id`, `from`, `to` | Updated track list |
| `play_playlist` | `playlist_id`, `provider` (default `youtube`) | Updated status |
| `play_track` | `track` | Updated status |
| `queue_add` | `track` | Updated queue |
| `now_playing` | none | Status with current playback |
| `pause` | none | Updated status |
| `resume` | none | Updated status |
| `next` | none | Updated status |
| `previous` | none | Updated status |
| `volume` | `volume` (0-100) | Updated status |
| `seek` | `position_secs` | Updated status |
| `shuffle` | `enabled` | Updated status |
| `repeat` | `repeat` (off, one or all) | Updated status |
| `queue_remove` | `index` | Updated queue |
| `queue_reorder` | `from`, `to` | Updated queue |
| `queue_play_next` | `index` | Updated queue |
| `queue_clear` | none | Updated queue |
| `retry_download` | `provider`, `track_id` | `{ok: true}` |
| `download_status` | `provider`, `track_id` (optional) | Download summary and queue tracks |
| `rss_list_feeds` | none | RSS feeds the daemon follows |
| `rss_add_feed` | `url` | Added feed with episode count |
| `rss_remove_feed` | `feed_id` | `{ok: true}` |
| `rss_refresh_feed` | `feed_id` | Refreshed feed |
| `rss_refresh_feeds` | none | Refreshed feeds |

`track` is the object from `search` or `get_playlist`:
`provider`, `track_id`, `title`, `artist`, `album` and `duration`.

Tool results include `structuredContent` with a JSON schema. The schema comes
from the shared `recvrd-api` DTOs.

## Errors

The proxy maps daemon errors to MCP errors. The daemon code travels in the
error data as `{"code": "<daemon code>"}`.

- A bad request or an unknown playlist or video ID returns an
  invalid-parameter error. The daemon message stays in the error message.
- An expired session returns an invalid-parameter error with code
  `auth_expired`. Refresh `headers.txt` and restart the daemon.
- An upstream failure returns an internal error. The daemon message and the
  daemon code stay in the error message and in the error data.
- A stopped daemon returns an internal error with `Cannot reach the daemon at
  <url>. Start it with: recvrd serve`.
- A bad volume returns an invalid-parameter error before the request.

The proxy does not crash on daemon errors. The client shows the message.

## Scope

The proxy has the 33 tools above. These items are post-v1:

- lyrics;
- streamable HTTP MCP transport;
- resources and prompts.

## Tests

```sh
cargo test -p recvrd mcp::
cargo test -p recvrd --test mcp_stdio
```

The unit tests use an in-process mock daemon. The stdio test starts the real
binary, performs the MCP handshake, lists 33 tools, calls `now_playing` and
checks that the process exits when stdin closes. The playlist tool tests cover
the create, rename, delete, add, remove and move calls. The new tool tests
cover seek, shuffle, repeat, queue remove, queue reorder, queue play next,
queue clear and retry download.
