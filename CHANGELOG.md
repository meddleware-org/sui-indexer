# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.18] - 2026-10-24

### Changed

- Log and error text name the Move package `sealed_storage` (was `seal_policies`; ADR-0004). No behaviour change.

## [0.0.17] - 2026-10-24

### Security

- `ctutils` 0.4.5 (RUSTSEC-2026-0385). The client contract tests pin `@meddleware/sealed-storage-client` 0.0.24 and `@mysten/sui` 2.37.0. Comments use the renamed package names. No behaviour change.

## [Unreleased]

## [0.0.16] - 2026-10-22

### Security

- `rustls-webpki` 0.103.16 and `webpki-roots` 1.0.10 (RUSTSEC-2026-0382 and RUSTSEC-2026-0381: certificate paths through distrusted emSign intermediates could be accepted). Affects the TLS connection to the full node.

## [0.0.15] - 2026-10-20

### Removed

- The one-off SQLite importer and `rusqlite`; no behaviour change for the API. Gone: the `import-sqlite` cargo
  feature (store, ingest, api, testkit), `ingest import-sqlite`, `crates/store/src/import.rs`,
  `crates/ingest/src/import_cmd.rs`, the testkit's SQLite fixture builder (`seed-fixture sqlite`), the importer
  tests and the import-parity test with its recorded 0.0.13 answers (`tests/golden`,
  `scripts/golden-from-0.0.13.sh`), the image's `CARGO_FEATURES` build argument, the `CARGO_FEATURES` option of
  `scripts/check-sbom.sh`, and `rusqlite`, `libsqlite3-sys` and the crates only they pulled in from `Cargo.lock`.
  The image links no SQLite; its `ingest` binary is 2.1 MB smaller.

### Changed

- Release workflow only (no code or image change): `docker-publish.yml` builds amd64 and arm64
  on native runners instead of one QEMU-emulated build (the 0.0.14 publish took about 1.5 hours). The builds hold
  no registry credentials; a merge job builds the index, scans it by digest, signs and attests it as before
  (D61). A manual run is a dry run that publishes nothing. New `scripts/check-index.sh`.

## [0.0.14] - 2026-10-20

### Changed

- **BREAKING (internal): storage moves from embedded SQLite to the platform's shared PostgreSQL tier**
  (decisions D59, D60; OQ7). The HTTP API is unchanged: every route, status, header, ETag, cursor and error body
  is byte-identical to 0.0.13's (checked against a real 0.0.13 `api` binary over the same data, 53 routes, and
  against the live instance, 97 routes).
  - `crates/store` is rewritten on `tokio-postgres` 0.7.18 (+ `deadpool-postgres` 0.14.2 for the API's pool,
    `tokio-postgres-rustls` 0.14.0 with `rustls` 0.23.46 and the **ring** provider the gRPC client already
    uses). Still the only crate with SQL. The schema is the same logical schema (`migrations/0001_init.sql`):
    `bytea` for BCS, `bigint` for every integer, `numeric(39,0)` for commission totals, `CHECK` and `NOT NULL`
    where the code relies on them, the same three indexes, and a new `heartbeats` table for the poll and
    liveness rows (they were `meta` rows).
  - **Ids are ordered, not gapless.** `events.id` is a `GENERATED ALWAYS AS IDENTITY` column written by one
    connection; an ignored duplicate or a rolled-back page consumes numbers. Nothing (ETag, cursor, paging)
    reads an id as a count.
  - **Versioned migrations** (`schema_migrations`, SHA-256 per migration), applied by the ingester as the
    database owner at startup under `pg_advisory_xact_lock`. The API cannot run DDL; it answers `503`
    (`/readyz` 503, `/v1` 503, body `index not created yet`) until the schema is at the version it needs.
  - **Single writer enforced:** the ingester takes `pg_try_advisory_lock` on its one connection and holds it for
    the life of the process; a second writer (or the importer) exits non-zero. A lost connection loses the lock,
    so the ingester stops with a non-zero exit and Kubernetes restarts it.
  - **Read-only by role.** The API connects as a `SELECT`-only role; additionally every pooled connection is
    `SET default_transaction_read_only = on` and every request is a `REPEATABLE READ READ ONLY` snapshot (one
    consistent view per request, as the SQLite read transaction was). Pooled statements force custom plans
    (`plan_cache_mode = force_custom_plan`): a generic plan of the one-kind page walks the primary key.
  - **TLS `verify-full` only.** `DATABASE_URL` must carry `sslmode=verify-full` and `sslrootcert`; other modes
    are refused (local plaintext needs the explicit `ALLOW_INSECURE_DATABASE=1`). An empty password is refused.
    The URL and password are never logged.
  - `REINDEX_CONFIRM` is unchanged, but the pre-delete backup is a `bak_<ms>` schema copy
    (`CREATE TABLE … AS`) in the same transaction as the clearing (restore:
    `INSERT INTO events OVERRIDING SYSTEM VALUE SELECT * FROM bak_<ms>.events`, and likewise per table).
  - **No counter backfill and no cutover backup:** the `stats:v2` backfill and the `VACUUM INTO` cutover copy
    belonged to the SQLite store's upgrade path; every row now enters through `ingest`, which keeps the counters
    in the same transaction, and the importer copies and re-verifies them.
  - **The only `unsafe` code is gone** (the persisted-WAL FFI call); `unsafe_code = "deny"` has no exception left.
