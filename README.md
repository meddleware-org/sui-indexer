# sui-indexer

A lean read-indexer for Meddleware's Sui packages. It polls the `access_gate` events and
`sealed_content::SealedContentPublished` from a Sui full node over gRPC, stores them in PostgreSQL (the
platform's shared database tier), and serves cacheable GET pages. Apps use it to list history cheaply and fall
back to the full node when it is unavailable.

- **Two processes, one image, two Deployments.** Rust, shipped as `/app/ingest` and `/app/api` on distroless
  (`cc-debian13:nonroot`); the cluster runs them as separate Deployments with separate database roles.
  - `ingest` is the **only writer**. It reads `ListEvents` with Mysten's `sui-rpc` client and writes
    PostgreSQL as the database owner: it applies the schema migrations at startup and holds a
    `pg_try_advisory_lock` for the life of the process, so a second writer is refused.
  - `api` reads through a **`SELECT`-only role** (it cannot write or change the schema, whatever the process
    does), has no chain access and serves HTTP/1.1 with `hyper`. It learns how fresh the index is from the
    ingester's heartbeat rows. It is stateless: more than one replica is safe.
  - Measured 2026-10-20 on a shared 4-CPU laptop (release build, PostgreSQL 18 with TLS on the same host, pool of
    8, a closed-loop client with 64 keep-alive connections competing for the same cores; one-off, not a
    benchmark): the API serves about 3.6-4k req/s of the head page, a rare-kind page or a gate page, 2.5k for two
    kinds, 9.6k for `/coverage`, at about 10 MB RSS, with 17 rows or 300,000. The ingester is about 6 MB. Request cost does not grow with history; Cloudflare's cache
    (below) is what the public sees.
- **Same decoding as the apps.** The ingester does not decode BCS. It takes the event kind from the type
  name and the gate and commission from the full node's JSON rendering, and stores the BCS verbatim; the
  apps decode it with `@meddleware/access-gate-client` and `@meddleware/seal-client`, as they decode
  full-node events.
- **Cheap under load.**
  - Older pages never change and are served `immutable`.
  - The newest page is cached 10 s in browsers and 30 s at the edge.
  - Behind Cloudflare's cache, the home server answers only cache misses.
  - A per-IP token bucket keyed on `CF-Connecting-IP` limits what gets through.
- **Display-only.** Nothing may use it to authorise. Every row carries `transactionDigest` and
  `checkpoint`, so it can be checked against a full node.

## API

All routes are `GET` (and `HEAD`), and responses are JSON with `Access-Control-Allow-Origin: *`.

| Route | Returns |
| --- | --- |
| `/healthz` | `200` while EVERY stream's polls succeed (the last within 5 intervals), else `503`, with `{ ok, streams: { <stream>: { ok, lastPollAgeMs, lastError } } }`; for monitoring. It reads the ingester's heartbeat rows. |
| `/readyz` | `200` while the schema is at the version the API needs and the database answers; for the readiness probe, so stale data keeps being served while the full node or the ingester is down. `503` (`index not created yet`) before the ingester has migrated, `503` (`database unavailable`) while PostgreSQL cannot be reached. |
| `/v1/{network}/access-gate/events?kinds&gate&before&limit` | `{ events, cursor, indexedFromCheckpoint }` (head pages also `latestCheckpoint` and `polledAt`), newest first; `limit` ≤ 100 (default 20); `kinds` is a comma list of `GateCreated`, `AccessMinted`, `AccessConsumed`, `AccessBurned`, `GateFrozen`, `GateMadeFree`, `PlatformConfigUpdated`, `PlatformMigrated` |
| `/v1/{network}/sealed-content?gate&before&limit` | the same shape for one gate (`gate` required); `limit` ≤ 200 (default 50) |
| `/v1/{network}/gates/{id}/stats` | `{ gateId, minted, consumed, burned, commissionMist, indexedFromCheckpoint }` (`commissionMist` a decimal string) |
| `/v1/{network}/coverage` | oldest and newest indexed checkpoints, event counts, skipped-undecodable counts and pruned-cursor restarts (`gaps`) per stream |

- **Events.** Each event is the full node's entry: `eventType`, `sender`, `bcs` (base64),
  `checkpoint`, `transactionDigest` and `eventIndex`.
- **Cursors.** `cursor` is `<generation>.<row id>`. A rebuilt database gets a new generation, and
  an older cursor then returns `410`.
- **Before the schema exists** (a new database, until the ingester's first start) and **while the database is down**,
  `/v1` routes and `/healthz` answer `503` with `no-store` (`index not created yet`, `database unavailable`); the
  cause of an outage is logged, not sent.
- **Package changes.** Changing a stream's package (`ACCESS_GATE_ORIGINAL_ID`, `SEAL_ORIGINAL_ID` or the
  recorded defaults) would mean deleting the stored history, which the full node may already have
  pruned. The ingester therefore **refuses to start** until you set `REINDEX_CONFIRM=<stream>[,<stream>]`;
  it then copies the whole index into a new schema `bak_<epoch ms>` (`CREATE TABLE … AS`, in the same
  transaction as the clearing: both happen or neither does), clears that stream, re-indexes it from the new
  package's first event and rotates the generation. Restore a table with
  `INSERT INTO events OVERRIDING SYSTEM VALUE SELECT * FROM bak_<ms>.events` (likewise the other tables), and
  `DROP SCHEMA bak_<ms> CASCADE` when it is no longer needed. The database tier's daily `pg_dump` covers the
  `bak_` schemas too.
- **Validation.** Unknown query parameters, bad ids and out-of-range limits return `400`.
- **Canonical URLs.** Another spelling of the same query (a short or upper-case `gate`, `limit=05`,
  unsorted or repeated `kinds`, an upper-case id in the stats path) gets a cacheable `301` to the
  canonical form, so each query has one cache entry. The client packages already send canonical
  values; only the order of parameters is left as sent (a bounded number of permutations).
- **Freshness.** Head pages carry `polledAt` (epoch ms of the stream's last successful poll) and
  `latestCheckpoint`. Older pages are immutable and carry neither.
  The client packages (access-gate-client 0.0.10, seal-client 0.0.21) treat a head page whose `polledAt` is missing,
  `null` or older than 60 s as unavailable and read the full node instead.
- **Transport.** 5 s to receive the request headers, 10 s per request, at most 1024 open connections and
  a 32 KiB request head; `OPTIONS` preflight answered, other methods `405`.

## Configuration

| Variable | Read by | Default |
| --- | --- | --- |
| `NETWORK` | both | `testnet` (the one network this instance serves) |
| `DATABASE_URL` | both | required: `postgres://user:password@host:5432/database?sslmode=verify-full&sslrootcert=/etc/postgres-ca/ca.crt`. **`verify-full` and `sslrootcert` are required** (other `sslmode`s are refused), an empty password is refused, and neither the URL nor the password is ever logged. The ingester uses the owner role, the API the `SELECT`-only role. |
| `ALLOW_INSECURE_DATABASE` | both | unset. `1` admits `sslmode=disable` and an empty password: **local tests only** |
| `DB_POOL_SIZE` | api | `8` (1-64). The role's `CONNECTION LIMIT` is 20: replicas × pool (plus a rolling update's surge) must stay within it. The ingester uses exactly one connection (it carries the writer lock) |
| `POLL_MS` | both | `15000` (the API uses it for the `/healthz` 5-interval rule) |
| `GRPC_URL` | ingest | the public Mysten full node for `NETWORK` (`https` required; plain `http` only for `localhost` and `127.0.0.1`) |
| `ACCESS_GATE_ORIGINAL_ID` / `SEAL_ORIGINAL_ID` | ingest | the deployments recorded in the client packages (`crates/store/src/config.rs`) |
| `REINDEX_CONFIRM` | ingest | unset: a package change stops startup (see Package changes) |
| `PORT` | api | `8080` |
| `RATE_LIMIT_PER_SEC` / `RATE_LIMIT_BURST` | api | `10` / `40` per client IP (IPv6 clients by /64; at most 50 000 buckets are kept) |

- **First start.** The ingester backfills from the oldest event the full node still serves.
  History before that is not listed; `indexedFromCheckpoint` says where coverage starts.
- **Streams.** Each is read with one filter: the `access_gate` module, or the one sealed-content
  event.
- **Crash safety.** A page and its cursor are stored in one transaction. If the connection (which carries the
  writer lock) is lost, the ingester stops with a non-zero exit and Kubernetes restarts it.
- **Single writer.** `events.id` is an identity column written by one connection, so it is ledger order: a page
  bounded by `before` never changes once served. Ids are **ordered, not gapless** (an ignored duplicate or a
  rolled-back page consumes numbers); nothing reads an id as a count.
- **Migrations.** `crates/store/migrations/*.sql`, applied by the ingester (the database owner) at startup under
  `pg_advisory_xact_lock`, recorded with a SHA-256 in `schema_migrations`; a released migration is never edited.
  A migration must keep the previous version's queries working (add, then contract in a later release), because
  the API of the previous image runs while the new ingester migrates. The API cannot run DDL and answers `503`
  until the schema is at the version it needs.
- **Independent streams.** One stream failing never stops the other; each reports its own health.
- **Undecodable events** — a recognised event without what its row needs (no JSON rendering or field, no
  BCS, no position) — are counted, logged and skipped (the page advances past them).
- **Pruned cursor.** If the stored cursor falls below what the full node still serves (no event arrived
  to move it before the node pruned), the stream restarts from the node's earliest data and counts a
  `gap`; the events in between are gone for good.
- **Counters.** Coverage, gate stats and commission are maintained at ingest, so serving a page never
  reads a whole stream for them.
- **Indexes.** `events_stream (stream, id)` serves unfiltered pages, `events_gate (stream, gate_id, id)` serves
  every `gate=` page (with or without `kinds`), and `events_kind (stream, kind, id)` serves `kinds` pages
  (one seek per kind, merged), so a rare kind costs its own rows, not the stream's. All three are created by
  migration 1. Query-plan tests (`EXPLAIN`, buffer counts on 300,000 rows) pin which index each filter shape uses.
  The API's pooled connections force `plan_cache_mode = force_custom_plan`, because the generic plan of the
  one-kind query walks the primary key (right for a common kind, a table walk for a rare one).
- **One statement per request.** An events response (the generation, the stream's newest id for the ETag, its
  counters, its poll heartbeat and the page) is read by ONE `SELECT`: one round trip and one snapshot, and a
  cancelled request cannot leave a pooled connection in an open transaction.
- **Chain identity.** At startup the node's chain identifier must match `NETWORK` (testnet, mainnet).
- **Liveness.** `ingest --healthcheck` exits 0 while PostgreSQL answers and the ingester refreshed
  `heartbeats ingest:alive` within `5 × POLL_MS + 30 s`; it is the container's exec probe (distroless has no
  shell). The API's liveness probe is a TCP check: a database outage must not restart it (`/readyz` reports it).
