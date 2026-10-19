# sui-indexer

A lean read-indexer for Meddleware's Sui packages. It polls the `access_gate` events and
`sealed_content::SealedContentPublished` from a Sui full node over gRPC, stores them in SQLite, and
serves cacheable GET pages. Apps use it to list history cheaply and fall back to the full node when
it is unavailable.

- **Two processes, one image.** Rust, shipped as `/app/ingest` and `/app/api` on distroless
  (`cc-debian13:nonroot`); the cluster runs them as two containers of one pod sharing the data volume.
  - `ingest` is the **only writer**. It reads `ListEvents` with Mysten's `sui-rpc` client and writes
    SQLite (WAL, bundled with `rusqlite`).
  - `api` opens the database **read-only** (the volume is also mounted read-only), has no chain access and
    serves HTTP/1.1 with `hyper`. It learns how fresh the index is from the ingester's heartbeat rows.
  - Measured 2026-10-19 on a 4-CPU laptop (release build, one-off, not a benchmark): the API at about 5 MB
    RSS idle and 8 MB serving ≈ 18k req/s of the head page; 26 MB peak and ≈ 14k req/s against a 300k-row
    index; the ingester about 8 MB. Request cost does not grow with history.
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
| `/readyz` | `200` while the API can open the database; for Kubernetes probes, so stale data keeps being served while the full node or the ingester is down. `503` before the ingester has created the database. |
| `/v1/{network}/access-gate/events?kinds&gate&before&limit` | `{ events, cursor, indexedFromCheckpoint }` (head pages also `latestCheckpoint` and `polledAt`), newest first; `limit` ≤ 100 (default 20); `kinds` is a comma list of `GateCreated`, `AccessMinted`, `AccessConsumed`, `AccessBurned`, `GateFrozen`, `GateMadeFree`, `PlatformConfigUpdated`, `PlatformMigrated` |
| `/v1/{network}/sealed-content?gate&before&limit` | the same shape for one gate (`gate` required); `limit` ≤ 200 (default 50) |
| `/v1/{network}/gates/{id}/stats` | `{ gateId, minted, consumed, burned, commissionMist, indexedFromCheckpoint }` (`commissionMist` a decimal string) |
| `/v1/{network}/coverage` | oldest and newest indexed checkpoints, event counts, skipped-undecodable counts and pruned-cursor restarts (`gaps`) per stream |

- **Events.** Each event is the full node's entry: `eventType`, `sender`, `bcs` (base64),
  `checkpoint`, `transactionDigest` and `eventIndex`.
- **Cursors.** `cursor` is `<generation>.<row id>`. A rebuilt database gets a new generation, and
  an older cursor then returns `410`.
- **Before the database exists** (a fresh volume, until the ingester's first start), `/v1` routes and
  `/healthz` answer `503` with `no-store`.
- **Package changes.** Changing a stream's package (`ACCESS_GATE_ORIGINAL_ID`, `SEAL_ORIGINAL_ID` or the
  recorded defaults) would mean deleting the stored history, which the full node may already have
  pruned. The ingester therefore **refuses to start** until you set `REINDEX_CONFIRM=<stream>[,<stream>]`;
  it then copies the database to `<DB_PATH>.bak-<time>` (`VACUUM INTO`), clears that stream, re-indexes
  it from the new package's first event and rotates the generation. Keep the volume snapshotted.
- **Validation.** Unknown query parameters, bad ids and out-of-range limits return `400`.
- **Canonical URLs.** Another spelling of the same query (a short or upper-case `gate`, `limit=05`,
  unsorted or repeated `kinds`, an upper-case id in the stats path) gets a cacheable `301` to the
  canonical form, so each query has one cache entry. The client packages already send canonical
  values; only the order of parameters is left as sent (a bounded number of permutations).
- **Freshness.** Head pages carry `polledAt` (epoch ms of the stream's last successful poll) and
  `latestCheckpoint`. Older pages are immutable and carry neither.
- **Transport.** 5 s to receive the request headers, 10 s per request, at most 1024 open connections and
  a 32 KiB request head; `OPTIONS` preflight answered, other methods `405`.

## Configuration

| Variable | Read by | Default |
| --- | --- | --- |
| `NETWORK` | both | `testnet` (the one network this instance serves) |
| `DB_PATH` | both | `/data/indexer.db` |
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
- **Crash safety.** A page and its cursor are stored in one transaction.
- **Independent streams.** One stream failing never stops the other; each reports its own health.
- **Undecodable events** — a recognised event without what its row needs (no JSON rendering or field, no
  BCS, no position) — are counted, logged and skipped (the page advances past them).
- **Pruned cursor.** If the stored cursor falls below what the full node still serves (no event arrived
  to move it before the node pruned), the stream restarts from the node's earliest data and counts a
  `gap`; the events in between are gone for good.
- **Counters.** Coverage, gate stats and commission are maintained at ingest, so serving a page never
  reads a whole stream for them.
- **Chain identity.** At startup the node's chain identifier must match `NETWORK` (testnet, mainnet).
- **Liveness.** `ingest --healthcheck` exits 0 while the ingester refreshed `meta ingest:alive` within
  `5 × POLL_MS + 30 s`; it is the container's exec probe (distroless has no shell).
- **Upgrade from 0.0.11 (TypeScript).** Same schema, generation and cursor encoding: no reindex. The first
  start copies the old database to `<DB_PATH>.bak-pre-rust-<ms>` and records `meta schema_writer = rust`.
- **Image contents.** `/app/api`, `/app/ingest`, `Cargo.lock` (for scanners), `THIRD_PARTY_LICENSES-api.txt`
  and `THIRD_PARTY_LICENSES-ingest.txt` (cargo-about) and `LICENSE`.
- **SBOMs.** Each release attests three to the image digest: the image (SPDX, syft) and one CycloneDX SBOM
  per binary (cargo-cyclonedx), checked against the `Cargo.lock` graph by `scripts/check-sbom.sh`.
  `cosign verify-attestation --type spdxjson|cyclonedx <image>@<digest>` returns them.

## Development

```bash
cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
cargo deny check                                  # advisories, licences, bans, sources
cargo build --release --locked
DB_PATH=./dev.db ./target/release/ingest &        # the writer (testnet by default)
DB_PATH=./dev.db PORT=8080 ./target/release/api   # the read API
bash scripts/smoke.sh <image>                     # both binaries in containers (DOCKER=podman works)
```

The client contract (`tests/contract`, CI only) runs the published client packages against the built
`api` binary: `cargo build --release -p sui-indexer-api -p sui-indexer-testkit && cd tests/contract && npm ci && npm test`.

Container image: `quay.io/meddleware-org/sui-indexer`, built by `docker-publish.yml` on a `v*` tag.
The tag must match the Cargo workspace version. Deployment manifests live in the infrastructure workspace
(`post-bootstrap/sui-indexer`).

## Licence

BSD Zero Clause (`0BSD`) — see [LICENSE](LICENSE).