- **Two Deployments instead of one pod** (workspace `post-bootstrap/sui-indexer`): `sui-indexer-ingest` (1 replica,
  no Service, no volume) and `sui-indexer-api` (stateless; more than one replica is safe). Each has its own
  database role and Secret; NetworkPolicies are per pod.
- **Configuration:** `DB_PATH` is replaced by `DATABASE_URL`; new `ALLOW_INSECURE_DATABASE` (tests only) and
  `DB_POOL_SIZE` (API, default 8, 1–64; the ingester uses exactly one connection). Everything else is unchanged.
- `ingest --healthcheck` now needs the database: it fails when PostgreSQL is unreachable or the liveness
  heartbeat is older than `5 × POLL_MS + 30 s`. The API's liveness probe becomes a TCP check (a database outage
  must not restart the API); `/readyz` reports database reachability and the schema version.
- Tests run against a real PostgreSQL 18 (`TEST_DATABASE_URL`, an isolated database per test provisioned with the
  production roles; CI sets `REQUIRE_TEST_DATABASE=1` so none can be skipped). `scripts/test-postgres.sh`
  starts a pinned, TLS-enabled container (docker or rootless podman). Query-plan tests (`EXPLAIN`, custom and
  buffer counts on 300,000 rows) pin the index use of every filter shape.

### Added

- **Temporary: `ingest import-sqlite <path> [--replace-existing]`**, behind the cargo feature `import-sqlite`
  (the 0.0.14 image is built with it; SQLite is compiled in for it and for nothing else). It reads a 0.0.13
  SQLite index read-only and copies it into an empty PostgreSQL index **with the same ids, generation, package
  bindings, cursors and counters**, in one transaction, verifying row counts per stream, every cursor, the
  commission total, the largest id, a SHA-256 over `id:tx_digest:event_index` in id order and a recount of the
  counters before it commits; it prints a report and sets `meta schema_writer = postgres`. A target that already
  holds data is refused (`--replace-existing` copies it to a `bak_<ms>` schema first). **0.0.15 removes the
  feature, `rusqlite` and the importer** (tracked in `OPERATOR_TASKS.md` of the infrastructure workspace).
- `scripts/golden-from-0.0.13.sh` and `tests/golden/0.0.13.json`: the recorded answers of a real 0.0.13 API over
  the importer's fixture; `crates/api/tests/import_parity.rs` requires the imported index to reproduce them.

### Removed

- `rusqlite`/SQLite from the default build; `DB_PATH`; the data volume; the `-wal`/`-shm` persistence and the one
  `unsafe` call that did it; the `.bak-pre-rust-*` cutover copy; the counter backfill.

### Security