- **No counter backfill.** The PostgreSQL schema is created with the counters, and every row enters through
  `ingest`, which updates them in the same transaction (`Store::verify_counters` recounts from the rows).
- **Image contents.** `/app/api`, `/app/ingest`, `Cargo.lock` (for scanners), `THIRD_PARTY_LICENSES-api.txt`
  and `THIRD_PARTY_LICENSES-ingest.txt` (cargo-about) and `LICENSE`. No volume: all state is in PostgreSQL.
  The CA that verifies the server is mounted at run time (`/etc/postgres-ca/ca.crt`).
- **SBOMs.** Each release attests three to the image digest: the image (SPDX, syft) and one CycloneDX SBOM
  per binary (cargo-cyclonedx), checked against the `Cargo.lock` graph by `scripts/check-sbom.sh`.
  `cosign verify-attestation --type spdxjson|cyclonedx <image>@<digest>` returns them.

## Development

The tests run against a real PostgreSQL 18 with TLS. `scripts/test-postgres.sh` starts the pinned image
(`DOCKER=podman` works rootless) and writes the environment:

```bash
DOCKER=podman bash scripts/test-postgres.sh start && . "${TMPDIR:-/tmp}/sui-indexer-test-pg/env.sh"   # TEST_DATABASE_URL, REQUIRE_TEST_DATABASE=1
cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked                   # each test gets its own database and the two production roles
cargo deny check                                  # advisories, licences, bans, sources
cargo build --release --locked
# a local run against testnet, with the roles of the database tier (or any database you own):
DATABASE_URL='postgres://…/sui_indexer?sslmode=verify-full&sslrootcert=ca.crt' ./target/release/ingest &
DATABASE_URL='postgres://…(select-only role)…' PORT=8080 ./target/release/api
bash scripts/smoke.sh <image>                     # ingester and API in containers against PostgreSQL (DOCKER=podman works)
DOCKER=podman bash scripts/test-postgres.sh stop
```

