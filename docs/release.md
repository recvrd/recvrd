# Release checklist

The release version is `0.6.3`. The tag is `v0.6.3`.

## Before the tag

1. Start from a clean `main` checkout.
2. Run the checks:

   ```sh
   nix develop
   just check
   just test
   just web-check
   ```

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
   search, `?` help and the connection banner.
7. Check the MCP proxy:

   ```sh
   npx @modelcontextprotocol/inspector recvrd mcp
   ```

8. Check the systemd unit:

   ```sh
   systemctl --user restart recvrd
   systemctl --user status recvrd
   ```

9. Update `CHANGELOG.md` and `Cargo.toml` versions. Commit the changes.
10. Open a pull request for the version commit. Merge it after every CI check
    passes.

## Tag and publish

```sh
git tag -a v0.6.3 -m "recvrd v0.6.3"
git push origin v0.6.3
```

Create a GitHub release from the tag. The release workflow builds:

- `recvrd-v0.6.3-x86_64-unknown-linux-gnu.tar.gz`
- `recvrd-v0.6.3-aarch64-apple-darwin.tar.gz`
- `recvrd-v0.6.3-x86_64-apple-darwin.tar.gz`
- `recvrd-v0.6.3-x86_64-pc-windows-msvc.zip`
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
