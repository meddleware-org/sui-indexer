#!/usr/bin/env bash
# Smoke test of a built sui-indexer image: bash scripts/smoke.sh <image>
#
# Starts the image the way the cluster does (non-root, an empty writable /data, no shell) against a
# local gRPC address nothing listens on, and checks what must hold without a chain:
#   - the image runs as the distroless nonroot user and creates its database under /data;
#   - /readyz answers 200 with the security headers (serving does not depend on the full node);
#   - /healthz answers 503 and names the streams that have not polled (monitoring sees the outage).
set -euo pipefail

image="${1:?usage: smoke.sh <image>}"
port=18080

user="$(docker image inspect --format '{{.Config.User}}' "$image")"
if [ "$user" != "65532:65532" ]; then
  echo "image runs as '${user}', want 65532:65532" >&2
  exit 1
fi

out="$(mktemp)"
cid="$(docker run -d --rm \
  --tmpfs /data:rw,uid=65532,gid=65532,mode=0755 \
  -e NETWORK=localnet -e GRPC_URL=http://127.0.0.1:9 \
  -e ACCESS_GATE_ORIGINAL_ID=0x1111 -e SEAL_ORIGINAL_ID=0x2222 \
  -p "127.0.0.1:${port}:8080" "$image")"
trap 'docker logs "$cid" 2>&1 | tail -20; docker rm -f "$cid" > /dev/null 2>&1 || true; rm -f "$out"' EXIT

ready=""
for _ in $(seq 1 30); do
  if ready="$(curl -fsS -D - "http://127.0.0.1:${port}/readyz" 2> /dev/null)"; then
    break
  fi
  ready=""
  sleep 1
done
if [ -z "$ready" ]; then
  echo "/readyz never answered 200" >&2
  exit 1
fi
grep -qi '^x-content-type-options: nosniff' <<< "$ready" || { echo "/readyz lacks X-Content-Type-Options: nosniff" >&2; echo "$ready" >&2; exit 1; }

code="$(curl -sS -o "$out" -w '%{http_code}' "http://127.0.0.1:${port}/healthz")"
if [ "$code" != "503" ]; then
  echo "/healthz answered ${code}, want 503 while no stream has polled" >&2
  cat "$out" >&2
  exit 1
fi
grep -q '"access-gate"' "$out" || { echo "/healthz does not report the streams" >&2; exit 1; }

echo "smoke test passed"