Without `TEST_DATABASE_URL` the database tests print `SKIPPED` and return; CI sets `REQUIRE_TEST_DATABASE=1` (a
missing server is a failure) and fails on any ignored test. Test layers: pure unit tests (URL and config parsing,
paging logic), tests against PostgreSQL (store, API handler, ingest loop), the socket wiring, the
client contract and the image smoke test.

The client contract (`tests/contract`, CI only) runs the published client packages against the built
`api` binary, with a database seeded by the testkit's `seed-fixture`:
`cargo build --release -p sui-indexer-api -p sui-indexer-testkit && cd tests/contract && npm ci && TEST_DATABASE_URL=… npm test`.

Container image: `quay.io/meddleware-org/sui-indexer`, built by `docker-publish.yml` on a `v*` tag.
The tag must match the Cargo workspace version. Deployment manifests live in the infrastructure workspace
(`post-bootstrap/sui-indexer`).

Release flow (`docker-publish.yml`): each architecture is compiled on its own **native** runner (`ubuntu-latest`
for amd64, `ubuntu-24.04-arm` for arm64; the earlier single QEMU-emulated build took about 1.5 hours). The builds
hold no registry credentials: each exports an OCI archive, a short job pushes it as `<commit>-amd64` /
`<commit>-arm64`, and a merge job builds the multi-arch index (`docker buildx imagetools create`, which keeps
each architecture's BuildKit provenance), scans it with Trivy **by digest**, then signs it and attaches the
SBOMs and the build-provenance attestation. Everything (signature, attestations, the self-hosted mirror) is tied
to that merged index digest. The `<commit>-<arch>` tags are build intermediates and are left in place: deploy by
the signed index digest, never by those tags. To exercise the pipeline without publishing, run the workflow by
hand (Actions, "Publish (Docker)", `dry_run` on, the default): it builds both architectures and merges and scans
them in a throwaway registry on the runner, with no push, no signature and no secret.

## Licence

BSD Zero Clause (`0BSD`) — see [LICENSE](LICENSE).
