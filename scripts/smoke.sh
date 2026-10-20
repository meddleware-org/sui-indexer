#!/usr/bin/env bash
# Smoke test of a built sui-indexer image: bash scripts/smoke.sh <image>   (DOCKER=podman to use podman)
#
# Runs the image the way the cluster does — the ingester and the API as separate containers, non-root,
# read-only root filesystems, no capabilities, each with its own database role — against a real PostgreSQL 18
# with TLS (scripts/test-postgres.sh) provisioned like the platform's database tier (owner role + SELECT-only
# role, REVOKE FROM PUBLIC), and a local gRPC address nothing listens on. It checks what must hold without a chain:
#   - the image runs as the distroless nonroot user (65532) and has both binaries;
#   - before the ingester has migrated, the API answers /readyz 503 and /v1 503, and creates nothing;
#   - the ingester migrates; the API then answers /readyz 200 with the security headers, and /healthz 503
#     naming the streams that cannot poll;
#   - `/app/ingest --healthcheck` (the exec liveness probe) passes while the ingester runs;
#   - a second ingester is refused (single writer, advisory lock);
#   - the API's role cannot write or change the schema, whatever the process does;
#   - the API keeps serving when the ingester stops, answers 503 "database unavailable" while PostgreSQL is
#     down, and recovers by itself when it returns;
#   - (the 0.0.14 image) `ingest import-sqlite` exists and refuses a missing file.
set -euo pipefail

image="${1:?usage: smoke.sh <image>}"
docker="${DOCKER:-docker}"
here="$(cd "$(dirname "$0")" && pwd)"
port=18080
pgport=55442
pgdir="$(mktemp -d)"
pgname="sui-indexer-smoke-pg"

user="$("$docker" image inspect --format '{{.Config.User}}' "$image")"
if [ "$user" != "65532:65532" ]; then
  echo "image runs as '${user}', want 65532:65532" >&2
  exit 1
fi

out="$(mktemp)"
api="" ingest=""
cleanup() {
  for c in "$api" "$ingest"; do
    [ -n "$c" ] || continue
    echo "--- logs of ${c:0:12}" >&2
    "$docker" logs "$c" 2>&1 | tail -20 >&2 || true
    "$docker" rm -f "$c" > /dev/null 2>&1 || true
  done
  DOCKER="$docker" PG_NAME="$pgname" PG_DIR="$pgdir" bash "$here/test-postgres.sh" stop > /dev/null 2>&1 || true
  rm -rf "$out" "$pgdir" 2> /dev/null || true
}
trap cleanup EXIT

# 1. PostgreSQL with TLS, and the service's database provisioned as the tier does it.
DOCKER="$docker" PG_NAME="$pgname" PG_PORT="$pgport" PG_DIR="$pgdir" bash "$here/test-postgres.sh" start > /dev/null
# shellcheck disable=SC1091
. "$pgdir/env.sh"
admin_pw="$(sed -E 's#.*postgres://postgres:([^@]+)@.*#\1#' <<< "$TEST_DATABASE_URL")"
ingest_pw="$(openssl rand -hex 16)"
api_pw="$(openssl rand -hex 16)"
psql_admin() { "$docker" exec -i "$pgname" psql -X -q -v ON_ERROR_STOP=1 -U postgres -h 127.0.0.1 "$@"; }
psql_admin -d postgres << SQL
CREATE ROLE sui_indexer_ingest LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION CONNECTION LIMIT 8 PASSWORD '${ingest_pw}';
CREATE ROLE sui_indexer_api LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION CONNECTION LIMIT 20 PASSWORD '${api_pw}';
ALTER ROLE sui_indexer_api SET default_transaction_read_only = on;
ALTER ROLE sui_indexer_api SET statement_timeout = '5s';
CREATE DATABASE sui_indexer OWNER sui_indexer_ingest;
REVOKE ALL ON DATABASE sui_indexer FROM PUBLIC;
GRANT CONNECT, TEMPORARY, CREATE ON DATABASE sui_indexer TO sui_indexer_ingest;
GRANT CONNECT ON DATABASE sui_indexer TO sui_indexer_api;
SQL
psql_admin -d sui_indexer << SQL
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE, CREATE ON SCHEMA public TO sui_indexer_ingest;
GRANT USAGE ON SCHEMA public TO sui_indexer_api;
ALTER DEFAULT PRIVILEGES FOR ROLE sui_indexer_ingest IN SCHEMA public GRANT SELECT ON TABLES TO sui_indexer_api;
ALTER DEFAULT PRIVILEGES FOR ROLE sui_indexer_ingest IN SCHEMA public GRANT SELECT ON SEQUENCES TO sui_indexer_api;
SQL
db_url() { echo "postgres://$1:$2@127.0.0.1:${pgport}/sui_indexer?sslmode=verify-full&sslrootcert=/etc/postgres-ca/ca.crt"; }

common=(--network host --read-only --cap-drop ALL --security-opt no-new-privileges
  -v "${pgdir}/certs/ca.crt:/etc/postgres-ca/ca.crt:ro"
  -e NETWORK=localnet -e GRPC_URL=http://127.0.0.1:9
  -e ACCESS_GATE_ORIGINAL_ID=0x1111 -e SEAL_ORIGINAL_ID=0x2222 -e POLL_MS=1000)
