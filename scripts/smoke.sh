#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${YTERMUSICD_URL:-http://127.0.0.1:8080}"

if ! command -v curl >/dev/null 2>&1; then
  echo "error: curl is required" >&2
  exit 1
fi

if ! curl -fsS --max-time 5 "$BASE_URL/api/status" >/dev/null; then
  echo "error: daemon is not reachable at $BASE_URL" >&2
  echo "start it with: cargo run -p ytermusicd -- serve" >&2
  exit 1
fi
echo "status: ok"

if ! curl -fsS --max-time 5 "$BASE_URL/" | grep -q 'id="app"'; then
  echo "error: web UI is not served at /" >&2
  exit 1
fi
echo "web: ok"

search=$(curl -fsS --max-time 15 "$BASE_URL/api/search?q=civilization")
if ! grep -q '"tracks"' <<<"$search"; then
  echo "error: search endpoint failed" >&2
  exit 1
fi
echo "search: ok"

video_id=$(printf '%s' "$search" | tr ',' '\n' | grep -m1 -o '"video_id":"[^"]*"' | cut -d'"' -f4)
if [[ -z "$video_id" ]]; then
  echo "error: search returned no video id" >&2
  exit 1
fi
if ! curl -fsS --max-time 20 -o /dev/null "$BASE_URL/api/tracks/$video_id/cover"; then
  echo "error: cover art endpoint failed" >&2
  exit 1
fi
echo "cover: ok"

sse_file=$(mktemp)
trap 'rm -f "$sse_file"' EXIT
curl -fsS -N --max-time 2 "$BASE_URL/api/events" >"$sse_file" 2>/dev/null || true
if ! grep -q . "$sse_file"; then
  echo "error: no SSE events received" >&2
  exit 1
fi
echo "events: ok"

if command -v playerctl >/dev/null 2>&1; then
  if playerctl -l 2>/dev/null | grep -q ytermusic; then
    echo "mpris: ok"
  else
    echo "error: MPRIS player 'ytermusic' not found" >&2
    exit 1
  fi
else
  echo "mpris: skipped (playerctl not installed)"
fi

if command -v ytermusicd >/dev/null 2>&1; then
  echo "mcp: available (run 'ytermusicd mcp' with an MCP client)"
else
  echo "mcp: skipped (ytermusicd not in PATH)"
fi
