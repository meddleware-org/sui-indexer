# CLAUDE.md — sui-indexer

A read-indexer for display data. See [README.md](README.md) for the API and configuration.

## Invariants

- **Display-only.**
  - No endpoint may be used, or documented, as an authorisation source.
  - Rows are the full node's events verbatim, and the clients decode and type-check them again.
  - Clients fall back to the full node.
- **No JavaScript/TypeScript backend.** The service is Rust. TypeScript appears only in
  `tests/contract`, the CI job that drives the published client packages against the built binary.
- **One parser.** BCS is decoded only by `@meddleware/access-gate-client` and `@meddleware/sealed-storage-client`.
  The ingester never decodes BCS and never copies their struct layouts (the BCS writer in
  `crates/testkit` encodes test fixtures only). It derives the stored columns this way:
  - `kind` from the event type: the address must be the stream's package **original id**, the module
    `access_gate` (or `sealed_content` with the struct `SealedContentPublished`), and the struct name
    one of the kinds below; anything else is unrecognised and skipped.
  - `gate_id` and `commission_mist` from the full node's `json` rendering (`read_mask` includes `json`).
  - The BCS bytes are stored unchanged.
- **JSON fields relied on** (the Move field names; checked 2026-10-19 against
  `access-gate-sui/sources/access_gate.move`, `sealed-storage-sui/sources/sealed_content.move` and live
  testnet events):

  | Struct (kind) | `gate_id` (`ID` → `"0x…"` hex) | `commission_mist` (`u64` → decimal string) |
  | --- | --- | --- |
  | `GateCreatedEvent` (`GateCreated`) | yes | — |
  | `AccessMintedEvent` (`AccessMinted`) | yes | yes |
  | `AccessConsumedEvent` (`AccessConsumed`) | yes | — |
  | `AccessBurnedEvent` (`AccessBurned`) | yes | — |
  | `GateFrozenEvent` (`GateFrozen`) | yes | — |
  | `GateMadeFreeEvent` (`GateMadeFree`) | yes | — |
  | `PlatformConfigUpdatedEvent` (`PlatformConfigUpdated`) | none (gate `NULL`) | — |
  | `PlatformMigratedEvent` (`PlatformMigrated`) | none (gate `NULL`) | — |
  | `SealedContentPublished` (`SealedContentPublished`) | yes | — |

  - **Verified live (testnet, read-only gRPC, 2026-10-19):** `GateCreatedEvent`, `AccessMintedEvent`
    and `AccessConsumedEvent` of `access_gate` `0xd7ddaa94…` (current) and the earlier `0xa55789d7…` and
    `0x1a81ca17…` (archive): 19 events, every one the node still serves. The JSON-derived columns matched
    the 0.0.11 path (the client parsers on the BCS) on 19/19. No `SealedContentPublished`,
    `AccessBurned`, `GateFrozen`, `GateMadeFree` or `PlatformConfigUpdated` event exists on testnet for
    these packages; for them the field names come from the Move sources (the node's rendering uses Move
    field names, as the verified kinds show).
  - **Parity re-run 2026-10-22:** all nine kinds, 43 events, identical to the client parsers' decoding of the BCS and to the node's `json`
    (34 access_gate events and 2 pointers on the live index; 7 on a scratch publication for `GateMadeFree` and `PlatformMigrated`). See the audit, F24.
  - A recognised event whose rendering lacks the field (or has no BCS or position) is **undecodable**:
    counted in `stream_stats.undecodable`, logged, skipped. A commission above 63 bits is undecodable
    too (the column is a `bigint`; commission is at most 10% of a `u64` price).
  - A Move change that renames `gate_id` or `commission_mist` breaks this mapping: update the table, the
    decoder (`crates/ingest/src/decode.rs`) and its tests together, and re-run the parity check.
- **Ledger order = row id.**
  - Ingest reads each stream ascending, and a page and its cursor commit in one transaction.
  - `events.id` is `GENERATED ALWAYS AS IDENTITY`, written by ONE connection that holds the writer lock, so ids
    follow ledger order and a reader never sees a higher id without every lower id that will ever exist. That is
    what makes a page bounded by `before` immutable (cached for a year).
  - The invariant is **order, not gapless**: an `ON CONFLICT DO NOTHING` duplicate and a rolled-back page consume
    identity numbers. No code may read an id as a count (the ETag is `"<generation>-<newest id>"`, cursors are
    `<generation>.<id>`, counts come from the counters).
  - Never insert rows out of order: no parallel backfill, no second writer, no descending reads into the table,
    no explicit ids (`OVERRIDING SYSTEM VALUE`).
