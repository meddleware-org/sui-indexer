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
| `/healthz` | `200` while polls succeed (the last within 5 intervals), else `503`; for monitoring |
| `/readyz` | `200` while the server runs; for Kubernetes probes, so stale data keeps being served while the full node is down |
| `/v1/{network}/access-gate/events?kinds&gate&before&limit` | `{ events, cursor, indexedFromCheckpoint }`, newest first; `limit` ≤ 100 (default 20); `kinds` is a comma list of `GateCreated`, `AccessMinted`, `AccessConsumed`, `AccessBurned`, `GateFrozen`, `GateMadeFree`, `PlatformConfigUpdated` |
| `/v1/{network}/sealed-content?gate&before&limit` | the same shape for one gate (`gate` required); `limit` ≤ 200 (default 50) |
| `/v1/{network}/gates/{id}/stats` | `{ minted, consumed, burned, commissionMist, indexedFromCheckpoint }` |
| `/v1/{network}/coverage` | oldest and newest indexed checkpoints and event counts per stream |

- **Events.** Each event is the full node's entry: `eventType`, `sender`, `bcs` (base64),
  `checkpoint`, `transactionDigest` and `eventIndex`.
- **Cursors.** `cursor` is `<generation>.<row id>`. A rebuilt database gets a new generation, and
  an older cursor then returns `410`.
- **Validation.** Unknown query parameters, bad ids and out-of-range limits return `400`.

## Configuration

| Variable | Default |
| --- | --- |
| `NETWORK` | `testnet` (the one network this instance serves) |
| `GRPC_URL` | the public Mysten full node for `NETWORK` (https required) |
| `DB_PATH` | `/data/indexer.db` |
| `PORT` | `8080` |
| `POLL_MS` | `15000` |
| `ACCESS_GATE_ORIGINAL_ID` / `SEAL_ORIGINAL_ID` | the recorded deployments in the client packages |
| `RATE_LIMIT_PER_SEC` / `RATE_LIMIT_BURST` | `10` / `40` per client IP |

- **First start.** The indexer backfills from the oldest event the full node still serves.
  History before that is not listed; `indexedFromCheckpoint` says where coverage starts.
- **Streams.** Each is read with one filter: the `access_gate` module, or the one sealed-content
  event.
- **Crash safety.** A page and its cursor are stored in one transaction.

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