api_env=(-e "DATABASE_URL=$(db_url sui_indexer_api "$api_pw")" -e PORT="$port" -e DB_POOL_SIZE=3)
ingest_env=(-e "DATABASE_URL=$(db_url sui_indexer_ingest "$ingest_pw")")

get() { # get <path> → status code, body in $out
  curl -sS -o "$out" -w '%{http_code}' "http://127.0.0.1:${port}$1" || echo 000
}

wait_for() { # wait_for <path> <status>
  for _ in $(seq 1 40); do
    [ "$(get "$1")" = "$2" ] && return 0
    sleep 1
  done
  echo "$1 never answered $2 (last: $(cat "$out"))" >&2
  exit 1
}

# 2. The API alone: the schema does not exist yet.
api="$("$docker" run -d "${common[@]}" "${api_env[@]}" "$image")"
wait_for /readyz 503
grep -q 'index not created yet' "$out" || { echo "/readyz 503 body: $(cat "$out")" >&2; exit 1; }
[ "$(get /v1/localnet/coverage)" = "503" ] || { echo "/v1 must answer 503 before the schema exists" >&2; exit 1; }
tables="$(psql_admin -At -d sui_indexer -c "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")"
[ "$tables" = "0" ] || { echo "the API created ${tables} table(s): it must never touch the schema" >&2; exit 1; }

# 3. The ingester migrates and starts; the API starts serving.
ingest="$("$docker" run -d "${common[@]}" "${ingest_env[@]}" "$image" /app/ingest)"
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

# 4. The ingester's exec liveness probe.
"$docker" exec "$ingest" /app/ingest --healthcheck || { echo "ingest --healthcheck failed" >&2; exit 1; }

# 5. Single writer: a second ingester is refused while the first runs.
if second="$("$docker" run --rm "${common[@]}" "${ingest_env[@]}" "$image" /app/ingest 2>&1)"; then
  echo "a second ingester started: the writer lock is not enforced" >&2
  exit 1
fi
grep -q 'writer lock' <<< "$second" || { echo "second ingester refused without naming the writer lock: ${second}" >&2; exit 1; }

# 6. Role separation, whatever the processes do: the API's role cannot write or change the schema.
api_psql() { PGPASSWORD="$api_pw" "$docker" exec -i -e PGPASSWORD "$pgname" psql -X -q -At -h 127.0.0.1 -U sui_indexer_api -d sui_indexer "$@"; }
# (the read-only default is switched off first, as a hostile client could: the grants must still refuse)
for sql in "INSERT INTO meta (key, value) VALUES ('x', 'y')" "CREATE TABLE evil (x int)" "DELETE FROM events"; do
  if err="$(api_psql -c "SET default_transaction_read_only = off" -c "$sql" 2>&1)"; then echo "the API role could run: ${sql}" >&2; exit 1; fi
  grep -q 'permission denied' <<< "$err" || { echo "unexpected refusal for '${sql}': ${err}" >&2; exit 1; }
done
api_psql -c "SELECT count(*) FROM events" > /dev/null || { echo "the API role cannot read" >&2; exit 1; }

# 7. The importer exists in the 0.0.14 image and refuses a missing file (nothing is written).
if [ "${SMOKE_EXPECT_IMPORTER:-1}" = "1" ]; then
  if imp="$("$docker" run --rm "${common[@]}" -e "DATABASE_URL=$(db_url sui_indexer_ingest "$ingest_pw")" "$image" /app/ingest import-sqlite /nonexistent.db 2>&1)"; then
    echo "import-sqlite accepted a missing file" >&2
    exit 1
  fi
  # (the running ingester holds the writer lock, so the importer is refused before it looks at the file)
  grep -qE 'writer lock|is not a file' <<< "$imp" || { echo "import-sqlite: unexpected output: ${imp}" >&2; exit 1; }
fi

# 8. The API keeps serving after the ingester stops (its own crash domain), and the ingester exits cleanly.
"$docker" stop -t 10 "$ingest" > /dev/null
[ "$("$docker" inspect --format '{{.State.ExitCode}}' "$ingest")" = "0" ] || { echo "ingest did not stop cleanly on SIGTERM" >&2; exit 1; }
[ "$(get /v1/localnet/coverage)" = "200" ] || { echo "the API stopped serving without the ingester: $(cat "$out")" >&2; exit 1; }
[ "$(get /readyz)" = "200" ] || { echo "/readyz failed without the ingester" >&2; exit 1; }

# 9. PostgreSQL goes away and comes back: the API says so (503, no internals) and recovers by itself.
"$docker" stop -t 10 "$pgname" > /dev/null
wait_for /v1/localnet/coverage 503
grep -q 'database unavailable' "$out" || { echo "database down: unexpected body: $(cat "$out")" >&2; exit 1; }
[ "$(get /readyz)" = "503" ] || { echo "/readyz must be 503 while the database is down" >&2; exit 1; }
"$docker" start "$pgname" > /dev/null
for _ in $(seq 1 30); do "$docker" exec "$pgname" pg_isready -q -h 127.0.0.1 -U postgres && break; sleep 1; done
wait_for /v1/localnet/coverage 200

echo "smoke test passed"
