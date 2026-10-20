# CLAUDE.md — sui-indexer

A read-indexer for display data. See [README.md](README.md) for the API and configuration.

## Invariants

- **Display-only.**
  - No endpoint may be used, or documented, as an authorisation source.
  - Rows are the full node's events verbatim, and the clients decode and type-check them again.
  - Clients fall back to the full node.
- **No JavaScript/TypeScript backend.** The service is Rust. TypeScript appears only in
  `tests/contract`, the CI job that drives the published client packages against the built binary.
- **One parser.** BCS is decoded only by `@meddleware/access-gate-client` and `@meddleware/seal-client`.
  The ingester never decodes BCS and never copies their struct layouts (the BCS writer in
  `crates/testkit` encodes test fixtures only). It derives the stored columns this way:
  - `kind` from the event type: the address must be the stream's package **original id**, the module
    `access_gate` (or `sealed_content` with the struct `SealedContentPublished`), and the struct name
    one of the kinds below; anything else is unrecognised and skipped.
  - `gate_id` and `commission_mist` from the full node's `json` rendering (`read_mask` includes `json`).
  - The BCS bytes are stored unchanged.
- **JSON fields relied on** (the Move field names; checked 2026-10-19 against
  `access-gate-sui/sources/access_gate.move`, `seal-policies-sui/sources/sealed_content.move` and live
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
  - A recognised event whose rendering lacks the field (or has no BCS or position) is **undecodable**:
    counted in `stream_stats.undecodable`, logged, skipped. A commission above 63 bits is undecodable
    too (the column is an INTEGER; commission is at most 10% of a `u64` price).
  - A Move change that renames `gate_id` or `commission_mist` breaks this mapping: update the table, the
    decoder (`crates/ingest/src/decode.rs`) and its tests together, and re-run the parity check.
- **Ledger order = row id.**
  - Ingest reads each stream ascending, and a page and its cursor commit in one transaction.
  - That is what makes a page bounded by `before` immutable (cached for a year).
  - Never insert rows out of order: no parallel backfill, and no descending reads into the table.
- **Cursor encoding.** The stored cursor is the base64 (standard, padded) of the full node's watermark
  cursor bytes, with the page semantics of `@mysten/sui`'s `core.listEvents` (`limit + 1` look-ahead,
  `SCAN_LIMIT`/`ITEM_LIMIT` resume from the frontier). Changing it orphans every stored cursor.
- **Generation.** A new database gets a new random generation. Cursors carry it and are refused
  (`410`) across generations, so cached pages of an old database stay self-consistent.
- **Schema identity.** `crates/store` keeps the tables, pragmas, meta keys and counter semantics of the
  0.0.11 TypeScript store, so either version opens the other's database. New `meta` rows are additive
  (`poll:<stream>`, `ingest:alive`, `schema_writer`), and so is the one added index.
- **Indexes match the page query.** `events_stream (stream, id)`, `events_gate (stream, gate_id, id)` and
  `events_kind (stream, kind, id)` (0.0.13) are created by the ingester only (`SCHEMA`, all `IF NOT
  EXISTS`); the read-only API never creates one and must keep answering without `events_kind`. `read::page`
  (`page_sql`) picks the shape: a gate uses `events_gate` (`+kind` keeps the planner off `events_kind`),
  one kind uses `events_kind`, several kinds run one bounded seek per kind merged by `UNION ALL` (a
  single `kind IN (…)` makes SQLite walk the stream). A new filter shape or index needs a query-plan test
  in `crates/store/src/index_tests.rs`.
- **Package binding.** `Store::bind_packages` records the original id each stream indexes. A stream
  whose package changed (a fresh publish) — or that holds rows from before binding — is NOT cleared
  silently: startup fails with `PackageChangedError` until `REINDEX_CONFIRM` names the stream, then the
  database is backed up (`VACUUM INTO`), the stream cleared and the generation rotated. Never mix two
  packages' events in a stream, and never delete history without that confirmation.