- Least privilege is enforced by the database (a role without a write grant) instead of by a mount flag and an
  open mode; the database tier's role/limit/timeout settings bound a runaway client. The ingester and the API
  hold different credentials. See `SECURITY.md` and the audit (findings F27–F32).

## [0.0.13] - 2026-10-19

### Changed

- **A `kinds` filter for a rare kind no longer walks the stream (audit F25).** New index
  `events_kind (stream, kind, id)`, additive (0.0.11 and 0.0.12 ignore it, so a rollback still works). The
  ingester creates it at startup with `CREATE INDEX IF NOT EXISTS`, so a fresh database and an existing one
  both get it (about 0.5 s on 300,000 rows; WAL readers keep reading meanwhile); the read-only API never
  creates it and answers correctly, only slower, until it exists.
- `crates/store` page query: several `kinds` (no `gate`) run one bounded `events_kind` seek per kind, merged
  by id, because a single `kind IN (…)` query makes SQLite walk the whole stream to avoid a sort; with a
  `gate` the kinds only filter the gate's rows (`events_gate`, as before). Repeated kinds are ignored.
  Responses are unchanged.
- Measured on 300,000 rows (release build, one thread): a page for a rare kind 8 pages/s before, about
  33,000 after; two kinds, one rare, about 9,000.

## [0.0.12] - 2026-10-19

### Changed

- **BREAKING (internal): rewritten in Rust; the HTTP API is unchanged.** Backend services are not written in
  JavaScript/TypeScript (maintainer rule). A Cargo workspace replaces the Node.js bundle:
  - `crates/store` — the SQLite schema, writes and reads (`rusqlite`, bundled SQLite); the schema, pragmas,
    meta keys, generation, counters and the one-time counter backfill are those of 0.0.11;
  - `crates/ingest` → `/app/ingest`, the **sole writer**: `ListEvents` over gRPC through Mysten's
    `sui-rpc` 0.4.0, ascending from the stored cursor, 50 events a page, at most 20 pages per stream per poll,
    30 s per request, page and cursor in one transaction, pruned-cursor gap recovery, package binding with
    `REINDEX_CONFIRM` and a `VACUUM INTO` backup, chain-identity check, `POLL_MS` loop, clean SIGTERM;
  - `crates/api` → `/app/api`: hyper, no web framework; opens the database **read-only** (`SQLITE_OPEN_READ_ONLY`
    and `PRAGMA query_only`) and has no chain access. Every route, status, body, header, ETag/304 rule,
    canonical 301, rate limit and timeout of 0.0.11 is ported; responses are byte-identical to 0.0.11's
    (checked against the live instance).
- **Two processes.** One image runs as two containers of one pod (workspace `post-bootstrap/sui-indexer`):
  the API's volume is mounted read-only, so a compromised API cannot write the index, and each side restarts
  on its own. `/healthz` reads the ingester's per-stream heartbeat rows (`meta poll:<stream>`, same 5 ×
  `POLL_MS` rule and JSON). Before the ingester has created the database the API answers `503` (and
  `/readyz` 503).
- **"One parser" without decoding.** The ingester derives `kind` from the event type and `gate_id` /
  `commission_mist` from the full node's `json` rendering; it stores the BCS bytes unchanged and never decodes
  them (the client packages do). Checked against the 0.0.11 path (the client parsers on the BCS) on every
  testnet event of the current and two earlier packages: 19/19 identical, and the same ledger cursor.
- **Data continuity.** Same schema, generation and cursor encoding (base64 of the watermark cursor bytes): an
  existing database continues without a reindex and old cursors keep paging. On its first start against a
  database written by 0.0.11 the ingester copies it to `<DB_PATH>.bak-pre-rust-<ms>` (`VACUUM INTO`) before
  writing, and records `meta schema_writer = rust`.
- The runtime image is `gcr.io/distroless/cc-debian13:nonroot` (uid 65532) with `/app/api` (default command)
  and `/app/ingest`; release binaries are 2.9 MB and 5.7 MB. Measured: about 5 MB RSS idle and 8 MB at
  ~18k req/s for the API (0.0.11: about 110 MB at 3.5k req/s), 8 MB for the ingester.

