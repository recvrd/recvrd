# Terminal client (`recvc`)

`recvc` is a terminal client for the daemon. It is a thin client: the
daemon owns playback, queue, downloads and auth. The client uses the REST and
SSE API, so it shows the same state as the web UI, MPRIS and the MCP proxy.

## Requirements

- A running `recvrd` daemon.
- A terminal of at least 80x24 for the full layout. The client hides the sidebar
  below 60 columns.
- A terminal with mouse reporting for mouse actions. Keyboard actions always
  work.

## Run the client

```sh
nix run .#recvc
```

From the repository:

```sh
nix develop
cargo run -p recvc
# or
just tui
```

The default URL is `http://127.0.0.1:8080`. Use `--url` or
`RECVRD_URL` for another daemon:

```sh
recvc --url http://127.0.0.1:9000
RECVRD_URL=http://127.0.0.1:9000 recvc
```

The client does not read `headers.txt`. It shows an auth banner when the daemon
reports an auth problem.

## Views

- **Library** shows the playlists. Press `Enter` to open one. Press `p` to play
  one.
- **Playlist** shows the tracks. Press `Enter` to play the selected track.
  Press `a` to add it to another playlist. Press `J` or `K` to move it.
- **Queue** shows the current queue and download state. Press `Enter` to play
  the selected track. Press `J` or `K` to reorder it.
- **Search** opens with `/` or `Ctrl+K`. Type a query. The client searches
  after a short pause. Press `Enter` to search now. Press `Enter` again to play
  or open the selected result.
- **Now playing** shows cover art, title, artist, progress and controls.

## Keys

| Key | Action |
| --- | --- |
| `j` / `Down` | Move down |
| `k` / `Up` | Move up |
| `gg` / `G` | First / last item |
| `Ctrl+D` / `Ctrl+U` | Half page down / up |
| `Tab` | Switch focus between the sidebar and the main view |
| `Enter` | Play or open the selected item |
| `Space` | Play or pause |
| `x` | Add the selected track to the queue |
| `a` | Add the selected track to a playlist |
| `dd` | Delete the selected playlist, track or queue item |
| `D` | Clear the queue (queue view) |
| `t` | Retry the selected download (queue view) |
| `J` / `K` | Move the selected playlist track or queue item |
| `n` / `r` | Create / rename a playlist |
| `p` | Play the selected or open playlist |
| `s` / `R` | Toggle shuffle / cycle repeat |
| `o` / `O` | Switch provider / toggle offline mode |
| `+` / `-` | Change the volume by 5 |
| `Left` / `Right` | Seek back or forward 5 seconds |
| `Ctrl+Left` / `Ctrl+Right` | Previous / next track |
| `/` or `Ctrl+K` | Open search |
| `Ctrl+R` | Reload the library and the open playlist |
| `1` / `2` | Open the library / queue |
| `?` | Open help |
| `Esc` | Close an overlay or return to the library |
| `q` / `Ctrl+C` | Quit |

In the search palette, `Ctrl+X` adds the selected track to the queue. `Ctrl+A`
adds it to a playlist. `Up` and `Down` move the selection.

`dd` and the destructive context menu actions ask for confirmation. Press `y`
to confirm or `n` to cancel.

## Mouse

- Click a sidebar entry to open it.
- Click a row to select it. Double click a playlist or track to open or play it.
- Click a search result to open or play it.
- Right click a row to open the context menu.
- Scroll over a pane to move its selection.
- Click the progress bar to seek.
- Click the volume bar to set the volume.
- Click the transport, shuffle and repeat buttons.
- Click a toast to dismiss it.

Some multiplexers and terminals need extra configuration for mouse reporting.
Keyboard actions remain available.

## Cover art

The client detects terminal graphics support at startup. It uses the Kitty,
Sixel or iTerm2 protocol when the terminal reports one. It falls back to
Unicode half blocks when detection fails or finds no protocol. Half blocks need
a truecolor terminal for correct colors.

The detection query can add a short delay at startup on a terminal that does
not answer it.

Use `--cover-mode halfblocks` or `RECVC_COVER_MODE=halfblocks` to force the
half-block renderer.

## Limits

- The client has no lyrics view.
- The client never downloads audio files. The daemon owns the download queue.
- The client asks the daemon for a cover when the current track changes. A
  missing cover shows a placeholder.

## Troubleshooting

**The header shows `disconnected`.** Start the daemon. Check `--url` and
`RECVRD_URL`. The client reconnects with backoff. This is different from
`offline mode`, which is a manual switch and keeps the daemon connected.

**The banner shows `Auth`.** Refresh `headers.txt` and restart the daemon. See
[Authentication](../README.md#authentication).

**The mouse does not work.** Check the terminal or multiplexer mouse mode. Use
the keys.

**The layout is narrow.** Use at least 80x24. The client removes the sidebar
below 60 columns.

**The colors are wrong.** Use a truecolor terminal. Cover art uses RGB colors.