- **New event types need a fresh publish.** The module filter and the client parsers match the
  package's *original* id. An event struct added by a later compatible upgrade is typed at the
  upgrade's id, so this indexer would not see it (and would not log it: the node never returns it).
  Adding one means a fresh publish and a `REINDEX_CONFIRM` rebind, not an upgrade.
- **Never stall on one bad input.** Streams are polled independently; an undecodable event is counted
  and skipped; a cursor below the node's retention restarts from its earliest data (a counted gap).
  `/healthz` reports each stream.
- **Counters, not scans.** `stream_stats`, `gate_stats` and `gate_commission` are updated in the ingest
  transaction (and rebuilt once for older databases). No request path may scan a stream for them, and a `kinds` page
  reads its own rows through `events_kind`.
- **Canonical URLs.** Non-canonical spellings of a query get a cacheable `301`; the cache-busting
  protection is `400` for unknown parameters plus this.
- **Cache headers are the cost model.**
  - Older pages: `public, max-age=31536000, immutable`.
  - The newest page: `public, max-age=10, s-maxage=30`, with an ETag.
  - Keep `Access-Control-Allow-Origin: *` (public read-only data). Cloudflare ignores `Vary`, so
    a reflected origin would be cached for the wrong site.
- **Strict input.** Unknown query parameters, bad ids and limits get `400`. That also stops
  cache-busting parameters from reaching the origin as new cache keys.
- **Two processes, single writer.**
  - `ingest` is the only process that writes the database. `api` opens it with `SQLITE_OPEN_READ_ONLY`
    and `PRAGMA query_only` on a read-only mount, and never creates it (it answers `503` until it exists).
  - Freshness reaches the API only through the heartbeat rows; the API has no chain access.
  - The writer keeps the `-wal`/`-shm` files on close (`SQLITE_FCNTL_PERSIST_WAL`), so the read-only API
    can open the database while the ingester is stopped. That one FFI call is the only `unsafe` code.
  - One replica with an RWO volume; both containers in one pod.
- **Storage is embedded SQLite, deliberately.** Not the shared PostgreSQL of the `database` namespace (D59, OQ6): that is one
  replica for the auth stack with no backup, so it adds no durability and couples this display-only index to auth. All SQL
  stays in `crates/store`, so a later move (2+ API replicas, or a shared HA PostgreSQL with PITR) is contained there.
- **Lean.**
  - Rust: `rusqlite` (bundled SQLite), `hyper` + `hyper-util` (no web framework), `sui-rpc` (Mysten's
    SDK) for gRPC, `serde_json`. No ORM, no async runtime beyond tokio.
  - Distroless `cc` runtime, nonroot, no shell. Release profile: overflow checks on, `panic = "abort"`.

## Tests

`cargo test --locked` runs:

- **store** — schema, atomic ingest, counters (and their backfill), package binding and backups, the
  cutover backup, the read-only view and the persisted WAL files; the `events_kind` migration, the query
  plan of every filter shape and the page cost of a rare kind on 300,000 rows (`index_tests.rs`; the
  `--ignored` measurement `f25_measurement_on_300k_rows` prints pages/s before and after);
- **ingest** — JSON-derived columns, the poll loop against a scripted full node, gap recovery, page
  assembly from stream frames (with proptests), the real `sui-rpc` client against a local LedgerService
  stub, and the startup wiring (chain check, package refusal, backups, loop, healthcheck);
- **api** — every case of the 0.0.11 handler tests, URL parsing and canonicalisation (with proptests),
  the database lifecycle (`503` before it exists), and the socket wiring (HEAD, timeouts, shutdown);
- **config, chain ids and the rate limiter.**

CI also runs the **client contract** (`tests/contract`: the published client packages against the built
`api` binary), the SBOM gate tests (`scripts/tests/check-sbom-test.sh`) and the two-container smoke test.
Before a release, run both binaries locally against testnet (README → Development).
