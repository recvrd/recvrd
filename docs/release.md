# Release checklist

The release version is `0.8.0`. The tag is `v0.8.0`.

## Before the tag

1. Start from a clean `main` checkout.
2. Run the checks:

   ```sh
   nix develop
   just web-install
   just ci
   ```

   `just web-install` is needed one time on a new checkout.

3. Build the package:

   ```sh
   nix build .#recvrd
   ```

4. Set a real headers file and a free port. Run the local smoke test:

   ```sh
   RECVRD_HEADERS="$HOME/.config/recvrd/headers.txt" \
     just smoke-local
   ```

5. Start the daemon and check each item:

   ```sh
   curl http://127.0.0.1:8080/api/status
   curl 'http://127.0.0.1:8080/api/search?q=civilization'
   curl -N --max-time 2 http://127.0.0.1:8080/api/events
   playerctl -l
   ```

6. Open `http://127.0.0.1:8080` and check library, playlist, play, queue,
   search, `?` help and the connection banner. With `local.music_dir` set,
   check the Local files provider, the full player and the visualizer.
7. With a Podverse provider configured, set `stream` mode and play a track as
   the first track. Seek several times. The bar must move and no earlier file
   track may replay. Then set `stream_then_download`, play another track and
   let the cache download finish. Restart the daemon and confirm the cached
   track starts from the file (`playback.source_kind` is `file`).
8. Turn offline mode on. Confirm the library and search show cached content
   only, uncached rows are dimmed, a cached cover still renders, and download
   retry/RSS refresh return `offline_unavailable`. The Local files provider
   must keep its library, playlist, search and playback. Toggle offline off
   and confirm providers work again without a restart. Restart once more
   while offline and confirm the daemon makes no network provider calls at
   startup.
9. Check that the old `ytermusicd` and `ytmc` names still work, and that
   `recvrd --version` and `recvc --version` report `0.8.0`.
10. Check the MCP proxy:

    ```sh
    npx @modelcontextprotocol/inspector recvrd mcp
    ```

11. Check the systemd unit:

    ```sh
    systemctl --user restart recvrd
    systemctl --user status recvrd
    ```

12. Check the Docker image:

    ```sh
    docker build --target runtime -t recvrd:release .
    docker run --rm --entrypoint /usr/local/bin/recvrd recvrd:release --version
    ```

13. Confirm `CHANGELOG.md` and all versions name `0.8.0`, then open a pull
    request for the release commit. Merge it after every CI check passes.

## Tag and publish

```sh
git tag -a v0.8.0 -m "recvrd v0.8.0"
git push origin v0.8.0
```

Create a GitHub release from the tag. The release workflow builds:

- `recvrd-v0.8.0-x86_64-unknown-linux-gnu.tar.gz`
- `recvrd-v0.8.0-aarch64-apple-darwin.tar.gz`
- `recvrd-v0.8.0-x86_64-apple-darwin.tar.gz`
- `recvrd-v0.8.0-x86_64-pc-windows-msvc.zip`
- a SHA-256 file for each archive

Each archive contains the `recvrd` daemon, the `recvc` terminal client and the
`ytermusicd` and `ytmc` compatibility aliases.

## After the tag

1. Download one archive and run `sha256sum -c` or `shasum -a 256 -c`.
2. Extract the archive and run `recvrd --version` and
   `recvc --version`.
3. Run `recvrd serve` and open the web UI.
4. Check that the archive contains the embedded web UI:
   `curl http://127.0.0.1:8080/ | grep 'id="app"'`.
5. Close the roadmap issue when all checks pass.

## Rollback

Delete the release and the tag. Revert the version commit. Do not reuse the tag
for a different build.