### Added

- The API serves at most 1024 connections at once and buffers at most 32 KiB of request head (request
  bodies are never read), so its memory stays bounded whatever the connection count.
- `ingest --healthcheck`, the container's exec liveness probe (`meta ingest:alive`, refreshed around every
  full-node request).
- CI `rust-ci.yml`: fmt, clippy `-D warnings`, tests, coverage, cargo-deny, MSRV (1.88), the client-contract job
  (the published `@meddleware/access-gate-client` and `@meddleware/seal-client` against the built `api`
  binary; test tooling only), licence notices (cargo-about), Trivy configuration and image scans, and a
  two-container smoke test (read-only API mount).
- The release attests a CycloneDX SBOM per binary (cargo-cyclonedx), checked against the `Cargo.lock`
  dependency graph by `scripts/check-sbom.sh` (bash + jq), next to the image's SPDX SBOM.

### Removed

- `package.json`, the lockfile, the TypeScript sources and tests, ESLint/TypeScript/Vitest configuration,
  `node-ci.yml`, `scripts/check-sbom.mjs` and `scripts/third-party-licenses.mjs` (git history keeps them).

## [0.0.11] - 2026-10-19

### Changed

- **Runs on Node 26.** The runtime image is `gcr.io/distroless/nodejs26-debian13:nonroot` (was nodejs24) and the
  builder is `node:26-trixie-slim`; CI runs on Node 26.12. Node 26 becomes LTS on 2026-10-28; the image tags and the
  digests we pin do not change at that promotion. `engines.node` stays `>=24`. The full suite (53 tests, `node:sqlite`
  ingest and API) passes on Node 26, and the container smoke test runs in CI on the new image.

## [0.0.10] - 2026-10-18

### Added

- The release attests a second SPDX SBOM of the bundled production npm dependencies (`npm sbom --omit dev`),
  because the image SBOM sees only OS packages; `scripts/check-sbom.mjs` checks it against the lockfile in CI
  and in the release (a missing package fails the build)

### Changed

- `noUncheckedIndexedAccess` is on; the two guarded sites in `src/` and five in tests are fixed
- Operator-keyed lookups (`NETWORK` to default gRPC URL and chain id) use `Map`s, so inherited names such as
  `constructor` are not networks
- `@meddleware/access-gate-client` ^0.0.9, `@meddleware/seal-client` ^0.0.20

## [0.0.9] - 2026-10-09

### Changed

- Merged tooling and workflow updates; clients access-gate-client ^0.0.8 and seal-client ^0.0.19

## [0.0.8] - 2026-10-09

### Changed

- Distroless runtime base rebuilt on a digest without the fixable libssl3t64 HIGH advisories (the image scan gate blocked 0.0.7)

## [0.0.7] - 2026-10-09

### Changed

- Depends on access-gate-client ^0.0.8 and seal-client ^0.0.19; indexes the 2026-10-09 testnet publications (rebinding needs REINDEX_CONFIRM, see README)
- CI and release gate: the tag workflow reuses `Node CI` on the tagged commit, which now also scans the
  Dockerfile configuration and the built image with Trivy, and smoke-tests the image
  (`scripts/smoke.sh`: non-root, `/readyz`, security headers, `/healthz` 503 before any poll). The
  published image is scanned before it is signed; the version label drops the leading `v`; the
  `cosign verify` command is pinned to the workflow identity in the run summary.
- `.dockerignore` excludes `.env*` and local database files from the build context.

### Documented

- A package upgrade that adds an event struct is not indexed: the struct is typed at the upgrade's
  package id, while the module filter and the parsers use the original id. New event types therefore
  need a fresh publish (the project republishes before v0.2 anyway); see CLAUDE.md.

## [0.0.6] - 2026-10-08

### Changed

- Fix: startup compared the node's base58 chain identifier with the short id and refused testnet; it now maps the digest to its first four bytes (0.0.5 never became ready)

## [0.0.5] - 2026-10-08