- **Cursor encoding.** The stored cursor is the base64 (standard, padded) of the full node's watermark
  cursor bytes, with the page semantics of `@mysten/sui`'s `core.listEvents` (`limit + 1` look-ahead,
  `SCAN_LIMIT`/`ITEM_LIMIT` resume from the frontier). Changing it orphans every stored cursor.
- **Generation.** A new database gets a new random generation. Cursors carry it and are refused
  (`410`) across generations, so cached pages of an old database stay self-consistent.
- **Schema identity.** `crates/store/migrations/0001_init.sql` is the schema (tables, columns, meta keys, counter
  semantics) with PostgreSQL types and constraints: `bytea`,
  `bigint`, `numeric(39,0)` for commission totals, `NOT NULL` and `CHECK` where the code relies on them. The
  poll and liveness rows moved from `meta` to a `heartbeats` table (hot, heap-only updates). Migrations are
  versioned (`schema_migrations`, SHA-256 each), embedded, applied by the ingester as the owner under
  `pg_advisory_xact_lock`, and never edited after release; each must keep the previous release's queries
  working (the API of the previous image runs while the new ingester migrates). The API cannot run DDL and
  answers `503` until `schema_migrations` reaches the version it needs.
- **Indexes match the page query.** `events_stream (stream, id)`, `events_gate (stream, gate_id, id)` and
  `events_kind (stream, kind, id)` are created by migration 1. `read::page_sql` picks the shape: a gate uses
  `events_gate` (`(kind || '')` keeps the planner off `events_kind`), one kind uses `events_kind`, several kinds
  run one bounded seek per kind merged by `UNION ALL` (a single `kind IN (…)` would walk the stream). The events
  response is ONE statement (`read::events_sql`: facts + the page as a lateral join). Pooled API connections force
  `plan_cache_mode = force_custom_plan`: the generic plan of the one-kind query walks the primary key. A new
  filter shape or index needs a query-plan test in `crates/store/src/index_tests.rs`.
- **Package binding.** `Store::bind_packages` records the original id each stream indexes. A stream
  whose package changed (a fresh publish) — or that holds rows from before binding — is NOT cleared
  silently: startup fails with `PackageChangedError` until `REINDEX_CONFIRM` names the stream, then the
  whole index is copied into a `bak_<epoch ms>` schema (`CREATE TABLE … AS`, in the same transaction as the
  clearing), the stream cleared and the generation rotated. Never mix two packages' events in a stream, and
  never delete history without that confirmation.
- **New event types need a fresh publish.** The module filter and the client parsers match the
  package's *original* id. An event struct added by a later compatible upgrade is typed at the
  upgrade's id, so this indexer would not see it (and would not log it: the node never returns it).
  Adding one means a fresh publish and a `REINDEX_CONFIRM` rebind, not an upgrade.
- **Never stall on one bad input.** Streams are polled independently; an undecodable event is counted
  and skipped; a cursor below the node's retention restarts from its earliest data (a counted gap).
  `/healthz` reports each stream.
- **Counters, not scans.** `stream_stats`, `gate_stats` and `gate_commission` are updated in the ingest
  transaction. There is no backfill (the schema is created with them and every row enters through `ingest`;
  `Store::verify_counters` recounts). No request path may scan a
  stream for them, and a `kinds` page reads its own rows through `events_kind`.
- **Canonical URLs.** Non-canonical spellings of a query get a cacheable `301`; the cache-busting
  protection is `400` for unknown parameters plus this.
- **Cache headers are the cost model.**
  - Older pages: `public, max-age=31536000, immutable`.
  - The newest page: `public, max-age=10, s-maxage=30`, with an ETag.
  - Keep `Access-Control-Allow-Origin: *` (public read-only data). Cloudflare ignores `Vary`, so
    a reflected origin would be cached for the wrong site.
- **Strict input.** Unknown query parameters, bad ids and limits get `400`. That also stops
  cache-busting parameters from reaching the origin as new cache keys.
- **Two Deployments, two roles, single writer.**
  - `ingest` is the only process that writes. It connects as the database OWNER (`sui_indexer_ingest`), applies
    migrations, and takes `pg_try_advisory_lock(WRITER_LOCK_KEY)` on its one connection for the life of the
    process; a second writer exits non-zero. A lost connection loses
    the lock, so `run_loop` ends with an error and the process exits non-zero.
  - `api` connects as the `SELECT`-only role (`sui_indexer_api`: no write grant, `default_transaction_read_only`),
    each pooled connection additionally `SET default_transaction_read_only = on`. It never migrates, never
    creates anything, and answers `503` until the schema exists; it has no chain access.
  - Freshness reaches the API only through the heartbeat rows.
  - No SQL outside `crates/store`. No `unsafe` anywhere (`unsafe_code = "deny"` with no exception).
  - TLS to PostgreSQL is `verify-full` only (`DATABASE_URL` with `sslrootcert`; `ALLOW_INSECURE_DATABASE=1` is for
    local tests). The URL and password are never logged or put in an error.
