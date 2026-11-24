# Contributing

## Development environment

Use the Nix dev shell when possible:

```sh
nix develop
```

See [docs/testing.md](docs/testing.md) for the full local development and
testing guide.

## Before opening a pull request

1. Create a worktree on a branch for the issue. See [AGENTS.md](AGENTS.md).
2. Run `just web-install` one time on a new checkout.
3. Run `just ci` until it passes.
4. Squash and fixup the commits into one clean commit.
5. Push the branch one time.
6. Open the pull request, or mark the draft ready for review.

A draft pull request runs no CI jobs. Mark the pull request ready for review
to run the full matrix. CI confirms the local result. It must not find new
failures. `just check` and `just test` are faster subsets for the inner loop.

For bug fixes and behavior changes, add a test that fails before the fix and
passes after it. If the failure cannot be tested deterministically (audio
device, DBus/MPRIS, live YouTube network), document the manual reproduction and
add a smoke test where practical.

## Secrets

Never commit `headers.txt`, `account_id.txt`, browser cookies, or logs that
contain them. Use `RECVRD_HEADERS` to point tests at a file outside the
repository.

## Scope

This project is a fork of [ytermusic](https://github.com/ccgauche/ytermusic).
Keep generic fixes reusable and call out changes that should be upstreamed.
See [0004 — upstream strategy](docs/decisions/0004-upstream-strategy.md) for the
sync procedure and change classification.
