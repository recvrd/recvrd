#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${YTERMUSICD_HEADERS:-}" ]]; then
  echo "error: set YTERMUSICD_HEADERS to a valid headers.txt" >&2
  exit 1
fi
if [[ ! -f "$YTERMUSICD_HEADERS" ]]; then
  echo "error: headers file not found: $YTERMUSICD_HEADERS" >&2
  exit 1
fi

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build -p ytermusicd
bin="$root/target/debug/ytermusicd"

tmp=$(mktemp -d)
port=$((20000 + RANDOM % 20000))
base="http://127.0.0.1:$port"
export XDG_CONFIG_HOME="$tmp/config"
export XDG_CACHE_HOME="$tmp/cache"
export XDG_STATE_HOME="$tmp/state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME"
log="$tmp/daemon.log"

"$bin" serve --bind 127.0.0.1 --port "$port" --headers "$YTERMUSICD_HEADERS" >"$log" 2>&1 &
pid=$!

cleanup() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT

ready=0
for _ in $(seq 1 60); do
  if curl -fsS --max-time 1 "$base/api/status" >/dev/null 2>&1; then
    ready=1
    break
  fi
  if ! kill -0 "$pid" 2>/dev/null; then
    cat "$log" >&2
    exit 1
  fi
  sleep 0.5
done
if [[ "$ready" != "1" ]]; then
  cat "$log" >&2
  echo "error: daemon did not become ready" >&2
  exit 1
fi
echo "status: ok"

curl -fsS --max-time 5 "$base/api/status" | grep -q '"status":"ok"'
echo "health: ok"

curl -fsS --max-time 5 "$base/" | grep -q 'id="app"'
echo "web: ok"

search=$(curl -fsS --max-time 30 "$base/api/search?q=civilization")
if ! grep -q '"tracks"' <<<"$search"; then
  echo "error: search endpoint failed" >&2
  exit 1
fi
echo "search: ok"

track_id=$(printf '%s' "$search" | tr ',' '\n' | grep -m1 -o '"track_id":"[^"]*"' | cut -d'"' -f4)
if [[ -z "$track_id" ]]; then
  echo "error: search returned no track id" >&2
  exit 1
fi
curl -fsS --max-time 20 -o /dev/null "$base/api/tracks/youtube/$track_id/cover"
echo "cover: ok"

sse=$(curl -sS -N --max-time 2 "$base/api/events" || true)
if ! grep -q 'snapshot' <<<"$sse"; then
  echo "error: no SSE snapshot received" >&2
  exit 1
fi
echo "events: ok"

mcp=$(
  {
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}'
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
  } | timeout 15 "$bin" mcp --url "$base" || true
)
if ! grep -q '"tools"' <<<"$mcp"; then
  echo "error: MCP tools/list failed" >&2
  exit 1
fi
if ! grep -q '"name":"search"' <<<"$mcp"; then
  echo "error: MCP search tool is missing" >&2
  exit 1
fi
echo "mcp: ok"

echo "smoke test passed"