### Fixed

- **The indexer no longer wedges when its cursor falls below the full node's retention.** A stream
  whose cursor stopped advancing (no events for days) retried the same pruned cursor forever
  ("requested data below earliest available") and `/healthz` stayed red. It now restarts from the
  node's earliest data and counts a `gap`. Streams are polled independently, so one failing no longer
  stops the other.
- An event that does not decode no longer halts ingest: it is counted (`undecodable`), logged loudly
  and skipped.

### Changed

- Coverage, gate stats and commission come from counters kept in the ingest transaction instead of
  full-table scans (a coverage read was ~150 ms at 300k rows on the single request thread). Older
  databases are rebuilt once at startup.
- A package change no longer deletes history on start: it stops startup until `REINDEX_CONFIRM` names the
  stream, backs the database up first (`VACUUM INTO`) and then clears it.
- `/healthz` reports each stream (`ok`, `lastPollAgeMs`, `lastError`); head pages carry `latestCheckpoint`
  and `polledAt`; `/coverage` adds `undecodable` and `gaps`.
- Non-canonical spellings of a query get a cacheable `301` to the canonical form (one cache entry per
  query); IPv6 clients share a rate-limit bucket per /64; the bucket map is capped; weak, listed and
  wildcard `If-None-Match` revalidate.
- Startup refuses a full node whose chain identifier does not match `NETWORK`; the node client is no
  longer cast to the slice the indexer uses, so tsc checks it.
- The image ships `package.json` + `package-lock.json` (so SBOM scanners see the bundled npm packages)
  and `THIRD_PARTY_LICENSES`; CI checks the notices.
- `@meddleware/seal-client` ^0.0.16, `@meddleware/access-gate-client` ^0.0.6.

## [0.0.4] - 2026-10-03

### Fixed

- Every full-node `listEvents` call has a 30 s timeout (`requestTimeoutMs`). A hung call used to stall
  ingest indefinitely: liveness probes `/readyz`, which stays green while serving, so nothing restarted
  the pod.
- `ACCESS_GATE_ORIGINAL_ID` and `SEAL_ORIGINAL_ID` overrides must be `0x` + hex and are normalised, so
  a differently cased value no longer reads as a package change (which clears and re-indexes).

### Changed

- `@meddleware/access-gate-client` 0.0.4 and `@meddleware/seal-client` 0.0.15.

## [0.0.3] - 2026-10-02

### Changed

- `@meddleware/access-gate-client` 0.0.3 and `@meddleware/seal-client` 0.0.13 (event decoding is
  unchanged; the clients' parsers now fail closed and the indexer reader is shared).

## [0.0.2] - 2026-10-02

### Added

- **Package binding.** The index records which package each stream reads. When the configured
  package changes (the version-gated republish of `access_gate` and `seal_policies`), that stream is
  cleared and re-indexed from the new package's first event, and the generation rotates so older API
  cursors are refused (`410`).

### Changed

- `@meddleware/access-gate-client` `^0.0.2` and `@meddleware/seal-client` `^0.0.11`: the default
  packages are the version-gated testnet `access_gate` `0xa55789…` and `seal_policies` `0x61c4aa…`;
  `PlatformMigrated` events are indexed (they concern no gate).

## [0.0.1] - 2026-09-30

### Added

- **Ingest.** Polls the `access_gate` module events and `sealed_content::SealedContentPublished`
  over gRPC into SQLite (WAL).
  - Reads each stream ascending from a stored cursor.
  - Stores each page and its cursor in one transaction.
  - Decodes with `@meddleware/access-gate-client` and `@meddleware/seal-client`.
- **API.**
  - `/healthz`.
  - `/v1/{network}/access-gate/events`, `/v1/{network}/sealed-content`,
    `/v1/{network}/gates/{id}/stats` and `/v1/{network}/coverage`.
  - Immutable older pages; a short-TTL newest page with an ETag.
  - Generation-scoped cursors, strict validation, per-IP rate limit, CORS `*`.
- **Image.** One esbuild bundle on distroless Node 24, nonroot.