- **Storage is the shared PostgreSQL tier** (D59 revised, D60; OQ7): own database `sui_indexer`, roles
  `sui_indexer_ingest` (owner) and `sui_indexer_api` (SELECT), provisioned by the database tier. Shared fate with
  the auth Postgres is accepted (display data degrades, authorisation does not: clients fall back to the full
  node); durability is the tier's backup (daily `pg_dump`; only the offsite copy is a backup on one node).
- **Lean.**
  - Rust: `tokio-postgres` (+ `deadpool-postgres` for the API's pool, `tokio-postgres-rustls` on rustls with the
    **ring** provider the gRPC client already uses: one TLS stack), `hyper` + `hyper-util` (no web framework),
    `sui-rpc` (Mysten's SDK) for gRPC, `serde_json`. No ORM, no async runtime beyond tokio.
  - Distroless `cc` runtime, nonroot, no shell. Release profile: overflow checks on, `panic = "abort"`.

## Tests

`cargo test --workspace --locked` needs a PostgreSQL 18: `TEST_DATABASE_URL` (a superuser URL; `scripts/test-postgres.sh`
starts a pinned TLS one, `DOCKER=podman` works). Each test creates its own database and the two production roles
(`sui_indexer_store::testdb`), so the API's tests really run as the SELECT-only role. Without the variable they print
`SKIPPED` and return; CI sets `REQUIRE_TEST_DATABASE=1` (a missing server fails) and fails on any ignored test.

- **store** — schema and constraints, atomic ingest, ids ordered-not-gapless, counters and their recount, package
  binding and the `bak_` backup (atomic with the clearing), migrations (once, racing migrators, checksum, newer
  schema), the writer lock, role separation (the API role cannot write, change the schema or read a backup schema;
  one service's roles cannot connect to another's database), the reader (readiness, one-snapshot responses,
  cancellation, unreachable database), TLS against a wrong CA, URL/config parsing (nothing prints the password);
  the query plan of every filter shape (custom plan, buffer counts on 300,000 rows), merged kind seeks against the
  plain filter, custom plans on pooled connections (`index_tests.rs`; `INDEX_MEASURE=1` prints pages/s);
- **ingest** — JSON-derived columns, the poll loop against a scripted full node, gap recovery, page assembly from
  stream frames (with proptests), the real `sui-rpc` client against a local LedgerService stub, the startup wiring
  (chain check, a second ingester refused, package refusal and the `bak_` copy, the loop, a lost connection, the
  healthcheck);
- **api** — every case of the 0.0.11 handler tests, URL parsing and canonicalisation (with proptests), the database
  lifecycle (`503` before the schema exists, the API creates nothing, `503 database unavailable` and recovery, a
  server-closed connection, a timed-out request), the socket wiring;
- **config, chain ids and the rate limiter.**

CI also runs the **client contract** (`tests/contract`: the published client packages against the built `api`
binary and a seeded PostgreSQL), the SBOM gate tests (`scripts/tests/check-sbom-test.sh`) and the container smoke
test (`scripts/smoke.sh`: PostgreSQL with TLS, the ingester and the API as separate containers, role separation,
a second ingester refused, database outage and recovery).
Before a release, run both binaries locally against testnet (README → Development).

## Release

`docker-publish.yml` (tag `v*`): `version` and `verify` (the full `rust-ci.yml`), then per architecture a native
build (amd64 on `ubuntu-latest`, arm64 on `ubuntu-24.04-arm`, never QEMU) to an OCI archive with no registry
credentials, `publish-arch` pushes `<commit>-<arch>`, `merge-docker-public` merges the index, runs the Trivy scan
by digest, signs, attests (image SPDX, the two CycloneDX per-binary SBOMs from `sbom-crates`, build provenance)
and exports `digest`; `mirror-private` promotes that digest. The `<commit>-<arch>` tags are intermediates and stay.
`scripts/check-index.sh` asserts the merged index holds both architectures and their provenance. Running the
workflow by hand is a dry run (`dry_run` input, default on): nothing is pushed or signed. Do not reintroduce
`setup-qemu-action` for a Rust compile.
