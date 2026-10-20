#!/usr/bin/env bash
# Record the answers of a real 0.0.13 `api` binary (SQLite) for every route in tests/golden/routes.tsv, over the
# importer's fixture index. The result, tests/golden/0.0.13.json, is what the PostgreSQL API must reproduce
# after `ingest import-sqlite` (crates/api/tests/import_parity.rs).
#
#   bash scripts/golden-from-0.0.13.sh <path-to-0.0.13-api> <seed-fixture-binary-built-with-import-sqlite>
#
# Build the old binary from the v0.0.13 tag (`cargo build --release -p sui-indexer-api`) and the fixture
# writer with `cargo build -p sui-indexer-testkit --features import-sqlite`.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
old_api="${1:?usage: golden-from-0.0.13.sh <0.0.13 api binary> <seed-fixture binary>}"
seed="${2:?usage: golden-from-0.0.13.sh <0.0.13 api binary> <seed-fixture binary>}"
work="$(mktemp -d)"
pid=""
trap '[ -z "$pid" ] || kill "$pid" 2> /dev/null || true; rm -rf "$work"' EXIT
port=$((19000 + RANDOM % 500))

"$seed" sqlite "$work/indexer.db" > /dev/null
NETWORK=testnet DB_PATH="$work/indexer.db" PORT="$port" RATE_LIMIT_PER_SEC=10000 RATE_LIMIT_BURST=10000 "$old_api" > "$work/api.log" 2>&1 &
pid=$!
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:${port}/readyz" > /dev/null 2>&1 && break
  sleep 0.1
done

echo '[]' > "$work/out.json"
while IFS=$'\t' read -r method path extra; do
  [ -n "$method" ] || continue
  args=(-sS -X "$method" -D "$work/headers" -o "$work/body" "http://127.0.0.1:${port}${path}")
  [ "$method" = HEAD ] && args=(-sS -I -D "$work/headers" -o /dev/null "http://127.0.0.1:${port}${path}")
  [ -n "${extra:-}" ] && args=(-H "$extra" "${args[@]}")
  : > "$work/body" # curl leaves the file alone when the answer has no body (304, 204, HEAD)
  curl "${args[@]}" > /dev/null
  status="$(head -1 "$work/headers" | awk '{print $2}')"
  # The headers that are part of the contract (cache cost model, validators, CORS), lower-cased names.
  headers="$(tr -d '\r' < "$work/headers" | tail -n +2 | awk -F': ' 'tolower($1) ~ /^(cache-control|etag|location|content-type|access-control-allow-origin|access-control-allow-methods|access-control-max-age|x-content-type-options|allow|retry-after)$/ {printf "%s\t%s\n", tolower($1), substr($0, length($1) + 3)}' | jq -R 'split("\t") | {(.[0]): .[1]}' | jq -s 'add // {}')"
  jq --arg method "$method" --arg path "$path" --arg extra "${extra:-}" --argjson status "$status" --argjson headers "$headers" --rawfile body "$work/body" \
    '. + [{method: $method, path: $path, requestHeader: $extra, status: $status, headers: $headers, body: $body}]' "$work/out.json" > "$work/out.next"
  mv "$work/out.next" "$work/out.json"
done < "$here/tests/golden/routes.tsv"
jq '.' "$work/out.json" > "$here/tests/golden/0.0.13.json"
echo "wrote tests/golden/0.0.13.json ($(jq length "$here/tests/golden/0.0.13.json") routes)"
