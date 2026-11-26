# Deployment

## Nix package

```sh
nix build .#recvrd
nix profile install .#recvrd
nix run .#recvrd -- serve
```

The package wraps the binary with `yt-dlp` and `ffmpeg` on `PATH` and installs
the systemd user unit under `$out/lib/systemd/user/`. The package builds the web
UI and embeds it in the binary.

## Reverse proxy and TLS

The daemon API trusts local clients. Bind it to `127.0.0.1` and use a reverse
proxy for remote access. The proxy must add authentication and TLS. The daemon
logs a warning at startup when the bind address is not loopback. Config
validation rejects a bind value that is not an IP address.

Caddy example:

```caddy
music.example.com {
  reverse_proxy 127.0.0.1:8080
  basicauth {
    user $2a$14$replace-with-a-real-hash
  }
}
```

nginx example:

```nginx
location / {
  proxy_pass http://127.0.0.1:8080;
  proxy_http_version 1.1;
  proxy_set_header Host $host;
  proxy_buffering off;
}
```

`proxy_buffering off` keeps the SSE stream live. The daemon sends
`X-Accel-Buffering: no` for the same reason. Do not expose the daemon port
without a proxy.

## NixOS module

The flake exposes `nixosModules.default` for headless/server deployments:

```nix
{
  inputs.recvrd.url = "github:recvrd/recvrd";

  outputs = { self, nixpkgs, recvrd, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        recvrd.nixosModules.default
        {
          services.recvrd = {
            enable = true;
            bind = "127.0.0.1";
            port = 8080;
            environmentFile = "/run/secrets/recvrd.env";
          };
        }
      ];
    };
  };
}
```

The system service runs as the `recvrd` system user with `HOME` under
`/var/lib/recvrd`; put `headers.txt` in
`/var/lib/recvrd/.config/recvrd/headers.txt` or point
`RECVRD_HEADERS` at it via `environmentFile`. The module moves the contents of
`/var/lib/ytermusicd` to `/var/lib/recvrd` on the first start. The old
`ytermusicd.service` name is an alias for one release. For desktop/MPRIS use
the home-manager or systemd user unit instead, so the daemon runs inside the
user session.

## home-manager module

```nix
{
  imports = [ inputs.recvrd.homeManagerModules.default ];
  services.recvrd = {
    enable = true;
    environmentFile = "/home/you/.config/recvrd/env";
  };
}
```

The module installs `recvrd` and `recvc` on `PATH`. It sets
`RECVRD_URL` to the configured bind address and port, so `recvc` connects
without extra flags.

## systemd user service

The repository ships a user unit at
[`packaging/systemd/recvrd.service`](../packaging/systemd/recvrd.service).

```sh
# Install the binary somewhere on PATH (Nix profile shown here)
nix profile install .#recvrd

mkdir -p ~/.config/systemd/user
cp packaging/systemd/recvrd.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now recvrd

systemctl --user status recvrd
journalctl --user -u recvrd -f
```

The unit runs `recvrd serve` from a controlled `PATH` that includes
`~/.nix-profile/bin`, `/run/current-system/sw/bin` (NixOS) and the usual system
paths. Install the binary in one of those locations, or override `PATH` in the
environment file.

### Environment

Copy the example environment file and uncomment options:

```sh
cp packaging/systemd/recvrd.env.example ~/.config/recvrd/env
$EDITOR ~/.config/recvrd/env
```

The unit creates `~/.config/recvrd`, `~/.cache/recvrd` and
`~/.local/state/recvrd` via systemd's `ConfigurationDirectory`,
`CacheDirectory` and `StateDirectory`. On the first start it moves the old
`ytermusicd` directories to these paths. The old `ytermusicd.service` name is
an alias for one release.

Authentication defaults to `~/.config/recvrd/headers.txt` (with a legacy
`~/.config/ytermusic/headers.txt` fallback). See the README for how to create
`headers.txt`; never commit it.

To enable the Podverse provider, set `RECVRD_PODVERSE_URL` to the API base and
an optional `RECVRD_PODVERSE_TOKEN`. Do not put the token in a world-readable
file.

Playback modes are chosen at runtime with
`PUT /api/providers/{provider}/playback` and saved in
`~/.config/recvrd/settings.toml`. The daemon owns that file: it rewrites it
atomically and quarantines a corrupt file as `settings.toml.bad`. The defaults
are `download` for YouTube and RSS and `stream_then_download` for Podverse.
RSS and Podverse can also use `stream` to play without caching.

### Smoke tests

```sh
curl http://127.0.0.1:8080/api/status
curl http://127.0.0.1:8080/ | grep 'id="app"'
playerctl -l
```

`playerctl` lists `recvrd` while the daemon runs.

`cache_limit_mb` caps the download cache (default 10240 MiB, 0 disables).
When it is exceeded, the oldest complete downloads are evicted, keeping the
current queue. `.part` files and active downloads are never removed.

### Troubleshooting

- **No audio**: the service runs in the user session. Make sure the user has
  access to the PipeWire session. recvrd links to PipeWire on Linux and does
  not use direct ALSA access. Install the `libpipewire-0.3-0` package on
  Debian and Ubuntu. On NixOS, use the Home Manager module or another user
  unit for audio. The system-level `services.recvrd` unit runs as a different
  user and cannot connect to the desktop PipeWire session.
- **DBus/MPRIS**: the user manager must have `DBUS_SESSION_BUS_ADDRESS` set;
  this is normal for `systemctl --user` sessions.
- **Service exits at startup**: run `recvrd serve` in a terminal with the
  same environment to see the actionable auth/config error.
- **Port conflict**: set `RECVRD_PORT` in the environment file or change
  `--port` in `ExecStart`.

## Docker

The Nix flake stays the primary build. Docker serves users without Nix.

### Deploy

Copy `.env.example` to `.env`. Copy `recvrd.env.example` to `recvrd.env` when you need secrets. Put `headers.txt` in `./config`. Set `MUSIC_DIR` in `.env` to your music library. Run `docker compose up --build -d`. Open `http://127.0.0.1:8080`. Check the status with `curl http://127.0.0.1:8080/api/status`.

Compose publishes the port on loopback only. The daemon binds `0.0.0.0` inside the container. The daemon trusts local clients. Use a reverse proxy with authentication for remote access. See the reverse proxy section above.

Compose stores daemon state in named volumes. The config directory is a bind mount at `./config`. The music directory is a read-only bind mount. The daemon runs as your host user and group. Set `UID` and `GID` in `.env` to match them.

### Develop

Run `docker compose -f compose.yaml -f compose.dev.yaml up --build`. The daemon runs from the source mount with `cargo run`. The web service runs the Vite dev server on port 5173. The web service shares the daemon network. The Vite proxy sends `/api` to the daemon. Open `http://127.0.0.1:5173` for the dev UI. Restart the daemon container to reload Rust changes.

### Local audio

The deploy compose file mounts no audio socket. The daemon still serves the web UI and the API without audio. Playback needs the host PipeWire socket. Add an override file with the PipeWire and D-Bus mounts from `compose.dev.yaml`. Set `DBUS_SESSION_BUS_ADDRESS` to the bus path in that file. MPRIS logs a warning without D-Bus. The daemon continues without it.
