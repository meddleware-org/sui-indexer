#!/usr/bin/env bash
# Smoke test of a built sui-indexer image: bash scripts/smoke.sh <image>   (DOCKER=podman to use podman)
#
# Runs the image the way the cluster does — two containers sharing one data directory, non-root,
# read-only root filesystems, the API's mount read-only — against a local gRPC address nothing
# listens on, and checks what must hold without a chain:
#   - the image runs as the distroless nonroot user (65532) and has both binaries;
#   - before the ingester has created the database the API answers /readyz 503 (and creates nothing);
#   - the ingester creates the database; the API (read-only mount) then answers /readyz 200 with the
#     security headers, and /healthz 503 naming the streams that cannot poll;
#   - `/app/ingest --healthcheck` (the exec liveness probe) passes while the ingester runs;
#   - the API keeps serving when the ingester stops (persisted WAL files on a read-only mount).
set -euo pipefail

image="${1:?usage: smoke.sh <image>}"
docker="${DOCKER:-docker}"
port=18080

user="$("$docker" image inspect --format '{{.Config.User}}' "$image")"
if [ "$user" != "65532:65532" ]; then
  echo "image runs as '${user}', want 65532:65532" >&2
  exit 1
fi

data="$(mktemp -d)"
chmod 0777 "$data"
out="$(mktemp)"
api="" ingest=""
cleanup() {
  for c in "$api" "$ingest"; do
    [ -n "$c" ] || continue
    echo "--- logs of ${c:0:12}" >&2
    "$docker" logs "$c" 2>&1 | tail -20 >&2 || true
    "$docker" rm -f "$c" > /dev/null 2>&1 || true
  done
  rm -rf "$data" "$out" 2> /dev/null || true
}
trap cleanup EXIT

common=(--read-only --cap-drop ALL --security-opt no-new-privileges
  -e NETWORK=localnet -e GRPC_URL=http://127.0.0.1:9
  -e ACCESS_GATE_ORIGINAL_ID=0x1111 -e SEAL_ORIGINAL_ID=0x2222 -e POLL_MS=1000)

get() { # get <path> → status code, body in $out
  curl -sS -o "$out" -w '%{http_code}' "http://127.0.0.1:${port}$1" || echo 000
}

wait_for() { # wait_for <path> <status>
  for _ in $(seq 1 30); do
    [ "$(get "$1")" = "$2" ] && return 0
    sleep 1
  done
  echo "$1 never answered $2 (last: $(cat "$out"))" >&2
  exit 1
}

# 1. The API alone: the database does not exist yet.
api="$("$docker" run -d "${common[@]}" -v "${data}:/data:ro" -p "127.0.0.1:${port}:8080" "$image")"
wait_for /readyz 503
grep -q 'index not created yet' "$out" || { echo "/readyz 503 body: $(cat "$out")" >&2; exit 1; }
[ "$(get /v1/localnet/coverage)" = "503" ] || { echo "/v1 must answer 503 before the database exists" >&2; exit 1; }
[ -z "$(ls -A "$data")" ] || { echo "the API created files on its read-only mount: $(ls -A "$data")" >&2; exit 1; }

# 2. The ingester creates the database; the API starts serving it.
ingest="$("$docker" run -d "${common[@]}" -v "${data}:/data" "$image" /app/ingest)"
wait_for /readyz 200
headers="$(curl -fsS -D - -o /dev/null "http://127.0.0.1:${port}/readyz")"
grep -qi '^x-content-type-options: nosniff' <<< "$headers" || { echo "/readyz lacks nosniff" >&2; exit 1; }
grep -qi '^access-control-allow-origin: \*' <<< "$headers" || { echo "/readyz lacks CORS *" >&2; exit 1; }

# Polls fail (nothing listens on :9); /healthz names both streams and the error once both have tried.
for _ in $(seq 1 20); do
  code="$(get /healthz)"
  grep -q '"sealed-content":{"ok":false,"lastPollAgeMs":null,"lastError":"[^"]' "$out" && break
  sleep 1
done
[ "$code" = "503" ] || { echo "/healthz answered ${code}, want 503 while no stream has polled" >&2; exit 1; }
grep -q '"access-gate"' "$out" || { echo "/healthz does not report the streams: $(cat "$out")" >&2; exit 1; }
grep -q '"lastError":"[^"]' "$out" || { echo "/healthz does not carry the poll error: $(cat "$out")" >&2; exit 1; }
[ "$(get /v1/localnet/coverage)" = "200" ] || { echo "/v1/localnet/coverage: $(cat "$out")" >&2; exit 1; }

# 3. The ingester's exec liveness probe.
"$docker" exec "$ingest" /app/ingest --healthcheck || { echo "ingest --healthcheck failed" >&2; exit 1; }

# 4. The API keeps serving after the ingester stops (its own crash domain).
"$docker" stop -t 10 "$ingest" > /dev/null
[ "$("$docker" inspect --format '{{.State.ExitCode}}' "$ingest")" = "0" ] || { echo "ingest did not stop cleanly on SIGTERM" >&2; exit 1; }
[ "$(get /v1/localnet/coverage)" = "200" ] || { echo "the API stopped serving without the ingester: $(cat "$out")" >&2; exit 1; }
[ "$(get /readyz)" = "200" ] || { echo "/readyz failed without the ingester" >&2; exit 1; }

echo "smoke test passed"
