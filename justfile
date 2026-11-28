set shell := ["bash", "-c"]

default:
    @just --list

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo check --workspace

test:
    cargo test --workspace
    @if [ -d crates/recvrd-web/node_modules ]; then cd crates/recvrd-web && npm test; else echo "web tests skipped (run 'just web-install')"; fi

# Includes tests that need real YouTube Music credentials and network access.
test-all:
    cargo test --workspace -- --include-ignored

# Reproduce the CI matrix. Run 'just web-install' first on a new checkout.
ci:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo check --workspace --all-targets
    cargo test --workspace
    cargo build -p recvrd
    @if cargo tree -i alsa-sys --target x86_64-unknown-linux-gnu 2>/dev/null | grep -q 'alsa-sys'; then echo "alsa-sys is still in the Linux dependency graph"; exit 1; fi
    @if ldd target/debug/recvrd | grep -q libasound; then echo "the Linux binary links libasound"; exit 1; fi
    @if cargo tree -p player --target x86_64-pc-windows-msvc -e normal 2>/dev/null | grep -qE 'pipewire|libspa'; then echo "PipeWire leaked into the Windows dependency graph"; exit 1; fi
    @if cargo tree -p player --target aarch64-apple-darwin -e normal 2>/dev/null | grep -qE 'pipewire|libspa'; then echo "PipeWire leaked into the macOS dependency graph"; exit 1; fi
    @test -d crates/recvrd-web/node_modules || { echo "web dependencies are missing; run 'just web-install'"; exit 1; }
    cd crates/recvrd-web && npm run check && npm test && npm run build
    nix flake check --no-build
    nix build .#recvrd .#recvc
    git diff --exit-code -- crates/recvrd-api/bindings

fmt:
    cargo fmt --all

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the daemon with the default bind/port.
dev:
    cargo run -p recvrd -- serve

# Run the terminal client against the default daemon URL.
tui:
    cargo run -p recvc

# Install web UI dependencies.
web-install:
    cd crates/recvrd-web && npm ci

# Type-check and build the web UI.
web-check:
    cd crates/recvrd-web && npm run check && npm run build

# Run the Vite dev server; /api is proxied to the daemon.
web:
    cd crates/recvrd-web && npm run dev

# Smoke-test a running daemon; search/SSE checks require #12/#13.
smoke:
    ./scripts/smoke.sh

# Start a temporary daemon and smoke-test status, web, search, SSE and MCP.
smoke-local:
    ./scripts/smoke-local.sh
