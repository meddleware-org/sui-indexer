# sui-indexer

A lean read-indexer for Meddleware's Sui packages. It polls the `access_gate` events and
`sealed_content::SealedContentPublished` from a Sui full node over gRPC, stores them in SQLite, and
serves cacheable GET pages. Apps use it to list history cheaply and fall back to the full node when
it is unavailable.

- **One process.** Node 24, `node:sqlite` (WAL) and `node:http`, shipped as one esbuild bundle on
  distroless. About 110 MB RSS at 3.5k req/s on a laptop.
- **Same decoding as the apps.** Events are decoded with `@meddleware/access-gate-client` and
  `@meddleware/seal-client`. Rows are served verbatim (BCS in base64), and the clients decode and
  type-check them again.
- **Cheap under load.**
  - Older pages never change and are served `immutable`.
  - The newest page is cached 10 s in browsers and 30 s at the edge.
  - Behind Cloudflare's cache, the home server answers only cache misses.
  - A per-IP token bucket keyed on `CF-Connecting-IP` limits what gets through.
- **Display-only.** Nothing may use it to authorise. Every row carries `transactionDigest` and
  `checkpoint`, so it can be checked against a full node.

## API

All routes are `GET`, and responses are JSON with `Access-Control-Allow-Origin: *`.

| Route | Returns |
| --- | --- |
| `/healthz` | `200` while EVERY stream's polls succeed (the last within 5 intervals), else `503`, with `{ ok, streams: { <stream>: { ok, lastPollAgeMs, lastError } } }`; for monitoring |
| `/readyz` | `200` while the server runs; for Kubernetes probes, so stale data keeps being served while the full node is down |
| `/v1/{network}/access-gate/events?kinds&gate&before&limit` | `{ events, cursor, indexedFromCheckpoint }` (head pages also `latestCheckpoint` and `polledAt`), newest first; `limit` ≤ 100 (default 20); `kinds` is a comma list of `GateCreated`, `AccessMinted`, `AccessConsumed`, `AccessBurned`, `GateFrozen`, `GateMadeFree`, `PlatformConfigUpdated` |
| `/v1/{network}/sealed-content?gate&before&limit` | the same shape for one gate (`gate` required); `limit` ≤ 200 (default 50) |
| `/v1/{network}/gates/{id}/stats` | `{ minted, consumed, burned, commissionMist, indexedFromCheckpoint }` |
| `/v1/{network}/coverage` | oldest and newest indexed checkpoints, event counts, skipped-undecodable counts and pruned-cursor restarts (`gaps`) per stream |

- **Events.** Each event is the full node's entry: `eventType`, `sender`, `bcs` (base64),
  `checkpoint`, `transactionDigest` and `eventIndex`.
- **Cursors.** `cursor` is `<generation>.<row id>`. A rebuilt database gets a new generation, and
  an older cursor then returns `410`.
- **Package changes.** Changing a stream's package (`ACCESS_GATE_ORIGINAL_ID`, `SEAL_ORIGINAL_ID` or the
  client defaults) would mean deleting the stored history, which the full node may already have
  pruned. The indexer therefore **refuses to start** until you set `REINDEX_CONFIRM=<stream>[,<stream>]`;
  it then copies the database to `<DB_PATH>.bak-<time>` (`VACUUM INTO`), clears that stream, re-indexes
  it from the new package's first event and rotates the generation. Keep the volume snapshotted.
- **Validation.** Unknown query parameters, bad ids and out-of-range limits return `400`.
- **Canonical URLs.** Another spelling of the same query (a short or upper-case `gate`, `limit=05`,
  unsorted or repeated `kinds`, an upper-case id in the stats path) gets a cacheable `301` to the
  canonical form, so each query has one cache entry. The client packages already send canonical
  values; only the order of parameters is left as sent (a bounded number of permutations).
- **Freshness.** Head pages carry `polledAt` (epoch ms of the stream's last successful poll) and
  `latestCheckpoint`. Older pages are immutable and carry neither.

## Configuration

| Variable | Default |
| --- | --- |
| `NETWORK` | `testnet` (the one network this instance serves) |
| `GRPC_URL` | the public Mysten full node for `NETWORK` (https required) |
| `DB_PATH` | `/data/indexer.db` |
| `PORT` | `8080` |
| `POLL_MS` | `15000` |
| `ACCESS_GATE_ORIGINAL_ID` / `SEAL_ORIGINAL_ID` | the recorded deployments in the client packages |
| `RATE_LIMIT_PER_SEC` / `RATE_LIMIT_BURST` | `10` / `40` per client IP (IPv6 clients by /64; at most 50 000 buckets are kept) |
| `REINDEX_CONFIRM` | unset: a package change stops startup (see Cursors) |

- **First start.** The indexer backfills from the oldest event the full node still serves.
  History before that is not listed; `indexedFromCheckpoint` says where coverage starts.
- **Streams.** Each is read with one filter: the `access_gate` module, or the one sealed-content
  event.
- **Crash safety.** A page and its cursor are stored in one transaction.
- **Independent streams.** One stream failing never stops the other; each reports its own health.
- **Undecodable events** are counted, logged and skipped (the page advances past them).
- **Pruned cursor.** If the stored cursor falls below what the full node still serves (no event arrived
  to move it before the node pruned), the stream restarts from the node's earliest data and counts a
  `gap`; the events in between are gone for good.
- **Counters.** Coverage, gate stats and commission are maintained at ingest, so serving a page never
  scans a stream (a scan took ~150 ms at 300k rows).
- **Chain identity.** At startup the node's chain identifier must match `NETWORK` (testnet, mainnet).
- **Image contents.** The image carries `package.json`, `package-lock.json` (so SBOM tools see the
  bundled packages) and `THIRD_PARTY_LICENSES`.

## Development

```bash
npm ci
npm run type-check && npm run lint && npm test
npm run build && DB_PATH=./dev.db PORT=8080 npm start
```

Container image: `quay.io/meddleware-org/sui-indexer`, built by `docker-publish.yml` on a `v*` tag.
The tag must match `package.json`. Deployment manifests live in the infrastructure workspace
(`post-bootstrap/sui-indexer`).

## Licence

BSD Zero Clause (`0BSD`) — see [LICENSE](LICENSE).
