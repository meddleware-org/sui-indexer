# Security Audit — `sui-indexer`

**Classification:** Internal security review (re-verified 2026-10-09; code gates closed 2026-10-18; re-verified 2026-10-19 against the Rust rewrite 0.0.12 — awaiting external review)
**Project:** sui-indexer — a lean read-indexer for display data.

- It polls two streams from a Sui full node over gRPC into SQLite (WAL):
  - the `access_gate` module events;
  - `sealed_content::SealedContentPublished`.
- It serves cacheable GET pages behind Cloudflare.
- Apps (treasury-ui activity, seal-ui discovery) read history from it when `VITE_INDEXER_URL` is set,
  and fall back to the full node otherwise.

**Project type:** Rust service (from 0.0.12: a Cargo workspace with two binaries, `ingest` — the sole
SQLite writer — and `api` — read-only, `hyper` — in one image, run as two containers of one pod) +
container image. Up to 0.0.11 it was a TS service (Node `node:http` + `node:sqlite`, one esbuild bundle);
the findings keep their TS-era text and carry a 2026-10-19 re-verification against the Rust code.
**Template:**

- AUDIT_TEMPLATE.md (2026-10-08)
- AUDIT_TEMPLATE_RUST.md (2026-10-08) — from 0.0.12 (the service is Rust)
- AUDIT_TEMPLATE_SUI_CLIENT.md (2026-10-08) — chain reads: event filters, package IDs, type matching
- AUDIT_TEMPLATE_IMG.md (2026-10-08) — `Dockerfile` + `docker-publish.yml` + the cluster manifests

Not triggered: SUI (no Move code), SEAL, WALRUS, VUE, AUTH (no authentication: public read-only
data), OPS (no signing or irreversible scripts), GO, WORKERS, PROXY (it serves its own data; it
forwards nothing), SITE, PLATFORM (the cluster and edge are the platform's own review; this audit
cites them). TS applied up to 0.0.11 (the A16–A24 rows of that era are replaced by RUST rows); from
0.0.12 the only TypeScript is `tests/contract`, a CI-only project that runs the published client
packages against the binary (lockfile committed, Dependabot-tracked, never shipped).

**Deployment status:**

- **2026-10-19: `0.0.12` (Rust rewrite) committed locally, not released or deployed.** Commits `1fcbbe9`
  (rewrite), `76595ca`, `784250a` (CI, image, SBOM), `ddfc77d` (connection cap) and the docs commit on top of
  `fb79cd7`. The cluster still runs `0.0.11` (`sha256:c2497d4c…5abe`, the digest in `config/images.yaml`).
  The workspace manifests for the two-container pod are written (`post-bootstrap/sui-indexer`, decision D58)
  but must not be applied before the 0.0.12 digest is pinned (the overlay would run the 0.0.11 image as two
  containers). Rollout gate: Section D, pre-testnet.
- **Current (2026-10-18): `0.0.10`, released and deployed.** Tag `v0.0.10` = `5251f38` (code gates in
  `4aab344`). Index `sha256:b3484ce42a8484383efd598a3f21d3cb7077c7e3d4b73eb0df1c740b221d8a58` on quay.io and
  Docker Hub (same digest; pinned in `config/images.yaml` and the `post-bootstrap/sui-indexer` overlay). The cosign
  keyless signature was verified 2026-10-18 against the workflow-anchored identity (`docker-publish.yml@refs/tags/v*`,
  `bootstrap/images/verify-digests.sh`, 16/16 valid). **The 0.0.10 digest carries two SPDX attestations: the
  image SBOM (17 packages) and the npm packages SBOM (24 packages); both were verified with `cosign
  verify-attestation --type spdxjson` on 2026-10-18** (closes the post-release check of F18). The rollout
  (`apply.sh sui-indexer`) succeeded and `/healthz` reads `ok` on the same database generation `e32e37afbbe9`
  as before (the SQLite index on the Longhorn PVC was kept; re-read 2026-10-18 15:18Z: `ok: true`, both streams
  `ok`, last polls 3 s old). The live bundles of the consuming apps contain only the current ids (`0xd7ddaa94…`,
  `0x0c8f7349…`, gate `0x316f1bf9…`). The lines below describe the 0.0.9 deployment of 2026-10-09 and are kept as the
  baseline of that review.
- Tag `v0.0.9` = `a3369bb` (HEAD of `main`, 2026-10-09). Tags `v0.0.5`–`v0.0.9` were released 2026-10-08
  and 2026-10-09 (see the CHANGELOG).
- `quay.io/meddleware-org/sui-indexer:0.0.9` = `docker.io/meddleware/sui-indexer:0.0.9` = index
  `sha256:aea58153…1704` (checked with `skopeo` on both registries, created 2026-10-09 17:41Z). It is
  the digest in `config/images.yaml` and in the `post-bootstrap/sui-indexer` overlay, and all deployed
  images were cosign-verified on 2026-10-09 (`bootstrap/images/verify-digests.sh`, 16/16).
- Live at `https://sui-indexer.meddleware.co.uk`: `/healthz` returned `ok: true` for both streams with
  polls 5–6 s old (2026-10-09 22:49Z). It indexes the republished testnet packages (`access_gate`
  `0xd7ddaa94…88c9`, `seal_policies` `0x0c8f7349…773d`). `/v1/testnet/coverage` shows 3 `access-gate`
  events (from checkpoint 392921227), 0 `sealed-content` events, 0 undecodable, 0 gaps. A new-gate stats
  query shows the relay gate `0x316f1bf9…` with minted 1 / consumed 1, matching the paywall end-to-end
  run of 2026-10-09.
- The status page's "Chain Access → Chain index" check reads the in-cluster `/healthz`
  (`OPERATOR_TASKS.md`, done 2026-10-09).
- Manifests: `post-bootstrap/sui-indexer` in the infrastructure workspace (read for this pass, see Scope).
- Consumers: treasury-ui (`useTreasuryActivity`) and seal-ui (`discoverSealedContent`) through
  `VITE_INDEXER_URL` (`https://sui-indexer.meddleware.co.uk`). Decoding happens in access-gate-client
  (0.0.8) and seal-client (0.0.19).

**Review date:** 2026-10-03; re-verified 2026-10-09 (code at `a3369bb` = tag `v0.0.9`); code gates closed in 0.0.10 (`5251f38`), released and deployed 2026-10-18; re-verified 2026-10-19 against the Rust rewrite (0.0.12, local commits, not released)
**Reviewer:** Internal review
**Severity ceiling:** Medium.

- The service holds no secrets, signs nothing and authorises nothing (by invariant).
- Its worst plausible outcomes:
  - unavailability or staleness of display history;
  - permanent loss of history the full node has already pruned;
  - misleading display data from a compromised upstream.
- None of these can move funds or grant access.
- Realised ceiling at this pass: **Low**.

**Status:** re-verified 2026-10-09 against `v0.0.9`; code gates closed 2026-10-18 in `0.0.10` (commit `4aab344`; released and deployed 2026-10-18, signature and both SBOM attestations verified, rollout succeeded).
Of F1–F14, 5 findings were RESOLVED, 4 MITIGATED, 2 ADJUDICATED and the 3 Positives stand; F15–F21 were
added (F15–F17 RESOLVED, F18–F19 DEFERRED to the pre-mainnet gate, F20 ADJUDICATED, F21 ACCEPTED-RISK).
2026-10-18: F18, F20 and F21 are RESOLVED in 0.0.10, and F4 follows F18 to RESOLVED. Totals for F1–F21:
12 RESOLVED, 3 MITIGATED, 2 ADJUDICATED, 0 ACCEPTED-RISK, 1 DEFERRED (F19, a client change), 3 Positive.
Nothing is open without a named gate. First pass: 2026-10-03; there is no predecessor audit.
**2026-10-19 (Rust rewrite, 0.0.12):** F1–F21 re-verified against the new code (each finding carries a
"Re-verified 2026-10-19" note; no disposition regressed). F22–F26 added: F22 (read-only API process,
Positive), F23 (health and liveness across two processes, RESOLVED), F24 (stored columns from the node's
JSON rendering, ADJUDICATED with a parity gate), F25 (rare-kind filters walk the stream, pre-existing,
DEFERRED, OQ5), F26 (no connection cap, found by the RUST lens, RESOLVED). F11's last residual (the
infrastructure "rebuildable from the chain" wording) is fixed in the workspace manifests, so F11 is RESOLVED.
Totals for F1–F26: 15 RESOLVED, 2 MITIGATED, 3 ADJUDICATED, 0 ACCEPTED-RISK, 2 DEFERRED (F19, F25),
4 Positive.

**Front matter (RUST lens, from 0.0.12):**

**Toolchain:** `rust-version = "1.88"` (workspace; checked by the CI `msrv` job, and locally with 1.88 on
2026-10-19); builds on stable 1.98.1   **Edition:** 2024
**Crates published:** no (`publish = false`; shipped as an image)
**Runtime:** tokio — `ingest`: `current_thread` (`rt`, `time`, `signal`, `macros`); `api`: `multi_thread`
with `min(available_parallelism, 4)` workers (`net`, `time`, `signal`, `sync`)

| Crate | Binary | Direct dependencies (exact pins) |
| --- | --- | --- |
| `sui-indexer-store` | (library) | `rusqlite =0.40.2` (`bundled` SQLite), `serde`, `serde_json`, `getrandom =0.3.4` |
| `sui-indexer-ingest` | `/app/ingest` | `sui-rpc =0.4.0` (Mysten, Apache-2.0; tonic 0.14 + rustls `tls-ring` + webpki roots), `tonic =0.14.6`, `prost-types =0.14.4`, `tokio =1.53.3`, `base64 =0.22.1`, `bs58 =0.5.1`, `serde_json` |
| `sui-indexer-api` | `/app/api` | `hyper =1.12.0` (http1 server), `hyper-util =0.1.21`, `http-body-util`, `bytes`, `tokio`, `serde`, `serde_json`, `base64` |
| `sui-indexer-testkit` | `seed-fixture` (CI only) | fixtures; never in the image |

Up to 0.0.11 (TS lens): npm lockfile; esbuild bundle `dist/main.mjs` (603 kB); Node ≥ 24 on distroless
`nodejs26-debian13` (0.0.11); runtime dependencies `@meddleware/access-gate-client`, `@meddleware/seal-client`,
`@mysten/sui`.

**Front matter (SUI_CLIENT lens):**

| Field | Value |
| --- | --- |
| Transport | gRPC `LedgerService.ListEvents` (server streaming) and `GetServiceInfo` through `sui_rpc::Client` 0.4.0 (from 0.0.12; `SuiGrpcClient` before); `GRPC_URL` https-only except localhost |
| Package IDs | the client packages' recorded deployments, republished 2026-10-09 (testnet `access_gate` `0xd7ddaa94…88c9`, `seal_policies` `0x0c8f7349…773d`; original-id = published-at for both, a fresh publish). Overridable by env (validated and normalised). There is no mainnet deployment yet, so a mainnet start throws unless overridden. The superseded `0xa55789…6d41` / `0x61c4aa…7e42` are immutable and no longer indexed. |
| Network | one per instance (`NETWORK`, default `testnet`); also the API's first path segment. At startup the node's chain identifier must match `NETWORK` (0.0.5/0.0.6). |

**Front matter (IMG lens):**

| Field | Value |
| --- | --- |
| Images | `quay.io/meddleware-org/sui-indexer:0.0.10@sha256:b3484ce42a8484383efd598a3f21d3cb7077c7e3d4b73eb0df1c740b221d8a58` and `docker.io/meddleware/sui-indexer:0.0.10` (same index digest, amd64 + arm64; cosign signature and two SPDX attestations verified 2026-10-18; superseded 0.0.9 was `sha256:aea58153…1704`); self-hosted mirror best-effort. Deployed now: 0.0.11 `sha256:c2497d4c…5abe`. 0.0.12 (Rust) not yet published: a local podman build is 38.3 MB (about 15 MB compressed; 0.0.11's amd64 image is 60.4 MB compressed) |
| Base images | 0.0.12: builder `docker.io/library/rust:1-slim@sha256:4cd82946…1ab7` (cargo-chef 0.1.78, cargo-about 0.9.2); runtime `gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d…c1f2` (the gateway-rust pins). Up to 0.0.11: builder `node:2x-trixie-slim`, runtime distroless `nodejs2x-debian13:nonroot` (F17) |
| Runtime user | `65532:65532` (distroless nonroot; the published 0.0.9 image config says `65532:65532`) |
| Runtime FS | `/data` volume (SQLite, 1 Gi Longhorn RWO PVC); from 0.0.12 the `api` container mounts it read-only. The manifest sets `readOnlyRootFilesystem: true`, `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`, `runAsNonRoot` (65532), `seccompProfile: RuntimeDefault` and `automountServiceAccountToken: false`. |
| Deployed by | infrastructure workspace `post-bootstrap/sui-indexer` (read 2026-10-09); digest from `config/images.yaml` (`sha256:b3484ce4…8a58`, 0.0.10, since 2026-10-18; `sha256:aea58153…1704` at 0.0.9) stamped into the overlay |
| Build args | `VERSION`, `VENDOR`, `DESCRIPTION`, `SOURCE_URL`, `DOCUMENTATION_URL`, `IMAGE_URL` — all labels; no secrets |

**Location:** `sui-indexer/docs/audit/sui-indexer-audit.md`. This is the canonical audit (repo-local).

> **Access note:** `meddleware-org/sui-indexer` is not in this session's attached repository list.
> It is public, so it was cloned read-only. Nothing was pushed.

---

## Executive summary

**2026-10-19 — Rust rewrite (0.0.12, local commits).** The TypeScript service (below) is replaced by a Cargo
workspace; the HTTP API and the database are unchanged:

| Crate / module | Role |
| --- | --- |
| `store` (`lib.rs`, `config.rs`, `sui.rs`, `time.rs`) | the 0.0.11 schema verbatim; `Store` (the writer: atomic page + cursor ingest, counters and their one-time backfill, confirm-gated package binding, gap reset, heartbeats, the cutover backup) and `Reader` (read-only: `SQLITE_OPEN_READ_ONLY` + `query_only`); environment → config; address/type normalisation |
| `ingest` (`source.rs`, `decode.rs`, `ingestor.rs`, `chain.rs`, `lib.rs`, `main.rs`) | `sui-rpc` `ListEvents` with the page semantics of `@mysten/sui` `core.listEvents`; kind from the type, gate and commission from the node's JSON (no BCS decoding); poll loop, gaps, heartbeats; startup order; `--healthcheck` |
| `api` (`handler.rs`, `query.rs`, `ratelimit.rs`, `server.rs`, `lib.rs`, `main.rs`) | routing, validation, canonical URLs, cache headers and ETags, rate limit; WHATWG-exact URL/query handling; hyper server with timeouts, a connection cap and graceful shutdown |
| `testkit` | fixtures and `seed-fixture` (CI only) |

- **Tests:** 107 Rust tests (store 32, ingest 35, api 40; 7 of them property tests), 5 client-contract tests
  (the published client packages against the built binary), 10 SBOM-gate script cases, the two-container
  smoke test. Line coverage 90.7% (`cargo llvm-cov`; the two `main.rs` are thin and run by the smoke test).
- **Equivalence evidence:** every API case of 0.0.11 ported; the local `api` over a database the Rust
  ingester built from testnet answered byte-identically to the live 0.0.11 instance for 10 routes and 6
  error/redirect cases (modulo generation and poll time); JSON-derived vs BCS-derived columns 19/19 on every
  testnet event of the current and two earlier `access_gate` packages; the stored cursor equals the TS
  client's `endCursor` for the same page.
- **Measured (4-CPU laptop):** `api` 5 MB RSS idle, 8 MB at ≈ 18k req/s on the live-sized index, 26 MB peak
  and ≈ 14k req/s (head page) on a 300k-row index; `ingest` 8 MB; binaries 2.9 MB / 5.7 MB.

The TypeScript service (0.0.4–0.0.11) was 1,136 lines in seven modules (816 lines in six at the first pass):

| Module | Role |
| --- | --- |
| `config` | environment → config (adds `REINDEX_CONFIRM`) |
| `store` | SQLite schema, atomic page + cursor ingest, counters, pages, confirm-gated package binding |
| `ingest` | ascending poller per stream, independent streams, pruned-cursor recovery |
| `api` | routing, validation, canonical-URL redirects, cache headers, rate limit |
| `ratelimit` | per-key token buckets (bounded map) |
| `chain` | maps the node's chain identifier to the short id |
| `main` | wiring, startup refusals |

There were 52 tests in five files at 0.0.10 (30 at the first pass; 47 at 0.0.9).

**Re-verification 2026-10-09 (code `a3369bb` = `v0.0.9`).** The first-pass findings were fixed in 0.0.5
(2026-10-08) and the follow-up releases 0.0.6–0.0.9; the CHANGELOG lists them and each finding below
cites the commit and the test that pins it.

**What holds (verified):**

- **Ingest correctness.**
  - Each stream is read ascending from a stored cursor.
  - A page and its cursor commit in one transaction; re-delivery is a no-op
    (`UNIQUE (tx_digest, event_index)`).
  - Row id = ledger order, so pages bounded by `before` really are immutable.
  - A null end-cursor keeps the position.
  - A 20-page budget per stream per poll.
  - Streams are polled independently; an undecodable event is counted and skipped; a cursor below the
    node's retention restarts from the node's earliest data (a counted gap).
- **Full-node timeout works.** The 30-second `listEvents` timeout is honoured: `@mysten/sui` passes
  `signal` as `{ abort }` to the call (verified in the SDK source at 2.33.2; the type is now checked by
  tsc, F9).
- **One parser.** Events are decoded with the published client parsers against the configured
  original IDs. Rows are served verbatim (BCS base64 + digest + checkpoint) for the clients to decode
  again.
- **Package binding and generations.** A changed package no longer clears history on start: startup
  fails with `PackageChangedError` until `REINDEX_CONFIRM` names the stream; a `VACUUM INTO` backup is
  taken, the stream cleared and the generation rotated (old cursors return `410`).
- **API hardening.**
  - GET, HEAD and OPTIONS only.
  - Unknown parameters, bad IDs, limits and cursors get `400`; other spellings of a query get a cacheable
    `301` to the canonical form.
  - SQL is parameterised.
  - `nosniff`; CORS `*` without credentials.
  - `no-store` on errors and `429`.
  - Request and header timeouts.
  - Commission summed as `bigint`.
- **Image and release.**
  - Both bases pinned by digest; distroless nonroot runtime with no shell; multi-arch.
  - The tag workflow reuses the full CI (type-check, lint, tests, bundle, licence check, Trivy config
    scan, Trivy image scan, container smoke test), scans the published image before signing, and the
    printed `cosign verify` command pins the workflow identity.
  - The published 0.0.9 image carries the lockfile, `package.json` and `THIRD_PARTY_LICENSES`
    (checked by unpacking the amd64 image from quay.io).
  - Keyless cosign signature, GitHub provenance attestation and a signed SPDX SBOM attestation are
    present on the 0.0.9 index digest (OCI referrers on Docker Hub). That SBOM lacks the bundled npm
    packages (F18); from 0.0.10 the release also attests an SBOM of them.
- **Measured (2026-10-09).**
  - 47/47 tests; tsc, eslint and `npm audit --audit-level=high` (0) clean;
  - `npm run build` succeeds (603 kB); `npm run check:licenses` passes (24 production packages).
  - Coverage was not re-measured (the coverage plugin is not installed); the first-pass figures were
    82.0% statements / 85.6% branches with `main.ts` untested, and the counters, resilience and API
    suites added since raise coverage of `store.ts`, `ingest.ts` and `api.ts`.

**Findings at this pass (none above Low):**

| Disposition | Findings |
| --- | --- |
| RESOLVED | F1 (full-stream scans), F4 and F18 (SBOM of the bundled code: npm in 0.0.10, Cargo per binary in 0.0.12), F5 (licence notices), F6 (cache key and limiter), F9 (casts, ETags), F10 (CI and image pipeline), F11 (documentation drift; last residual fixed 2026-10-19), F15 (pruned-cursor wedge), F16 (chain-identity startup bug; the startup path is now tested), F17 (libssl advisories in the base image), F20 (operator-keyed lookups), F21 (`noUncheckedIndexedAccess`; TS-only, superseded), F23 (health across two processes, 0.0.12), F26 (connection cap, 0.0.12) |
| MITIGATED | F2 (stuck ingest; clients ignore `polledAt`, see F19), F3 (history loss on config change fixed; volume loss remains) |
| ADJUDICATED | F7 (new event types need a fresh publish), F8 (display-only trust in the node; chain identity now checked), F24 (stored columns from the node's JSON rendering; parity gate) |
| ACCEPTED-RISK | none |
| DEFERRED | F19 (clients treat a stale head page as unavailable): pre-mainnet gate row in Section D; the change lives in access-gate-client and seal-client. F25 (rare-kind filters walk the stream; an additive index after the cutover, OQ5) |
| Positive | F12, F13, F14, F22 (read-only API process) |

**Posture:**

- Small, careful code. The correctness invariants (ordering, atomicity, generations) are sound and
  tested.
- The operating-at-scale items (query cost, stuck ingest, destructive rebinding) are fixed in code and
  pinned by tests. What remains is the **platform side** (volume backup, F3) and the **client side of
  freshness** (F19). SBOM completeness (F18) is fixed in the 0.0.10 release workflow.
- The first pass recorded findings only; this pass records the dispositions of the fixes released
  2026-10-08/09 (see the re-verification log).
- 2026-10-19: the Rust rewrite keeps every behaviour above and adds a process boundary — the API can no
  longer write the index (F22). Its new trust point is the node's JSON rendering for three stored columns
  (F24), checked against the client parsers on all live testnet events.

---

## Threat model / trust boundaries

| Actor / source | Controls | Can do | Bounded by |
| --- | --- | --- | --- |
| Anonymous client (through Cloudflare) | URLs, headers, request rate | Read pages; force origin work with cache-miss URLs | Edge cache; per-IP bucket on `CF-Connecting-IP` (IPv6 by /64, bounded map); ingress limit 20 r/s; strict params; cacheable `301` to the canonical URL (F6); O(1) counters instead of scans (F1). The Cloudflare rate-limit rule is **not** set (free plan, maintainer item in the infrastructure README) |
| Client reaching the origin off-tunnel (in-cluster pod, mis-exposed Service) | `CF-Connecting-IP` | Forge the header to dodge the per-IP limit | SECURITY.md: only via the tunnel. The NetworkPolicy (default-deny ingress) admits only the `nginx-ingress` namespace and `platform-probe` on 8080 |
| Full node at `GRPC_URL` | the event stream (BCS and its JSON rendering) | Serve fabricated or omitted events; a JSON rendering that disagrees with the BCS; hang | https (rustls, webpki roots); 30-second timeout per request plus the SDK's 30-second body-idle watchdog; exact type match at the original ID; strict JSON field checks, else counted as undecodable (F24); the clients decode the BCS again; chain identifier must match `NETWORK` (F8, F16). **No authenticity of rows (F8, display-only)** |
| A compromised `api` process (from 0.0.12) | its own container | Serve wrong pages | cannot write the index: read-only mount, `SQLITE_OPEN_READ_ONLY`, `query_only` (F22); no chain access, no credentials |
| Operator / config (`NETWORK`, `GRPC_URL`, `*_ORIGINAL_ID`, `DB_PATH`) | what is indexed | A package-ID change clears a stream | Validated and normalised IDs; startup refuses until `REINDEX_CONFIRM` names the stream; `VACUUM INTO` backup first; generation rotation (F3). The backup sits on the same volume |
| `access_gate` / `seal_policies` publisher | event types and layouts | Add event types in an upgrade | Exact-type parsers. Upgrade-defined types are not indexed: new event types need a fresh publish (F7, decision) |
| Consumer apps | interpretation | Treat display data as truth | Display-only invariant; the clients decode again and fall back on errors. Head pages carry `polledAt`; the clients do not use it yet (F19) |
| Dependency and base-image authors | crates (0.0.12; bundled JS before); base layers | Ship vulnerable code | `Cargo.lock`; `cargo deny` (RustSec advisories, licences, bans, sources: crates.io only); digest pins; Trivy image scan before sign; `Cargo.lock` and licence notices in the image; a checked CycloneDX SBOM per binary (F18) |

### On-chain dependency matrix (SUI_CLIENT lens)

| Object / package | ID | Sourced from | Used as | If stale / wrong | Fails |
| --- | --- | --- | --- | --- | --- |
| `access_gate` package | original ID = published-at `0xd7ddaa94…88c9` (testnet, 2026-10-09; previously `0xa55789…6d41`) | `@meddleware/access-gate-client/deployments`, or `ACCESS_GATE_ORIGINAL_ID` | event filter `<id>::access_gate` (`eventTypeModule`); parser's expected type prefix | a wrong ID indexes nothing and, on the next start, is refused until `REINDEX_CONFIRM` (F3); upgrade-defined types missed (F7) | closed (refuses to start) |
| `seal_policies` package | original ID = published-at `0x0c8f7349…773d` (testnet, 2026-10-09; previously `0x61c4aa…7e42`) | `@meddleware/seal-client/deployments`, or `SEAL_ORIGINAL_ID` | filter `<id>::sealed_content::SealedContentPublished` | same | same |
| Full node | `GRPC_URL` | env / Mysten default | event source | wrong network ⇒ refused at startup (chain identifier check, F8); a compromised node ⇒ fabricated rows (display-only) | closed at startup |

### Threat model — service actor matrix (RUST lens, from 0.0.12)

| Actor | Controls | Bounded by |
| --- | --- | --- |
| Network client | request line, headers, timing (bodies are never read) | strict query/path parsing (`api/src/query.rs`, proptests), 5 s header and 10 s request timeouts, 32 KiB request head, 1024 connections (F26), per-IP limiter (A-rate limiting) |
| Upstream service | — (the API forwards nothing) | — |
| Sui full node (gRPC, tonic/prost through `sui-rpc`) | stream frames and payloads | generated protobuf decoding (prost; not hand-rolled), tonic's 4 MiB default message cap, one page of 51 items per request, 30 s timeout, JSON fields checked strictly (F24) |
| Shared state store | the SQLite file on the PVC | one writer (`ingest`); the `api` is read-only (F22); WAL; `busy_timeout` 5 s |
| Ingress / proxy in front | `CF-Connecting-IP` | trusted only because the NetworkPolicy admits nginx-ingress and platform-probe only (F6) |
| Crate registry and build pipeline | the compiled code | `Cargo.lock`, `--locked` everywhere, `cargo deny` (sources: crates.io only), digest-pinned builder, SBOM gate (B.1, B.RS-1) |

Up to 0.0.11 (TS lens): dependency authors were the two clients and `@mysten/sui` (+ `@mysten/bcs`, `@noble/*`,
`@protobuf-ts/*`, `@mysten/seal`), bounded by the lockfile, `npm ci` and `npm audit` (F18).

### Image supply chain (IMG lens)

| Actor / asset | Power | Bounded by |
| --- | --- | --- |
| Base-image publishers (Docker Library, Google distroless) | the base layers | digest pins (IMG-M1) |
| Build context | files sent to BuildKit | from 0.0.12 `COPY . .` into the build stages (only the binaries, `Cargo.lock`, `LICENSE` and the notices reach the runtime stage); `.dockerignore` excludes `target`, `**/node_modules`, `.git`, `.github`, `*.md`, `.env*`, `*.db`, `*.db-*`, `*.bak-*` (F10) |
| Build args | labels only | no secrets (IMG-M4) |
| CI publish job | push, sign, attest | SHA-pinned actions; OIDC; full CI reused on the tagged commit; Trivy scan before sign (F10) |
| Cluster manifests | runtime security context | `post-bootstrap/sui-indexer` read 2026-10-09: full restricted context, probes, limits, NetworkPolicy (Section B) |

---

## Severity scale

Critical / High / Medium / Low / Info / Positive.

## Scope

**In scope 2026-10-19 (0.0.12, Rust; local commits `1fcbbe9`…`ddfc77d` on `fb79cd7`):**

- `Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, `deny.toml`, `about.toml`, `about.hbs`
- `crates/store/src/{lib,config,sui,time,tests}.rs`, `crates/ingest/src/{lib,main,source,decode,ingestor,chain,tests}.rs`,
  `crates/api/src/{lib,main,handler,query,ratelimit,server,tests}.rs`, `crates/testkit/src/{lib,main}.rs`
- `tests/contract/` (CI-only client contract), `scripts/{smoke.sh,check-sbom.sh,tests/check-sbom-test.sh}`
- `Dockerfile`, `.dockerignore`, `.gitignore`, `.github/workflows/{rust-ci,docker-publish}.yml`, `.github/dependabot.yml`
- `README.md`, `CLAUDE.md`, `AGENTS.md`, `SECURITY.md`, `CHANGELOG.md`
- workspace: `post-bootstrap/sui-indexer/{base/deployment.yaml,base/pvc.yaml,README.md}` (two containers), decision D58

**In scope up to 0.0.11 (HEAD `a3369bb` = tag `v0.0.9`; first pass `9aa3370` = `v0.0.4`):**

- `src/{config,store,ingest,api,ratelimit,chain,main}.ts`
- `tests/{api,chain,config,ingest,sbom}.test.ts`, `tests/fixtures.ts`
- `package.json`, `package-lock.json`, `tsconfig.json`, `eslint.config.ts`, `vitest.config.ts`
- `Dockerfile`, `.dockerignore`, `.gitignore`, `scripts/{smoke.sh,third-party-licenses.mjs}`
- `README.md`, `CLAUDE.md`, `AGENTS.md`, `SECURITY.md`, `CHANGELOG.md`, `LICENSE`
- `.github/workflows/{node-ci,docker-publish}.yml`, `.github/dependabot.yml`

**Cross-repo evidence (read-only):**

- `@mysten/sui` gRPC core (`listEvents`, cursor and filter handling) in `node_modules`;
- the client deployments as installed (`access-gate-client` 0.0.8, `seal-client` 0.0.19) and their
  CHANGELOGs;
- treasury-ui and seal-ui indexer wiring;
- the infrastructure workspace: `post-bootstrap/sui-indexer` (Deployment, Ingress, NetworkPolicy, PVC,
  README), `config/images.yaml`, `OPERATOR_TASKS.md`, `STORAGE.md`;
- quay.io and Docker Hub (`skopeo inspect` of the 0.0.9 index; the OCI referrers and SBOM attestation
  on Docker Hub; the amd64 image unpacked to check `/app`);
- the live service (`/healthz`, `/v1/testnet/coverage`, a canonical-URL redirect, a gate stats query).

**Not available / not run:**

- the Actions run history (the repo's workflow runs);
- `cosign verify` of the signer identity (cosign is not installed in this environment; the
  infrastructure's `verify-digests.sh` verified 16/16 images on 2026-10-09, and the Fulcio identity
  check is therefore recorded from that run, not re-run);
- Trivy or Syft on the image (the CI Trivy gates are recorded from the workflow files);
- the running pod (probes and security context are read from the manifest, not from `kubectl`).

**Out of scope:** the client packages' parsers and indexer readers (their own audits); Cloudflare
configuration and the cluster platform (platform lens).

**Environment / commands (2026-10-09, Node 24.13.0 — `node:sqlite` available):**

| Command | Result |
| --- | --- |
| `npm ci` | clean |
| `npm run type-check` / `npm run lint` | clean / clean |
| `npx vitest run` | **47 passed** (4 files) at 0.0.9; 52 passed (5 files) at 0.0.10, 2026-10-18 |
| `npm audit --audit-level=high` | 0 vulnerabilities |
| `npm run build` | `dist/main.mjs` 603.1 kB |
| `npm run check:licenses` | 24 production packages; 4 without a licence file in the package (`@mysten/bcs`, `@mysten/seal`, `@mysten/utils`, `poseidon-lite`: the declared licence is recorded instead); `dist/THIRD_PARTY_LICENSES` contains the Apache-2.0 text |
| `npm ls` | one `@mysten/sui` (2.35.0) across both clients; `access-gate-client` 0.0.8 deduped under `seal-client` |
| Lockfile install scripts | `esbuild` (dev), `fsevents` (dev, optional) |
| `skopeo inspect` | `quay.io/meddleware-org/sui-indexer:0.0.9` and `docker.io/meddleware/sui-indexer:0.0.9` both `sha256:aea58153…1704`, equal to `config/images.yaml` |
| Published image config (amd64) | `User 65532:65532`; `PORT=8080 DB_PATH=/data/indexer.db`; label version `0.0.9` (no `v`); `/app` holds `main.mjs`, `package.json`, `package-lock.json`, `THIRD_PARTY_LICENSES` (91.9 kB) |
| Docker Hub referrers of the 0.0.9 index | GitHub provenance (`slsa.dev/provenance/v1`), cosign signature, and the SPDX SBOM attestation (annotation still reads `cosign/sign/v1`). The SBOM has 17 packages: the 15 Debian/Node packages, the root `@meddleware/sui-indexer` and the image; **none** of the bundled npm packages (F18) |
| Live service | `/healthz` 200 with both streams ok; coverage and gate stats as in Deployment status; `limit=05` answered `301` to `limit=5` with `max-age=86400`; head page `Cache-Control: public, max-age=10, s-maxage=30` with an ETag |
| Coverage run | **not run** (plugin not installed); first-pass figures: 82.01% statements / 85.64% branches / 75% functions / 83.21% lines at 0.0.4 |

**Environment / commands (2026-10-19, Rust rewrite; `cargo +stable` 1.98.1, Linux x86_64, 4 CPUs):**

| Command | Result |
| --- | --- |
| `cargo fmt --check` / `cargo clippy --all-targets --locked -- -D warnings` | clean / clean |
| `cargo test --locked` | **107 passed** (store 32, ingest 35, api 40) |
| `cargo llvm-cov --summary-only` | lines 90.7%, functions 87.6%, regions 89.4% (`main.rs` of both binaries 0%: thin, run by the smoke test) |
| `cargo +1.88 check --locked --workspace --all-targets` | passes (MSRV 1.88; let-chains need it) |
| `cargo deny check` (0.20.2) | advisories ok, bans ok (duplicates warned: `getrandom`, `syn`), licenses ok, sources ok |
| `cargo about generate --fail` (0.9.2), per binary | passes; `api` 44 crates, `ingest` 137 crates |
| `cargo cyclonedx` (0.5.9) + `scripts/check-sbom.sh` | `api` SBOM lists all 63 crates of its graph, `ingest` all 139; a removed crate fails (`scripts/tests/check-sbom-test.sh`, 10 cases) |
| `tests/contract` (vitest, `@meddleware/access-gate-client` 0.0.9, `@meddleware/seal-client` 0.0.20) | 5 passed against `target/release/api` |
| `podman build` | image 38.3 MB; `DOCKER=podman bash scripts/smoke.sh` passes (two containers, API mount `:ro`) |
| Live (read-only) | `ingest` against `fullnode.testnet.sui.io` into a scratch DB (11 events), `api` on it: 10 routes and 6 error/redirect cases identical to `https://sui-indexer.meddleware.co.uk` (generation and poll time aside); parity 19/19 (F24) |
| Load (dev-only closed-loop client, 64 keep-alive connections, 10 s, limiter raised) | head page ≈ 18k req/s (11 rows), ≈ 14k req/s (300k rows); `gate=` filter ≈ 5.2k req/s; `kinds=AccessConsumed` on 300k rows **10 req/s** (F25) |
| Not run | the publish workflow, cosign, Trivy, actionlint (GitHub Actions runs them); the cluster rollout |

First-pass commands (2026-10-03, Node 22.22.2) are recorded in the re-verification log.

---

## Findings

### F1 — Every events page computes stream coverage with a full scan, on the event loop

**Severity:** Low (availability of display data; the ceiling for this class is Medium)
**Disposition:** RESOLVED (0.0.5, `1973fca`)
**Where:**

- `src/api.ts:116`: `eventsPage` calls `store.coverage(stream)` for every head *and* older page;
- `:204`: the stats route calls it too;
- `src/store.ts:225-233`: `SELECT MIN(checkpoint), MAX(checkpoint), COUNT(*) … WHERE stream = ?`.
  The indexes are `(stream, id)` and `(stream, gate_id, id)` only;
- `src/store.ts:237-251`: `gateStats` iterates every mint row of the gate.

**Issue:**

- `MIN` and `MAX` over the unindexed `checkpoint` column, plus `COUNT(*)`, scan the whole stream.
- `node:sqlite` is synchronous, so each scan blocks the one thread that also serves every other
  request, runs the poll loop and answers `/healthz` and `/readyz`.
- Benchmark (300k rows, in memory, so a disk-backed database is no faster):

| Query | Time |
| --- | --- |
| `page()` | 0.18 ms |
| `headId()` | 0.02 ms |
| **`coverage()`** | **148 ms** |
| `gateStats()` for a gate with 6k mints | 16 ms |

- A cache-miss events request therefore costs about 150 ms of exclusive CPU at that size, which is
  about 7 requests a second for the whole process.
- The per-IP bucket allows 10 requests a second (burst 40) per address, and F6 lets any client make
  every request a cache miss.
- README's "about 3.5k req/s on a laptop" was presumably measured on a small database.

**Impact:**

- As history grows (every purchase, consume and burn is a row), a few clients can saturate the
  origin.
- Ingest falls behind and requests time out.
- `/readyz` may miss its probe window (a 10-second request timeout and synchronous queries), which
  makes the liveness probe restart the pod and replay the cost.
- The apps fall back to the full node only on errors, not on slowness.

**Remediation / evidence:**

- Maintain `fromCheckpoint`, `latestCheckpoint` and `events` per stream in `meta`, updated inside the
  `ingest()` transaction. They are monotone, except after `bindPackages`, which resets them.
- Or add `CREATE INDEX events_ckpt ON events (stream, checkpoint)` and drop `COUNT(*)` from the page
  path.
- Include `indexedFromCheckpoint` only on head pages, or read it from `meta`.
- Maintain per-gate counters and the commission sum at ingest (`gate_stats` table).
- Add a benchmark test with a row-count budget.

**Resolution (0.0.5, `1973fca`; re-verified at `a3369bb`):**

- The `Where` lines above describe the first-pass code; the scans are gone. `store.ts` now keeps
  `stream_stats` (from/latest checkpoint, event count, undecodable, gaps), `gate_stats` (count per gate
  and kind) and `gate_commission` (running `bigint` total as text), all updated **inside the ingest
  transaction**. Databases that predate them are rebuilt once at startup (`meta` key `stats:v2`).
- `coverage()` and `gateStats()` are now single-row or primary-key reads; `headId()` is
  `MAX(id)` on the `(stream, id)` index. The first-pass option "an index on `checkpoint`" was not needed.
- Pinned by `tests/ingest.test.ts` → *Store counters*: counters stay exact across duplicates and agree
  with a recount; they are rebuilt for a database that predates them; coverage over 50k rows is a
  constant-time read.
- Not done, by choice: `indexedFromCheckpoint` is still returned on every page (an O(1) read now), and
  there is no separate benchmark test beyond the 50k-row case.
- Live: `/v1/testnet/coverage` answers from the counters (2026-10-09).

**Re-verified 2026-10-19 (0.0.12, Rust):** The counters are kept verbatim: `Store::ingest` (`crates/store/src/lib.rs`) bumps `stream_stats`, `gate_stats` and `gate_commission` in the page transaction (commission summed as `u128`, stored as decimal text), and `backfill_stats` rebuilds them once (`stats:v2`). `coverage`, `health`, `gate_stats` and `head_id` are single-row or index reads. Tests: *counters_stay_exact_across_duplicates_and_agree_with_a_recount*, *counters_are_rebuilt_…*, *coverage_over_50k_rows_is_a_constant_time_read*, *commission_totals_are_exact_beyond_64_bits*. Disposition unchanged (RESOLVED). One request shape still walks rows: a `kinds` filter for a rare kind (F25, pre-existing).

### F2 — One undecodable event halts both streams forever; stale pages give no signal

**Severity:** Low   **Disposition:** MITIGATED (indexer side RESOLVED in 0.0.5, `1973fca`; the clients' use of `polledAt` is F19; monitoring done 2026-10-09)
**Where:**

- `src/ingest.ts:98-105`: `toIndexed` throws for a recognised event whose BCS does not decode, which
  aborts the page;
- `:113-118`: `poll()` runs `access-gate` then `sealed-content` sequentially, so a throw in the first
  skips the second;
- `:122-131`: the loop logs and retries the same cursor every interval.

**Issue (probe-verified):**

- A fake node served page 0 = [a valid `AccessMintedEvent`, an `AccessMintedEvent` with 3-byte BCS].
- Every `poll()` threw `Invalid typed array length: 32`, and the state stayed:
  - `access-gate` cursor `null`, 0 rows (the valid event on the same page was **not** stored);
  - `sealed-content` cursor `null`, 0 rows (**never polled**);
  - `lastSuccessAt` 0.
- A restart replays the same failure.
- How such an event could arise:
  - a client-parser bug or stricter "fail-closed" check (0.0.3 tightened the parsers);
  - version skew between the client and an upgraded package;
  - a full node returning truncated `contents`.
- A third party cannot forge one: Sui restricts `event::emit` to the defining module.
- `/healthz` goes 503, which is good. But every page, including the head page (cached 30 seconds),
  keeps answering `200` with the frozen history, and the response carries no "indexed up to" or
  "last poll" field.

**Impact:**

- Ingest for **both** products stops until a code or configuration change.
- treasury-ui and seal-ui keep showing stale history with no fallback, because they fall back only on
  errors. New sealed content is invisible in seal-ui discovery.
- The only signal is an external monitor on `/healthz`, whose wiring is not in this repo.

**Remediation / evidence:**

1. Poll the streams independently: catch per stream and record per-stream `lastSuccess`.
2. On a decode error, either:
   - store the raw row with `kind = 'Undecodable'` and advance (the clients decode again anyway); or
   - quarantine it in a side table and advance.

   Log loudly and expose a count in `/coverage`.
3. Add `latestCheckpoint` and `lastPollAt` (per stream) to head pages. Have the clients treat a head
   page older than a threshold as unavailable and fall back. That is a client change, coordinated
   with access-gate-client and seal-client.
4. Add a test for each.

**Resolution (0.0.5, `1973fca`; re-verified at `a3369bb`):**

- **Independent streams.** `Ingestor.poll()` calls `pollOne(stream)` per stream; a failure is caught and
  recorded per stream (`lastSuccessAt`, `lastError`), so a throw in `access-gate` no longer skips
  `sealed-content` (`ingest.ts` `pollOne`). Test: *polls the streams independently: one failing does not
  stop the other, and health names it*.
- **Undecodable events** are counted (`stream_stats.undecodable`), logged loudly (`UNDECODABLE …`) and
  skipped; the valid events of the same page are stored and the cursor advances. Decision for OQ2:
  **count and skip** (not store, not quarantine). Test: *skips and counts an undecodable event, keeps
  its page-mates, and advances*.
- **Freshness signal.** `/healthz` reports `{ ok, lastPollAgeMs, lastError }` per stream (503 when either
  stream has not succeeded within 5 poll intervals); head pages carry `latestCheckpoint` and `polledAt`
  (epoch ms of the stream's last successful poll); `/coverage` adds `undecodable` and `gaps`. Test:
  *head pages say how fresh the index is; older pages stay immutable and free of it*.
- **Monitoring.** The status page's "Chain Access → Chain index" check polls the in-cluster `/healthz`
  and expects `"ok":true` (done 2026-10-09, `OPERATOR_TASKS.md`; the check was failure-tested). The
  external monitor that the first pass found missing exists.
- **Remaining.** The access-gate-client and seal-client do not read `polledAt` or `latestCheckpoint` (grep
  of `src/` at 0.0.8 / 0.0.19), so a stale but healthy-looking head page is not treated as
  unavailable by the apps. Tracked as F19. A stuck ingest is now visible on the status page and in
  `/healthz`, but the apps still show the stale history until the indexer recovers.

**Re-verified 2026-10-19 (0.0.12, Rust):** Independent streams (`Ingestor::poll_one` per stream), count-and-skip (`stream_stats.undecodable`, `UNDECODABLE` log line) and the freshness fields are ported. What counts as undecodable changed with the decoding (F24): a recognised event whose JSON rendering lacks a valid `gate_id`/`commission_mist`, or that has no BCS, digest or index. Tests: *skips_and_counts_an_undecodable_event_…*, *a_recognised_event_without_what_the_row_needs_is_undecodable*, *polls_the_streams_independently_…*. `/healthz` now reads the per-stream heartbeat rows the ingester writes (F23). Disposition unchanged (MITIGATED; F19 open).

### F3 — Clearing a stream permanently loses history the full node has pruned

**Severity:** Low   **Disposition:** MITIGATED (the destructive-clear path RESOLVED in 0.0.5, `1973fca`, OQ1 decided; volume loss remains a maintainer/platform item)
**Where:** `src/store.ts:97-126` (`bindPackages`: `DELETE FROM events WHERE stream = ?`); README
"First start … History before that is not listed".

**Issue:**

- `bindPackages` deletes a stream's rows whenever the configured package ID differs from the recorded
  one. That happens on:
  - a client bump that changes the default deployment (by design, for a fresh publish);
  - an `ACCESS_GATE_ORIGINAL_ID` or `SEAL_ORIGINAL_ID` override set, changed, mistyped or **removed**
    (removing it reverts to the default);
  - rows predating binding.
- Re-indexing reads only what the full node still serves. Testnet prunes events after about
  3 months (dao-ui CLAUDE.md), so everything older is gone for good.
- That pruned history is exactly what the indexer uniquely holds:
  - gate stats and commission totals;
  - treasury activity;
  - sealed-content pointers older than the node's window.
- Reverting a mistaken override clears the stream **again**: the old package's rows were deleted on
  the first change.
- The same loss follows from losing the RWO volume. Backups are an infrastructure matter and were not
  seen.

**Impact:** an irreversible loss of display history, including seal-ui discovery pointers (content
then becomes hard to find, although the on-chain events remain where the node keeps them).

**Remediation / evidence:**

- Never delete. Key rows by `(stream, package)`, serve only the bound package's rows, and keep the
  others (or move them to an archive table).
- Or require an explicit `REINDEX_CONFIRM=<stream>` to clear, and refuse to start otherwise.
- Snapshot the database file before any clear (`VACUUM INTO`).
- Document volume-snapshot backups in the deployment.

**Resolution (0.0.5, `1973fca`; re-verified at `a3369bb`):**

- **No silent clear.** `bindPackages` now returns the streams to clear only when they are listed in
  `REINDEX_CONFIRM`; otherwise it throws `PackageChangedError` and `main.ts` logs
  "refusing to start" and exits 1, changing nothing. Rows predating package binding are treated the same
  way. Confirming first copies the database to `<DB_PATH>.bak-<epoch ms>` with `VACUUM INTO`, then
  clears the stream's rows, cursor and counters (and the gate counters for `access-gate`) in one
  transaction and rotates the generation. The README documents the procedure. Tests (*Store.bindPackages*):
  refuses to clear history without confirmation and changes nothing; clears only the confirmed stream,
  backs the database up first, and rotates the generation; treats rows indexed before packages were
  recorded like a package change; writes a restorable backup before clearing.
- **Decisions (OQ1).** A fresh publish clears the old package's rows after a backup; history is not kept
  queryable per package. A reverted override is therefore also a confirmed clear.
- **Used in anger.** The 2026-10-09 republication of `access_gate` and `seal_policies` was indexed through
  this path (0.0.7 requires `REINDEX_CONFIRM` to rebind): the live index now holds only the new package's
  events (`0xd7ddaa94…` event types, coverage from checkpoint 392921227). The superseded packages are
  immutable and no longer indexed. The deployment manifest carries no `REINDEX_CONFIRM` now, so a later
  accidental change refuses to start again.
- **Correction to the first-pass text.** Testnet full nodes keep about **5½ days** of events (measured
  2026-10-08), not "about 3 months" (the dao-ui note was corrected). The window is far shorter, so the
  indexer holds almost all of its history uniquely: backups matter more than the first pass assumed.
- **Remaining (volume loss).** The `.bak-*` copy sits on the same 1 Gi Longhorn RWO volume, so it does not
  protect against losing the volume. The manifests set no snapshot or backup policy, and the
  infrastructure README and PVC comment still say the index is "rebuildable from the chain", which is
  true only for the node's retention window. PVC-level restore is "a separate backup/restore concern"
  (`STORAGE.md`). This is a platform item (PLATFORM lens, backups) and maintainer-owned; there is no
  `OPERATOR_TASKS.md` row for it yet (OQ4). Accepted for testnet, because the index is display-only and
  the apps fall back to the node for the recent window.

**Re-verified 2026-10-19 (0.0.12, Rust):** `Store::bind_packages` keeps the refusal (`PackageChangedError` until `REINDEX_CONFIRM`), the `VACUUM INTO <DB_PATH>.bak-<ms>` copy, the clear and the generation rotation; `start()` exits non-zero with "refusing to start". Tests: the six *Store.bindPackages* cases ported, plus *start_refuses_a_package_change_until_confirmed_then_backs_up_and_reindexes* (process wiring). New: the first start of 0.0.12 on a 0.0.11 database writes `<DB_PATH>.bak-pre-rust-<ms>` before writing anything (*backs_up_an_earlier_writers_database_once_before_marking_it*, *start_backs_up_a_database_of_the_previous_indexer_once*). That copy also sits on the 1 Gi volume and doubles the database for as long as it is kept (the live index is tens of kB, so this is not a space risk now). Volume loss remains the platform item; disposition unchanged (MITIGATED).

### F4 — The image SBOM omits every bundled npm dependency

**Severity:** Low   **Disposition:** RESOLVED (lockfile and licences shipped in 0.0.5, `1973fca`; the SBOM of the bundle is F18, 0.0.10, commit `4aab344`)
**Where:** `Dockerfile:12, 36` (esbuild bundle; only `main.mjs` reaches the runtime stage);
`docker-publish.yml:229-244` (`anchore/sbom-action` scans the **image**).

**Issue (verified on the published 0.0.4 SBOM attestation):** the SPDX document lists exactly
16 packages:

- `base-files`, `ca-certificates`, `gcc-14-base`, `libc6`, `libgcc-s1`, `libgomp1`, `libssl3t64`,
  `libstdc++6`, `libzstd1`, `media-types`, `netbase`, `node 24.21.0`, `tzdata`, `tzdata-legacy`,
  `zlib1g`;
- the image itself.

It has **none** of the bundled JavaScript: `@mysten/sui`, `@mysten/bcs`, `@mysten/seal`,
`@noble/curves`, `@noble/hashes`, `@scure/base`, `@protobuf-ts/*`, `@meddleware/*` and so on. Syft
finds JS packages through `package.json` and lockfiles, and a bundle has neither.

**Impact:**

- Registry scanners and SBOM-driven vulnerability management (Trivy or Grype on the image, Docker
  Scout, quay's scanner) cannot see a CVE in the code that parses network input and does cryptography.
- `npm audit` in CI covers the lockfile only at build time, not retrospectively for deployed images.

**Remediation / evidence:**

- Generate the SBOM from the source tree, which includes the lockfile, and merge it with the image
  SBOM before `cosign attest`. For example: `syft dir:. -o spdx-json`, or `npm sbom --sbom-format spdx`
  restricted to production dependencies.
- Or copy `package.json` + `package-lock.json` (production subset) into the runtime stage under
  `/app`, so image scanners resolve them.
- Add an image scan step (F10).

**Resolution / re-verification (0.0.5 → 0.0.9):**

- The Dockerfile copies `package.json` and `package-lock.json` into `/app` of the runtime stage so that
  scanners can resolve the bundled npm tree; verified in the published 0.0.9 amd64 image. The tag
  workflow's Trivy image scan runs on the published image before it is signed.
- **Not achieved: the SBOM.** The signed SPDX attestation on the 0.0.9 index digest (Docker Hub
  referrers, 2026-10-09) lists 17 packages: the Debian base, `node`, the root `@meddleware/sui-indexer`
  0.0.9 and the image, and **none** of the 24 bundled production packages (`@mysten/sui`, `@mysten/bcs`,
  `@mysten/seal`, `@noble/*`, `@protobuf-ts/*`, …). Syft's image scan reads installed packages
  (`node_modules/*/package.json`), not a lockfile, so shipping the lockfile did not change the SBOM. The
  CHANGELOG and README imply that SBOM tools now see the bundle; for the published SBOM they do not.
  Tracked as F18. Whether Trivy's image scan reads the shipped lockfile was not verified here (no Trivy).
- **Resolved 2026-10-18 (0.0.10, commit `4aab344`):** the release now attests a second SPDX SBOM generated
  from the lockfile (F18).

**Re-verified 2026-10-19 (0.0.12, Rust):** The bundle is gone. From 0.0.12 the image ships `Cargo.lock` in `/app` (for image scanners) and the release attests one CycloneDX SBOM per binary (F18). Disposition unchanged (RESOLVED).

### F5 — The bundle and image ship no third-party licence notices

**Severity:** Low   **Disposition:** RESOLVED (0.0.5, `1973fca`)
**Where:** `package.json` `build` script (esbuild bundle); `Dockerfile:36` (only `main.mjs` copied).

**Issue:**

- `grep -c '@license\|SPDX-License-Identifier\|Apache License' dist/main.mjs` returns **0**.
- The runtime image contains only `main.mjs`, with no `LICENSE` or `NOTICE` files. The bundled
  production dependencies include:

| Licence | Packages |
| --- | --- |
| Apache-2.0 | 7, including `@mysten/sui`, `@mysten/bcs`, `@mysten/seal`, `@protobuf-ts/grpcweb-transport` and `@protobuf-ts/runtime-rpc` |
| Apache-2.0 AND BSD-3-Clause | `@protobuf-ts/runtime` |
| MIT | 14, including `@noble/curves`, `@noble/hashes` and `@scure/base` |
| 0BSD | the two clients |

  These counts are from the lockfile; some packages may be tree-shaken out.
- Apache-2.0 §4(a) requires a copy of the licence with redistributions, and §4(d) any NOTICE text.
  MIT and BSD-3-Clause require the copyright notice and licence text.
- Publishing the image redistributes these packages.

**Impact:** licence non-compliance in a public image, the same class as seal-key-server F1.

**Remediation / evidence:**

- Build with `--legal-comments=linked` (or `external`) and generate `THIRD_PARTY_LICENSES` from the
  lockfile, e.g. with `npx license-checker-rseidelsohn --production --files`, or with esbuild's
  metafile plus each package's `LICENSE`/`NOTICE`.
- Copy that file into `/app` in the runtime stage.
- Check that the file is present in CI.

**Resolution (0.0.5, `1973fca`; re-verified at `a3369bb`):**

- `scripts/third-party-licenses.mjs` writes `dist/THIRD_PARTY_LICENSES` from the production packages in
  `package-lock.json` (licence and NOTICE texts; for a package that ships none, its declared licence).
  The Dockerfile runs it after the build and copies the file to `/app/THIRD_PARTY_LICENSES`. The
  published 0.0.9 image contains it (91.9 kB).
- CI runs `npm run check:licenses && test -s dist/THIRD_PARTY_LICENSES && grep -q 'Apache License' …`
  (`node-ci.yml`), and the release reuses that workflow.
- Residual, accepted: 24 production packages, of which four ship no licence file in the package
  (`@mysten/bcs`, `@mysten/seal`, `@mysten/utils`, `poseidon-lite`); the notice records their declared
  licence and the Apache-2.0 text is present through the sibling packages. The script's `--check` only
  requires `@mysten/sui` to be present, so a future package without a licence file would not fail CI.

**Re-verified 2026-10-19 (0.0.12, Rust):** Licence notices are generated by cargo-about 0.9.2 (`about.toml`, `about.hbs`) per binary, in the Dockerfile's `notices` stage (`--fail` on an unaccepted licence) and shipped as `/app/THIRD_PARTY_LICENSES-{api,ingest}.txt`; CI builds that stage and checks both files for the Apache-2.0 text. Every crate in the graph has a recognised licence (no clarifications needed; `ring` uses the cargo-about workaround). This also closes the 0.0.5 residual that a package without a licence file would not fail CI. Disposition unchanged (RESOLVED).

### F6 — Non-canonical URLs bypass the edge cache; per-IP limit weaknesses

**Severity:** Low   **Disposition:** RESOLVED (0.0.5, `1973fca`; NetworkPolicy seen 2026-10-09)
**Where:** `src/api.ts:63-90` (validation accepts many spellings), `:231-236` (limiter key);
`src/ratelimit.ts`; CLAUDE.md "Strict input … stops cache-busting parameters from reaching the origin
as new cache keys".

**Issue:**

- **Cache-key variants (probe).** Seven spellings of one query each returned `200`, and each is a
  distinct Cloudflare cache key:
  - `gate=0xa`, `gate=0xA`, `gate=0x0a`, `gate=0x000…0a`;
  - `limit=05` vs `limit=5`;
  - parameter order.

  `kinds` order (`A,B` vs `B,A`) and leading zeros in `before` are the same. Each `gate` value alone
  has hundreds of spellings: case per hex digit times 1–64 digits of padding.
- **IPv6.** The per-IP bucket is keyed on the full `CF-Connecting-IP`, so an IPv6 client with a /64
  has effectively unlimited buckets. The bucket map can grow by every new key until the next
  60-second sweep (`ratelimit.ts:164-169`).
- **Header trust.** Off-tunnel callers (another pod reaching the Service) can set `CF-Connecting-IP`
  freely. Requests without the header all share the tunnel's socket address. Both are documented in
  SECURITY.md, but no NetworkPolicy was seen.

**Impact:** together with F1, any client can drive the origin at its own chosen rate of cache misses.
The edge rate rule (not in this repo) is the remaining bound.

**Remediation / evidence:**

- Make every endpoint canonical: answer `301` (cacheable) to the canonical URL — lower-case,
  64-digit IDs, sorted unique `kinds`, no leading zeros, fixed parameter order — or `400` for
  non-canonical input.
- Key IPv6 buckets by /64.
- Cap the bucket map size (evict the oldest).
- Restrict the Service with a NetworkPolicy allowing only cloudflared.
- Correct the CLAUDE.md claim.

**Resolution (0.0.5, `1973fca`; re-verified at `a3369bb`):**

- **Cache keys.** `canonicalQuery` / `canonicalPath` put `gate` (and the stats path id) in 64-digit
  lower-case form, strip leading zeros from `limit`, and sort and de-duplicate `kinds`; any other
  spelling gets a cacheable `301` (`max-age=86400`) to the canonical URL, after the rate limit. The
  parameter order the client sent is kept (a bounded number of permutations; the client packages send
  one order). Tests: *redirects other spellings of a query to the canonical form, cacheably*, *serves
  canonical queries … without a redirect*, *the redirect target is itself canonical (no loops)*. Live:
  `limit=05` returns `301` to `limit=5`.
- **IPv6 and bucket map.** `clientKey` keys IPv6 clients by their /64; `RateLimiter` evicts the least
  recently used bucket past 50,000 keys. Test: *keys IPv6 clients by their /64 and caps the bucket map*.
  `If-None-Match` now accepts weak, listed and wildcard values (F9).
- **Off-tunnel callers.** `post-bootstrap/sui-indexer/base/networkpolicy.yaml` denies all ingress to the
  pod except from the `nginx-ingress` namespace and `platform-probe`, both on 8080; the Ingress adds a
  per-IP limit (20 r/s, burst 80, `429`) above the app's (10 r/s, burst 40). No other pod can forge
  `CF-Connecting-IP`. The first-pass suggestion "only cloudflared" does not fit the route (Cloudflare
  tunnel → nginx → pod); nginx is the admitted peer.
- CLAUDE.md now says "Canonical URLs … non-canonical spellings get a cacheable 301".
- Not set: the Cloudflare zone rate-limit rule (free plan, one rule used for the token service;
  documented in the infrastructure README). The edge cache and the ingress limit bound the load.

**Re-verified 2026-10-19 (0.0.12, Rust):** `canonical_query`/`canonical_path` (`crates/api/src/query.rs`) reproduce `URLSearchParams` parsing, serialisation and `set` semantics exactly (proptests: round trip, idempotence — no 301 loops — unique keys, path idempotence); `client_key` keys IPv6 by /64 and the limiter evicts the least recently used bucket past 50,000. Tests: all canonical-URL and transport cases of 0.0.11 ported. Live comparison: `limit=05` and an upper-case stats path get the same 301 `Location` as 0.0.11. Disposition unchanged (RESOLVED).

### F7 — Event types introduced by a future package upgrade are never indexed

**Severity:** Info   **Disposition:** ADJUDICATED (decision: new event types need a fresh publish; documented in `ef68727`, released with 0.0.7)
**Where:** `src/ingest.ts:31-35` (filter `<originalId>::access_gate` → SDK `eventTypeModule`),
`:54` (`parseAccessGateEvent(entry, originalId)`).

**Issue / Impact:**

- In Sui a struct's type address is the package version that first defined it.
- An event struct *added* in a compatible upgrade of `access_gate` is typed `<upgradeId>::access_gate::…`,
  so neither the module filter (original ID) nor the parser (original-ID prefix) matches it.
- Such events are silently absent; they are not even logged as skipped, because the node never
  returns them.
- Version-gated upgrades are the planned path (CLAUDE.md invariant 9 in access-gate-sui).

**Remediation / evidence:** record in both client packages' deployment records the set of type-origin
IDs, and poll a filter per origin. Or document that new event types require a fresh publish. Cross-ref
the access-gate-client audit (exact-type matching at the original ID).

**Adjudication (OQ3 decided; re-verified at `a3369bb`):**

- The behaviour is unchanged: the filter and the parsers use the **original** ID, so an event struct
  added by a compatible upgrade (typed at the upgrade's ID) is never indexed.
- The decision is that this project republishes before v0.2 rather than adding event types by upgrade
  (the 2026-10-09 republication did exactly that), so new event types are introduced with a fresh publish
  and a `REINDEX_CONFIRM` rebind (F3). Documented in CLAUDE.md ("New event types need a fresh publish")
  and the CHANGELOG (`ef68727`).
- Revisit for mainnet: if mainnet packages are ever upgraded to add events, the clients' deployment
  records need the type-origin IDs and the indexer one filter per origin.

**Re-verified 2026-10-19 (0.0.12, Rust):** Unchanged by design: `kind_of` (`crates/ingest/src/decode.rs`) accepts only types at the stream's **original** id and module, and `stream_filter` asks for `<original id>::access_gate` / the one sealed-content type. Disposition unchanged (ADJUDICATED).

### F8 — The full node is trusted completely; its chain identity is not checked

**Severity:** Info   **Disposition:** ADJUDICATED (display-only by design); the chain-identity suggestion is RESOLVED (0.0.5 `1973fca`, fixed in 0.0.6 `8573d0b`)
**Where:** `src/config.ts:47-54`; `src/main.ts:84`.

**Issue / Impact:**

- Rows are whatever `GRPC_URL` returns. Nothing compares the node's chain identifier with `NETWORK`,
  and nothing samples rows against a second node.
- A misconfigured `GRPC_URL` (a mainnet node for a testnet instance) indexes nothing, because the
  package IDs differ. That fails safe but silently: `/healthz` stays green.
- A compromised or MITM'd node (https limits this) could inject fabricated `SealedContentPublished`
  pointers.
  - seal-ui would list them as discovery results (cross-ref seal-client F6, discovery
    unauthenticated).
  - The clients decode them again but cannot check authenticity.

**Remediation / evidence:**

- At startup, read the chain identifier (`getChainIdentifier`) and refuse a mismatch with `NETWORK`.
- Optionally verify a sample of new rows by `getTransaction(digest)` against a second endpoint.
- Keep the display-only labelling in the apps.

**Resolution / adjudication (re-verified at `a3369bb`):**

- **Chain identity.** `main.ts` reads `client.core.getChainIdentifier()` for `testnet` and `mainnet` and
  exits 1 ("refusing to start") if the short id (first four bytes of the base58 genesis digest, `chain.ts`)
  differs from the one recorded for `NETWORK`. `localnet` has no recorded id and is not checked. Test:
  `tests/chain.test.ts` (maps the full digest to the short id; rejects non-base58 or short input). The
  first version (0.0.5) compared the full digest with the short id and never became ready (F16).
  The live service starts and ingests, so the check passes against the testnet node.
- **Authenticity.** Unchanged and accepted by design: rows are the node's, the clients decode again and
  cannot verify them; a second-node sample check (S5) was not built. The display-only label stays in the
  apps. A compromised node can still inject fabricated `SealedContentPublished` pointers (cross-ref
  seal-client F6).

**Re-verified 2026-10-19 (0.0.12, Rust):** `start()` (`crates/ingest/src/lib.rs`) reads `GetServiceInfo.chain_id` within 30 s and refuses a mismatch or an unreadable identifier; rows remain unauthenticated (display-only). The new trust in the node's JSON rendering is F24. Tests: *start_refuses_a_node_of_another_chain*, `chain.rs` tests. Disposition unchanged (ADJUDICATED).

### F9 — Type casts hide SDK drift; weak-ETag revalidation; config edges

**Severity:** Info   **Disposition:** RESOLVED (0.0.5, `1973fca`; one benign cast remains)
**Where:** `src/main.ts:84-85`; `src/api.ts:111-113`; `src/config.ts:48`.

**Issue / Impact:**

- **Casts.** `config.network as 'testnet'` and `client as unknown as EventSource` mean tsc never
  checks the real `SuiGrpcClient.core.listEvents` signature against the `EventSource` interface.
  - The `signal` support that F2's timeout depends on was verified only by reading the 2.33.2 source.
  - A future SDK change (renamed option, different cursor semantics) would compile.
  - Prefer `client.core satisfies EventSource['core']` or a typed adapter, plus a test against the
    real client's type.
- **Weak ETags.** The 304 path compares `If-None-Match` exactly to the strong ETag. Cloudflare weakens
  ETags when it compresses (`W/"…"`), and a list or `*` header never matches. Revalidation then
  always returns a full `200`. That costs efficiency only.
- **`NETWORK` is free-form.** An unknown network without `GRPC_URL` fails ("GRPC_URL is required").
  With `GRPC_URL` but no deployment (e.g. `mainnet` today), it throws from the client's deployment
  lookup, which is fail-closed and fine. `localnet` with overrides works. Document this.

**Remediation / evidence:** as above.

**Resolution (0.0.5, `1973fca`; re-verified at `a3369bb`):**

- **Casts.** `main.ts` now assigns `const source: EventSource = client` with no cast, so tsc checks the
  real `SuiGrpcClient.core.listEvents` signature (type-check is clean at `@mysten/sui` 2.35.0). The one
  remaining cast, `config.network as 'testnet'` when building the client, only satisfies the SDK's
  network union; the real network is bound by the chain-identifier check (F8).
- **ETags.** `etagMatches` implements RFC 9110 `If-None-Match` (a list, weak or strong tags, `*`). Test:
  *revalidates with weak, listed or wildcard If-None-Match*.
- **`NETWORK`.** Still free-form; unknown networks without `GRPC_URL` throw, networks without a
  deployment throw from the client's lookup (fail closed). See F20 for prototype-named values.

**Re-verified 2026-10-19 (0.0.12, Rust):** No casts exist any more: the source contract is the `EventSource` trait implemented by `GrpcSource` over the generated `sui-rpc` client (type-checked by rustc), and its frame-to-page semantics are pinned by unit tests, a proptest and a local LedgerService stub; ETag matching (`etag_matches`) keeps weak, listed and `*` values. Disposition unchanged (RESOLVED).

### F10 — CI and image pipeline gaps

**Severity:** Info   **Disposition:** RESOLVED (`ef68727`, released with 0.0.7; 2026-10-08); two best-effort residuals ACCEPTED-RISK
**Where:** `.github/workflows/*.yml`; `.dockerignore`; tests.

**Gaps:**

- **No image or config scan.** No Trivy or Grype on the pushed image and no Dockerfile config scan
  (IMG Section C). Given F4, a scan would currently see only Debian packages.
- **No container smoke test.** Nothing starts the built image (nonroot, `/data` writable,
  `/readyz` 200, headers) before push. `main.ts` has 0% coverage, and `Ingestor.start` is untested.
- **Context hygiene.** `.dockerignore` omits `.env*` and `*.db*`. The `COPY` is selective, so nothing
  leaks into layers, but a local `dev.db` is sent in the build context.
- **Verify job.** It runs no `npm run build`. The image build does it, but only after the verify job
  passes.
- **Mirror.** `publish-private` is `continue-on-error`, unsigned and unattested (documented
  best-effort). Since 2026-10-19 it is replaced by `mirror-private`, which copies the signed public image by digest.
- **Labels.** `VERSION` passes `github.ref_name` (`v0.0.4`) into `org.opencontainers.image.version`,
  whereas semver tags drop the `v`.
- **Referrer annotation.** The SPDX attestation's referrer annotation reads `cosign/sign/v1` although
  its payload is SPDX. That is a cosign quirk, but tools that filter referrers by annotation will miss
  the SBOM.

**Remediation / evidence:**

- Add `trivy image --exit-code 1 --severity HIGH,CRITICAL` (after F4) and `trivy config .`.
- Add a job that builds the image, runs it with a tmpfs `/data` and a fake or `localhost` gRPC URL,
  and probes `/readyz`, the headers and `id -u`.
- Extend `.dockerignore`.
- Strip the `v` for the version label.

**Resolution (`ef68727`; re-verified at `a3369bb`):**

- **Image and config scan.** `node-ci.yml` job `image` runs a Trivy configuration scan
  (`CRITICAL,HIGH`, exit 1), builds the image, scans it (`ignore-unfixed`, exit 1) and smoke-tests it.
  `docker-publish.yml` scans the **published** image by digest before cosign signs it. Trivy and the
  action are pinned (v0.74.0, SHA).
- **Container smoke test.** `scripts/smoke.sh` runs the image the way the cluster does (non-root, an
  empty tmpfs `/data`, no shell, an unreachable gRPC address) and checks the image user, `/readyz` 200
  with `nosniff`, and `/healthz` 503 naming the streams. It does not reach a chain, so the startup
  chain-identity check (F16) is not exercised.
- **Context hygiene.** `.dockerignore` excludes `.env*`, `*.db`, `*.db-*` and `*.bak-*`.
- **Verify job.** The release reuses `node-ci.yml` (`verify: uses: ./.github/workflows/node-ci.yml`), so
  type-check, lint, tests, the bundle build, the licence check, the config and image scans and the smoke
  test all run on the tagged commit; `publish-public` and `publish-private` `needs: [version, verify]`.
- **Labels.** The public job takes the label from `docker/metadata-action` (`0.0.9`, no `v`; verified on
  the published image config). The private-mirror job still passes `github.ref_name` (`v0.0.9`).
- **Verification command.** The release summary prints a `cosign verify` command pinned to
  `…/sui-indexer/\.github/workflows/docker-publish\.yml@refs/tags/v`.
- **Residuals, accepted.** (1) `publish-private` is `continue-on-error`, unsigned and unattested
  (documented best-effort; the mirror job fails without registry credentials, a maintainer item; replaced 2026-10-19 by the digest-preserving `mirror-private`). (2) The
  SBOM attestation's referrer annotation still reads `cosign/sign/v1` (verified on 0.0.9; a cosign quirk).
  The completeness of the SBOM is F18.

**Re-verified 2026-10-19 (0.0.12, Rust):** `rust-ci.yml` replaces `node-ci.yml`: fmt, clippy `-D warnings`, tests, coverage, cargo-deny, the SBOM gate, MSRV, the client-contract job, Trivy config scan, licence notices, image build, Trivy image scan and the two-container smoke test (`scripts/smoke.sh`, now also checking the read-only API mount, the `503` before the database exists, the exec healthcheck and serving after the ingester stops). The release reuses it (`verify: uses: ./.github/workflows/rust-ci.yml`); the tag is compared with the Cargo workspace version. Every action stays SHA-pinned (pins copied from gateway-rust and the previous workflows). Disposition unchanged (RESOLVED).

### F11 — Documentation drift

**Severity:** Info   **Disposition:** RESOLVED (2026-10-19: the last residual, the infrastructure retention wording, is fixed in the workspace manifests; MITIGATED before)

- CLAUDE.md "Strict input … stops cache-busting parameters" overstates it (F6).
- README "About 110 MB RSS at 3.5k req/s on a laptop" should state the database size (F1).
- README `GRPC_URL` says "https required"; `localhost` and `127.0.0.1` over http are also accepted
  (`config.ts:52`).
- SECURITY.md pod hardening (read-only root FS, capabilities dropped) and "reachable only through the
  Cloudflare tunnel" are claims about manifests outside this repo. Link them, or record them as
  assumptions.
- No document warns that changing or removing an `*_ORIGINAL_ID` override irrecoverably drops pruned
  history (F3).

**Re-verification (`a3369bb`):**

- Fixed: CLAUDE.md no longer overstates strict input (it names the canonical-URL redirect, F6); SECURITY.md's
  pod claims (read-only root filesystem, all capabilities dropped, reachable only through the tunnel/ingress)
  are now backed by the manifest and NetworkPolicy read this pass (F6, IMG baseline); the README documents
  that a package change refuses to start until `REINDEX_CONFIRM` is set and that the pruned history
  cannot be recovered (F3).
- Residual 1: README still says "About 110 MB RSS at 3.5k req/s on a laptop" without the database size.
  Fixed 2026-10-18 (0.0.10): the README now labels it a one-off measurement against a small database, not a
  benchmark (the original database size was not recorded, so none is stated).
- Residual 2: README's `GRPC_URL` row says "https required"; `localhost` and `127.0.0.1` over http are also
  accepted (`config.ts`). Fixed 2026-10-18 (0.0.10).
- Residual 3: the CHANGELOG keeps released items (CI gate, `.dockerignore`, the new-event-type note) under
  `[Unreleased]` although they shipped in 0.0.7–0.0.9, and the infrastructure PVC comment and README
  ("rebuildable from the chain") do not mention the node's 5½-day retention (F3). The CHANGELOG half is
  fixed 2026-10-18 (the items now sit under 0.0.7); the infrastructure wording is outside this repo and
  stays open (one residual item).

**Re-verified 2026-10-19 (0.0.12, Rust):** The docs are rewritten for Rust (README, CLAUDE.md, AGENTS.md, SECURITY.md, CHANGELOG). The last residual — the infrastructure PVC comment and README saying the index is "rebuildable from the chain" — is fixed in the workspace (`post-bootstrap/sui-indexer/base/pvc.yaml` and README now state the node's ~5½-day retention). **Disposition: RESOLVED** (2026-10-19; workspace files, which are not under git).

### F12 — Positive: ingest ordering, atomicity and generation semantics are sound

**Severity:** Positive

- **Ascending reads with cursor commit.** `order: 'ascending'` + `after: cursor`; the SDK rejects
  `after` with descending, so ordering is enforced. Each page commits with its cursor in one
  transaction (`store.ts:139-173`). `INSERT OR IGNORE` on `(tx_digest, event_index)`. A null
  `endCursor` keeps the position.
- **Row id = ledger order.** `INTEGER PRIMARY KEY` IDs exceed every existing row, so a page bounded
  by `id < before` never gains rows. After a clear, the generation rotates, so a reused ID never meets
  an old cursor.
- **Stable binding.** Override IDs are validated and normalised; the client default IDs are already
  long-form (probe). A cosmetic change therefore never clears a stream.
- **Bounded polls.** 20 pages per stream per poll, and a 30-second abortable timeout per call
  (verified in the SDK).
- **One parser.** The published client parsers are used against exact original IDs. The `bcs`,
  `eventType`, `sender`, `checkpoint` and digest are served verbatim, so the clients decode again.
  The tests encode fixtures independently of the parsers and include a client-contract test through
  the handler.

**Re-verified 2026-10-09 (`a3369bb`):** still true, and extended. Ordering, atomicity and the generation
rule are unchanged; the stored cursor now also survives the node's retention (a counted gap, F15); the
binding check is stricter (F3). The 2026-10-09 rebind of both streams to the new packages kept the
rule: the generation rotated (`e32e37afbbe9` live) and older cursors return `410`.

**Re-verified 2026-10-19 (0.0.12, Rust):** Ascending reads from the stored cursor, page + cursor in one transaction (`Store::ingest`), `INSERT OR IGNORE`, a missing end cursor keeping the position, the 20-page budget and the 30 s per-request timeout are ported (`Ingestor::poll_stream`). The cursor encoding is unchanged — base64 of the watermark cursor bytes with `core.listEvents`' page rules (`PageBuilder`) — verified live: the Rust source and `@mysten/sui` 2.36 return the same `endCursor` (`KAFCDginh429ARDNzazFDxgA`) for the same page. Positive stands.

### F13 — Positive: a small, strict public API

**Severity:** Positive

- **Methods and validation.**
  - GET, HEAD and OPTIONS only (`405` otherwise).
  - Unknown parameters `400`; IDs 1–64 hex, normalised; limits bounded; cursors pattern-checked and
    generation-bound (`410`).
  - `kinds` checked against the client's `ACCESS_GATE_EVENT_KINDS`.
  - All SQL parameterised.
- **Headers.**
  - `nosniff`; CORS `*` without credentials (correct for public data behind a cache that ignores
    `Vary`).
  - `no-store` on errors and `429` (with `Retry-After`).
  - Immutable older pages; short-TTL head page with an ETag.
- **Server limits.** `requestTimeout` 10 s and `headersTimeout` 5 s. Failures return a generic
  `500`, with the stack logged server-side only.
- **Health split.** `/readyz` (liveness) is separate from `/healthz` (poll freshness), so stale data
  keeps being served while the node is down.
- **Exact money.** Commission is summed as `bigint` with `setReadBigInts`, so values above 2^53 never
  round.

**Re-verified 2026-10-09 (`a3369bb`):** still true, and extended with canonical-URL redirects (F6), weak
and wildcard revalidation (F9), per-stream `/healthz`, `polledAt` and `latestCheckpoint` on head pages
(F2), and the `/coverage` counters. Live responses carry `nosniff`, `Access-Control-Allow-Origin: *` and
the documented cache headers.

**Re-verified 2026-10-19 (0.0.12, Rust):** Every route, status, body (byte-identical key order), header and timeout is ported (`crates/api/src/handler.rs`, `server.rs`); bigints are decimal strings (`u128` commission). New: the API cannot write (F22), a connection cap (F26), and `503` until the database exists. Positive stands.

### F14 — Positive: image hardening and release attestations

**Severity:** Positive

- **Image.** Builder and runtime pinned by digest. Distroless `nodejs24-debian13:nonroot`, with no
  shell or package manager. `USER 65532`. Only the bundle is copied. `npm ci` from the lockfile.
  Label-only build args.
- **Release.**
  - Tag = version checked before build; SHA-pinned actions; least privilege per job.
  - Multi-arch.
  - Keyless cosign signatures, BuildKit SLSA provenance (`mode=max`), a GitHub build-provenance
    attestation and a signed SPDX SBOM attestation, all verified as present on Docker Hub's 0.0.4
    index digest.

**Re-verified 2026-10-09 (`a3369bb`, published 0.0.9 = `sha256:aea58153…1704`):**

- Builder digest unchanged; the runtime base moved to `distroless/nodejs24-debian13:nonroot@sha256:9eeb7f58…278e`
  (F17). The published image runs as `65532:65532`, carries label version `0.0.9`, and contains only the
  bundle, `package.json`, `package-lock.json` and `THIRD_PARTY_LICENSES`.
- Quay and Docker Hub resolve the tag to the same index digest, equal to `config/images.yaml` and the
  deployed overlay. Referrers on Docker Hub: cosign signature, GitHub build-provenance attestation and the
  signed SPDX SBOM attestation (the SBOM's content: F18). The release now runs the full CI, scans the
  published image before signing and prints an identity-pinned `cosign verify`.

**Re-verified 2026-10-19 (0.0.12, Rust):** 0.0.12: builder `rust:1-slim` and runtime `distroless/cc-debian13:nonroot` pinned by digest; `USER 65532:65532`; only the two binaries, `Cargo.lock`, `LICENSE` and the notices reach the runtime stage; `cargo build --locked`; release profile with overflow checks and `panic = "abort"`. The release keeps multi-arch, cosign keyless signing, BuildKit provenance, the GitHub provenance attestation, the syft image SBOM and the digest-preserving `mirror-private`, and replaces the npm SBOM with per-binary Cargo SBOMs (F18). Not yet exercised on a release. Positive stands.

### F15 — A stream whose cursor falls below the node's retention retried forever (found and fixed after the first pass)

**Severity:** Low   **Disposition:** RESOLVED (0.0.5, `1973fca`)
**Where:** `src/ingest.ts` `pollOne` (`PRUNED` pattern, `resetCursorAfterGap`); `src/store.ts`
`resetCursorAfterGap`.

**Issue:**

- The first pass assumed a quiet stream is harmless. A stream that receives no events for longer than
  the node's retention (about 5½ days on testnet) keeps a cursor that the node no longer serves; the
  poll then fails with "requested data below earliest available" on every interval, `/healthz` stays red
  and the stream never recovers without manual action.
- The first-pass audit recorded the stuck-ingest class under F2 for a poison event only.

**Resolution:**

- On an error that matches `PRUNED` for a stream with a stored cursor, the ingestor deletes that cursor,
  counts a `gap` in `stream_stats` (and stores the time and reason in `meta` as `gap:<stream>`), and
  re-polls from the node's earliest data. Rows already stored are untouched (re-inserting is a no-op).
  Other errors are not treated as gaps. Tests: *restarts from the node's earliest data when the stored
  cursor fell below its retention*, *does not swallow other failures as a gap*. `/v1/{network}/coverage`
  reports `gaps` (live value: 0).
- The events between the old cursor and the node's earliest data are gone for good; the counter makes the
  loss visible.

**Re-verified 2026-10-19 (0.0.12, Rust):** Ported: `is_pruned` (same three phrases, case-insensitive) and `Store::reset_cursor_after_gap` (cursor deleted, gap counted, `meta gap:<stream>` with an ISO time and the first 200 UTF-16 units of the reason, as before). Tests: *restarts_from_the_nodes_earliest_data_…*, *does_not_swallow_other_failures_as_a_gap*, *a_pruned_message_without_a_stored_cursor_is_a_plain_failure*, and the gRPC stub returns an `OutOfRange` status that `is_pruned` recognises. Disposition unchanged (RESOLVED).

### F16 — Startup refused the real testnet node because the chain identifier was compared in the wrong form (found and fixed after the first pass)

**Severity:** Low   **Disposition:** RESOLVED (0.0.6, `8573d0b`)
**Where:** `src/chain.ts`, `src/main.ts` (chain-identity check added in 0.0.5 for F8).

**Issue:**

- 0.0.5 compared the node's full base58 genesis digest with the short id (`4c78adac`) and exited 1 on
  the real testnet node: "0.0.5 never became ready". No test reached the startup path (`main.ts` is
  untested and the container smoke test uses `localnet`, which has no recorded id), so the defect was
  found by running the release.

**Resolution:**

- `shortChainId` base58-decodes the identifier and takes the first four bytes in hex; `KNOWN_CHAIN_IDS`
  holds `testnet: 4c78adac` and `mainnet: 35834a8a`. Test: `tests/chain.test.ts` (maps a real testnet
  digest; rejects non-base58 or short input). The live service has run 0.0.6–0.0.9 against the public
  testnet node.
- Residual: nothing in CI starts the bundle against a real or faked node that reports a chain id. This
  is the same gap as the container smoke test (F10) and is recorded in Section C.

**Re-verified 2026-10-19 (0.0.12, Rust):** `short_chain_id` (`crates/ingest/src/chain.rs`, `bs58`) maps the real testnet and mainnet digests to `4c78adac` / `35834a8a`. The 0.0.5 residual is closed: the startup path is now tested (*start_refuses_a_node_of_another_chain* with the mainnet digest against `NETWORK=testnet`; *the_grpc_source_reads_list_events_frames_and_the_chain_identifier* through the real client), and the local run against the public testnet node passed the check. Disposition unchanged (RESOLVED).

### F17 — The distroless runtime base carried fixable `libssl3t64` HIGH advisories (found by the new scan gate, fixed in 0.0.8)

**Severity:** Low   **Disposition:** RESOLVED (0.0.8, `70fa4df`)
**Where:** `Dockerfile` runtime `FROM` digest; `.github/workflows/{node-ci,docker-publish}.yml` Trivy
steps.

**Issue:**

- The Trivy image gate added in `ef68727` (F10) failed the 0.0.7 release on fixable HIGH advisories in
  `libssl3t64` of the pinned distroless digest. The pin had kept the vulnerable layer (the cost of
  digest pinning), and the first pass had no scanner to see it.

**Resolution:**

- 0.0.8 bumped the runtime base to `gcr.io/distroless/nodejs24-debian13:nonroot@sha256:9eeb7f58…278e`.
  The published 0.0.9 SBOM lists `libssl3t64` 3.5.7-1~deb13u3 and the release's published-image scan
  passed (the image was signed). Image scan at the review date: not re-run here (no Trivy); the gate
  passing is the evidence. The release path is the control that will catch the next advisory.

**Re-verified 2026-10-19 (0.0.12, Rust):** Superseded base: 0.0.12 runs on `distroless/cc-debian13:nonroot` (the digest gateway-rust ships); the CI and release Trivy gates are unchanged and will judge it. Not scanned locally (no Trivy). Disposition unchanged (RESOLVED).

### F18 — The signed SBOM still omits the bundled npm packages

**Severity:** Low   **Disposition:** RESOLVED (0.0.10, commit `4aab344`; the new attestation was verified on the released 0.0.10 digest on 2026-10-18)
**Where:** `.github/workflows/docker-publish.yml` ("Generate SBOM": `anchore/sbom-action` with `image:`);
`Dockerfile` (lockfile copied to `/app`).

**Issue:**

- Verified on the 0.0.9 SBOM attestation (Docker Hub referrers, 2026-10-09): 17 packages, none of the 24
  bundled production dependencies. Syft's image scan finds installed packages, not a lockfile, so the
  F4 mitigation (shipping `package-lock.json`) does not reach the SBOM. The CHANGELOG and README say the
  lockfile ships "so SBOM tools see the bundled npm packages"; that holds for tools that read lockfiles
  in images, not for this SBOM.

**Impact:** SBOM-driven vulnerability management on the signed attestation cannot see a CVE in
`@mysten/sui`, `@noble/*` or `@protobuf-ts/*`. `npm audit` in CI and Dependabot cover the lockfile at
build time; the Trivy image scan may read the shipped lockfile (not verified).

**Remediation / evidence (2026-10-18, 0.0.10, commit `4aab344`):**

- `docker-publish.yml` now installs the lockfile (`npm ci --ignore-scripts`), writes the production
  dependency tree with `npm sbom --sbom-format spdx --omit dev` (23 packages plus the root, from the
  lockfile) and attests it with cosign next to the image SBOM, to both registries. Two SPDX attestations
  per digest, because syft's image scan cannot be made to see a bundle; `cosign verify-attestation --type
  spdxjson` returns both (README → SBOMs).
- `scripts/check-sbom.mjs` fails the step unless the SBOM lists every production package of
  `package-lock.json` at its locked version (a root dev dependency that is only an uninstalled peer is not
  expected). `node-ci.yml` runs the same generation and check, so it also gates the release (`verify`).
  Test: `tests/sbom.test.ts` (complete, a package missing, a version mismatch, an empty SBOM).
- Not run here: the publish workflow itself (needs the tag and registry credentials) and `cosign`; the
  commands were run locally (`npm sbom` and the check pass). **Post-release check done 2026-10-18:** the released
  0.0.10 digest `sha256:b3484ce4…8a58` carries two SPDX attestations, the image SBOM (17 packages) and the npm
  packages SBOM (24 packages), both verified with `cosign verify-attestation --type spdxjson`.

**Re-verified 2026-10-19 (0.0.12, Rust):** The release now attests, next to the syft image SBOM (SPDX), one CycloneDX SBOM per binary from `cargo cyclonedx --no-build-deps --target x86_64-unknown-linux-gnu` (0.5.9, pinned through `taiki-e/install-action`). `scripts/check-sbom.sh` (bash + jq, no Node) fails unless each SBOM lists every crate reachable from the binary's package through normal (non-dev, non-build) edges of `cargo metadata --locked`, at its locked version; it runs in CI (`check` job, with its own 10-case test script) and in the release before `cosign attest --type cyclonedx`. Local run: `api` 63/63, `ingest` 139/139. Not yet exercised on a release (post-release check: `cosign verify-attestation --type cyclonedx`). Disposition unchanged (RESOLVED).

### F19 — The clients do not use the freshness fields the indexer now serves

**Severity:** Low   **Disposition:** DEFERRED (pre-mainnet gate: *clients treat a stale head page as unavailable*, Section D)
**Where:** `access-gate-client/src/events.ts` and `seal-client/src/sealed-content.ts` (indexer readers);
`src/api.ts` `eventsPage` (`polledAt`, `latestCheckpoint`).

**Issue:**

- Head pages carry `polledAt` and `latestCheckpoint` (F2), but the indexer readers in access-gate-client
  0.0.8 and seal-client 0.0.19 do not read them (no `polledAt`, `latestCheckpoint` or staleness check in
  `src/`). Apps fall back to the full node on errors and timeouts only, so a stuck indexer that still
  answers `200` shows frozen history in treasury-ui and seal-ui until it recovers.

**Impact:** stale display data for as long as an ingest failure lasts. The status-page check (F2) makes the
outage visible to the operator, not to the apps. No funds or access are affected (display-only).

**Remediation / evidence (client change, coordinated with the two client audits):** have `readIndexerEvents`
treat a head page whose `polledAt` is older than N poll intervals as unavailable and fall back to the node;
add a test per client. Deferred to the pre-mainnet gate; not a testnet blocker.

**Note (2026-10-18):** the change lives only in access-gate-client and seal-client (`readIndexerEvents`);
this repo already serves `polledAt` and `latestCheckpoint` and its contract tests pin them. Nothing to change
here, so the finding stays DEFERRED on those two repos. The two clients' choice of N (a multiple of the poll
interval, 15 s by default) is theirs to make; recommendation: 4 intervals (60 s).

**Re-verified 2026-10-19 (0.0.12, Rust):** Unchanged: the API still serves `polledAt` and `latestCheckpoint` (now from the heartbeat rows, F23), and the client change remains in access-gate-client and seal-client. Disposition unchanged (DEFERRED).

### F20 — Operator-keyed lookups use plain objects (found by the TS lens *Caller-keyed lookups* check)

**Severity:** Info   **Disposition:** RESOLVED (0.0.10, commit `4aab344`; adjudicated hygiene, now fixed)
**Where:** `src/config.ts` (`DEFAULT_GRPC[network]`), `src/main.ts` (`KNOWN_CHAIN_IDS[config.network]`).

**Issue / adjudication:**

- Both maps are indexed by the `NETWORK` environment value without `Object.hasOwn`. A value such as
  `constructor` or `toString` resolves to a function: `new URL(<function>)` throws in `loadConfig`, or with
  `GRPC_URL` set the chain-identity comparison fails against a non-string and the process exits 1. The
  client packages' deployment lookups already use own-property checks. The value is operator-controlled
  (no request path reaches these lookups; the API compares `parts[1]` with the configured network string),
  so this is hygiene, not an exposure. Not changed in this pass.
- If the code is touched again, use `Object.hasOwn(DEFAULT_GRPC, network)` and a `Map` for the chain ids.
- Re-verified 2026-10-09 at `a3369bb`: unchanged (`config.ts:24,64`, `main.ts:31`; `chain.ts` types the map
  `Readonly<Record<string, string>>`). No code change was made in this alignment pass.
- **Fixed 2026-10-18 (0.0.10, commit `4aab344`):** `DEFAULT_GRPC` (`config.ts`) and `KNOWN_CHAIN_IDS`
  (`chain.ts`) are `Map`s read with `.get`, so an inherited name is simply unknown (`constructor` without
  `GRPC_URL` is `GRPC_URL is required for network constructor`; with it, the clients' deployment lookup
  rejects it). Tests: `config.test.ts` (*treats NETWORK as data*), `chain.test.ts` (*has no inherited
  entries*). No other caller-keyed plain-object lookup exists in `src/`: the stats `counts` object is keyed
  by kinds this process wrote from the client parsers, and `state[stream]` is keyed by the `Stream` union.

**Re-verified 2026-10-19 (0.0.12, Rust):** Rust has no prototype lookups; the operator-keyed tables are slices searched by exact string (`DEFAULT_GRPC`, `DEPLOYMENTS`, `KNOWN_CHAIN_IDS`). Tests: *treats_network_as_data* (config) and *has_no_inherited_entries* (chain) keep the F20 cases. Disposition unchanged (RESOLVED).

### F21 — `noUncheckedIndexedAccess` is off (found by the TS lens *Compiler strictness* check, 2026-10-09)

**Severity:** Info   **Disposition:** RESOLVED (0.0.10, commit `4aab344`)
**Where:** `tsconfig.json` (`strict: true`, no `noUncheckedIndexedAccess`, `skipLibCheck: true`);
`src/api.ts:251`, `src/store.ts:316`.

**Issue / evidence:**

- The TS lens asks that the state of `noUncheckedIndexedAccess` is recorded, and expects it on for modules
  that parse untrusted data. This service parses HTTP input and node events, so the flag is wanted.
- Measured read-only: `npx tsc --noEmit --noUncheckedIndexedAccess` reports 2 errors in `src/` and 5 in
  `tests/`. Both `src/` sites are guarded: `api.ts:251` is `parseId(parts[3], 'gate id')!` after the route
  matched five path parts and the id was validated (a non-null assertion on a validated value);
  `store.ts:316` is `page[page.length - 1].id` inside `more`, which is only true when the page is not empty.
- `skipLibCheck` hides only declaration files of dependencies; the project's own sources are checked
  (`include: src, tests`), and tsc is clean without the flag.
- Casts on the trust boundaries are SQLite row shapes written by this process (`store.ts`, typed row
  reads), `streams as Stream[]` after a per-item check (`config.ts:53`), `as const` tuples, and the benign
  `config.network as 'testnet'` (F9). No `any`, no `eslint-disable`, no `eval`/`new Function`/dynamic
  `import()`, no `btoa`/`atob` (`Buffer` base64 only, `api.ts:111`, `ingest.ts:44`).

**Impact:** none today (both sites are guarded). The risk is a future edit that indexes an array or row
without a guard.

**Remediation / evidence:** enable `noUncheckedIndexedAccess`, fix the 2 sites in `src/` and the 5 in tests
(a small change; not made in this pass, which makes no code changes). Accepted for testnet because the
service is display-only and the two sites are guarded.

**Resolution (2026-10-18, 0.0.10, commit `4aab344`):** `noUncheckedIndexedAccess` is on in `tsconfig.json`
(`src` and `tests`); `tsc --noEmit` is clean.

- `api.ts` (gate stats route): `parseId(parts[3] ?? null, 'gate id')` and a `400` if it is absent, instead of
  a non-null assertion.
- `store.ts` (`page`): `next` is `page.at(-1)?.id ?? null` when there is a further page.
- Tests: the ETag header is narrowed with a thrown error, the ingest mock call and stored row are checked
  rather than asserted, and the shared `pages` object no longer relies on a computed string key.
  No test or rule was weakened. The flag now guards the type-check in CI and the release gate.

**Re-verified 2026-10-19 (0.0.12, Rust):** Superseded: there is no TypeScript in the service. The Rust analogue — indexing without a guard — is covered by the RUST lens (A-panics): the only indexing on request paths follows a length check (`parts[3]` after `parts.len() == 5`), and `rows.last()` is an `Option`. Disposition unchanged (RESOLVED).

### F22 — Positive: the API process cannot write the index (Rust rewrite, 0.0.12)

**Severity:** Positive
**Where:** `crates/store/src/lib.rs` (`Reader::open`: `SQLITE_OPEN_READ_ONLY`, `PRAGMA query_only = ON`);
`crates/api` (no chain client, no write path); workspace `post-bootstrap/sui-indexer/base/deployment.yaml`
(`api` mounts `/data` with `readOnly: true`).

- Up to 0.0.11 one Node process held the writable database, the gRPC client and the public HTTP server. In
  0.0.12 the HTTP server is a separate process whose database handle, mount and code have no write path:
  three independent layers (read-only mount, read-only open flag, `query_only`). A compromise of the
  request path can at worst serve wrong pages (display-only), not alter the index or its cursors.
- Separate crash and restart domains: the API keeps serving while the ingester backs off or restarts, and
  ingestion continues if the API restarts. The writer keeps the `-wal`/`-shm` files on close
  (`SQLITE_FCNTL_PERSIST_WAL`, the one `unsafe` FFI call, justified in the source), because a read-only
  mount cannot create them; verified by the smoke test (the API keeps answering after the ingester stops).
- Before the ingester has created the database, the API answers `503` (`/readyz` too, so the pod is not
  Ready) and never creates a file.
- Tests: *the_reader_cannot_write_and_sees_new_commits*, *the_reader_is_not_ready_until_the_writer_creates_the_database*,
  *the_wal_files_outlive_the_writer_so_a_read_only_mount_can_open_them*,
  *before_the_ingester_creates_the_database_everything_but_options_is_503*; `scripts/smoke.sh` steps 1, 2 and 4.

### F23 — Health and liveness across two processes (Rust rewrite, 0.0.12)

**Severity:** Info   **Disposition:** RESOLVED (0.0.12, commit `1fcbbe9`)
**Where:** `crates/ingest/src/ingestor.rs` (`poll_one` writes `meta poll:<stream>`; `alive` writes
`meta ingest:alive`); `crates/api/src/handler.rs` (`healthz`, head-page `polledAt`); `crates/ingest/src/lib.rs`
(`healthcheck`).

**Issue (design consequence of the split):** `/healthz` and `polledAt` used the poller's in-memory state,
which the API process no longer has; and a distroless container has no shell for an exec probe.

**Resolution:**

- The ingester writes each stream's state after every poll as `meta poll:<stream>` =
  `{"lastSuccessAt", "lastError", "lastAttemptAt"}`; the API reads it, with the same 5 × `POLL_MS` rule and the
  same `/healthz` JSON. The state survives an ingester restart (the 0.0.11 state reset to 0 on restart).
  `POLL_MS` is now also read by the API (the deployment sets it on both containers).
- `ingest --healthcheck` (the exec liveness probe) passes while `meta ingest:alive` was refreshed within
  `5 × POLL_MS + 30 s`; the ingester refreshes it before and after every full-node request (each bounded by
  30 s), so a stuck process fails it but a full node that is down does not (a restart would not help).
- Tests: *polls_the_streams_independently_and_health_names_the_failing_one* (heartbeat rows),
  *a_restart_continues_from_the_persisted_heartbeat*, *the_healthcheck_follows_the_liveness_heartbeat*, the
  ported `/healthz` cases; smoke test step 3.
- Residual: if the database write of a heartbeat fails, `/healthz` shows the stream stale — failing towards
  "unhealthy", which is the safe direction.

### F24 — Stored columns come from the full node's JSON rendering, not from the BCS (Rust rewrite, 0.0.12)

**Severity:** Info   **Disposition:** ADJUDICATED (the "one parser" invariant kept by not decoding; a parity gate pins the equivalence)
**Where:** `crates/ingest/src/decode.rs` (`kind_of`, `to_indexed`); `crates/ingest/src/source.rs` (`READ_MASK`
includes `json`); CLAUDE.md "JSON fields relied on".

**Issue:** the rewrite must not copy the client packages' BCS layouts ("one parser"). The three columns
queries filter on — `kind`, `gate_id`, `commission_mist` — are therefore derived without decoding: `kind`
from the event type's struct name, `gate_id` and `commission_mist` from the node's `json` rendering. The
BCS bytes are stored and served unchanged, and the clients decode them as before. The node's rendering
becomes a second input that must agree with the BCS.

**Adjudication / evidence:**

- The rendering uses the Move field names (`gate_id` as `0x…` hex, `u64` as a decimal string); checked against
  `access_gate.move`, `sealed_content.move` and live testnet events (CLAUDE.md table).
- **Parity gate (2026-10-19, read-only):** every event the testnet archive serves for `access_gate`
  `0xd7ddaa94…` (current, 11), `0xa55789d7…` (4) and `0x1a81ca17…` (4) was run through both paths — the Rust
  JSON derivation and 0.0.11's `toIndexed` (the client parsers on the BCS). **19/19 identical** (kinds
  GateCreated 3, AccessMinted 7, AccessConsumed 9). No `SealedContentPublished`, `AccessBurned`,
  `GateFrozen`, `GateMadeFree`, `PlatformConfigUpdated` or `PlatformMigrated` event exists on testnet for
  these packages, so for those kinds the field names rest on the Move sources. The comparison script was
  run from a scratch directory and is not kept in the tree (the TypeScript path no longer exists).
- Strictness: an id must match `^0x[0-9a-fA-F]{1,64}$`, a `u64` must be a digit string (or an exact JSON
  integer) that fits 63 bits; anything else makes a recognised event **undecodable** (counted and skipped,
  F2), never a guessed value. Tests: *a_recognised_event_without_what_the_row_needs_is_undecodable* (ten
  mutations), *keeps_kind_gate_and_commission_from_the_json_rendering*.
- A divergence between rendering and BCS could only mis-file a row (wrong gate filter or stats), not change
  what is served: the BCS is the node's, and the clients re-decode and type-check it (display-only, F8).
- Lost by design: 0.0.11's strict re-encode check (a layout drift made the event undecodable at ingest).
  Drift is now caught where the BCS is decoded — in the clients, which skip and count invalid rows.
- Revisit: when the first sealed-content or gate-lifecycle events appear on testnet, re-run the parity
  check (Section D, pre-mainnet).

### F25 — A `kinds` filter for a rare kind walks the stream (found 2026-10-19 by the load test)

**Severity:** Low   **Disposition:** DEFERRED (pre-existing in 0.0.4–0.0.11; an additive index after the 0.0.12 cutover, OQ5)
**Where:** `crates/store/src/lib.rs` `read::page` (`WHERE stream = ? AND kind IN (…) [AND gate_id = ?]
ORDER BY id DESC LIMIT ?`); the schema's indexes are `(stream, id)` and `(stream, gate_id, id)` only (same as
`store.ts`).

**Issue (measured):** without an index on `kind`, SQLite walks `events_stream` backwards until it has found
`limit + 1` matching rows. For a kind that is rare in a large stream the page reads the whole stream: with
300,000 `AccessMinted` rows and 5 `AccessConsumed`, `kinds=AccessConsumed` served **10 req/s** on 4 CPUs (the
unfiltered head page 14k req/s). A `gate=` filter is unaffected (it uses `events_gate`). F1's counters do not
cover this path. The same query and schema ran in every TS release.

**Impact:** each such cache miss costs a stream scan; within the per-IP limit (10 r/s), the ingress limit and
the edge cache, a handful of clients can still occupy the API's workers. Live index today: 11 rows, so no
current effect.

**Remediation (recommended, not applied):** `CREATE INDEX IF NOT EXISTS events_kind ON events (stream, kind, id)`.
It is additive (0.0.11 ignores it, so a rollback still works) but it is a schema change, and the 0.0.12
cutover is deliberately schema-identical; apply it in the next patch with a page-cost test for a rare kind.

### F26 — The API had no connection cap (found 2026-10-19 by the RUST lens *HTTP hygiene* check)

**Severity:** Info   **Disposition:** RESOLVED (0.0.12, commit `ddfc77d`)
**Where:** `crates/api/src/server.rs` (`serve`).

**Issue:** the RUST lens asks for a concurrency limit that keeps buffering × concurrency within the
container's memory. The first Rust build (like 0.0.11) accepted connections without a cap, each with a read
buffer of up to hyper's default ~400 KB for the request head.

**Resolution:** at most `MAX_CONNECTIONS = 1024` connections are served at once (a semaphore; further ones wait
in the listen backlog) and the request-head buffer is capped at `MAX_BUF = 32 KiB` (`max_buf_size`; larger
heads are refused). Request bodies are never read. Worst case ≈ 32 MiB of head buffers, inside the 128 MiB
limit. Test: *an_oversized_request_head_is_refused*; the cap itself is a constant (not load-tested).

---

## Section A — Invariant verification matrix

Re-verified 2026-10-19 against the Rust code (0.0.12): file references below are the Rust ones where the
row says so; the TS-lens rows A16–A24 are replaced by RUST-lens rows (A16–A24 now) and A38–A43 are new.

| # | Invariant | Enforced at | Proven by | Status |
| --- | --- | --- | --- | --- |
| A1 | Display-only; rows verbatim; clients decode again and fall back | design; `serialize` | client-contract tests | HOLDS (clients do not yet use `polledAt` — F19) |
| A2 | One parser (the client packages); the indexer never decodes BCS | `crates/ingest/src/decode.rs` (type name + node JSON, BCS stored verbatim) | ingest tests; client-contract job; parity 19/19 (F24) | HOLDS (upgrade-defined types need a fresh publish — F7, decision) |
| A3 | Ledger order = row id; pages bounded by `before` immutable | `store.ts`, `ingest.ts` | ingest tests; reasoning about rowid | HOLDS |
| A4 | Page + cursor atomic | `Store.ingest` | tests | HOLDS |
| A5 | Generation rotates on rebuild or rebind; old cursors `410` | `Store`, `parseBefore` | tests; live generation rotated by the 2026-10-09 rebind | HOLDS |
| A6 | Package binding never mixes packages, and never deletes history unconfirmed | `bindPackages`, `main.ts` | *Store.bindPackages* tests (refuse, confirm, backup, legacy rows) | HOLDS (F3; the backup shares the volume) |
| A7 | Strict input; one cache entry per query | `api.ts` | validation and canonical-URL tests | HOLDS (F6) |
| A8 | Cheap under load | counters; cache headers; limiter | *Store counters* (50k-row constant-time read) | HOLDS (F1) |
| A9 | Ingest progresses despite a bad event; one stream cannot stall the other; a pruned cursor recovers | `Ingestor.pollOne` | resilience tests | HOLDS (F2, F15) |
| A10 | Full-node calls bounded in time | `tokio::time::timeout` 30 s per request (`Ingestor::poll_stream`), 30 s at startup (`CHAIN_CHECK_TIMEOUT`); the SDK's body-idle watchdog | *bounds_every_full_node_request*; *an_unreachable_node_is_an_error_not_a_hang* | HOLDS |
| A11 | Image SBOM covers what runs | publish workflow (syft image SBOM + one CycloneDX SBOM per binary) | `scripts/check-sbom.sh` + `scripts/tests/check-sbom-test.sh` in CI and the release | HOLDS (F18; 0.0.12 not yet released) |
| A12 | Licence obligations met in the image | `THIRD_PARTY_LICENSES-{api,ingest}.txt` (cargo-about `--fail`), CI `notices` stage | local `cargo about` run; CI | HOLDS (F5) |
| A13 | Least privilege at runtime | Dockerfile; manifest | manifest read 2026-10-09; image config | HOLDS (non-root, read-only root FS, caps dropped, seccomp, no SA token, NetworkPolicy) |
| A14 | The node serves the configured network | `crates/ingest/src/{chain,lib}.rs` (`start`) | chain tests; *start_refuses_a_node_of_another_chain*; live start against testnet | HOLDS (F8, F16; the startup path is now tested) |
| A15 | Operator-keyed lookups cannot resolve unintended entries | `config.rs`, `chain.rs` (exact-match slices) | *treats_network_as_data*, *has_no_inherited_entries* | HOLDS (F20) |
| A16 | RUST — memory safety: `unsafe_code = "deny"` workspace-wide; the one exception justified | `Cargo.toml` `[workspace.lints]`; `persist_wal` (`#[allow(unsafe_code)]`, one `sqlite3_file_control` call with a SAFETY comment) | clippy; read | HOLDS |
| A17 | RUST — panics: none on attacker-reachable paths; release `overflow-checks = true`, `panic = "abort"` | request parsing validates before `parse()` (1–4 / 1–15 digits); indexing only after length checks; poisoned locks recovered with `into_inner` (the API's limiter and pool) | proptests (query parsing, page assembly); handler tests | HOLDS (the `expect`s are on startup, serialisation of owned structs and pre-validated digits) |
| A18 | RUST — crypto-crate usage | none: no signatures, keys or MACs; TLS is rustls (`tls-ring`, webpki roots) inside `sui-rpc`/tonic | — | N/A |
| A19 | RUST — hand-rolled decoding: bounded, property-tested | the URL/query parser (`query.rs`) is the only hand-written parser; gRPC/protobuf is generated (prost, tonic 4 MiB message cap) | 6 proptests in `query.rs`, 1 in `tests.rs` (page assembly) | HOLDS |
| A20 | RUST — HTTP hygiene: bodies, header sizes, concurrency, outbound timeouts, https | bodies never read; 32 KiB request head; 1024 connections; 5 s / 10 s timeouts; gRPC 30 s per request, https enforced by `config.rs` | *an_oversized_request_head_is_refused*, *a_client_that_never_finishes_its_headers_is_cut_off*, config tests | HOLDS (F26) |
| A21 | RUST — shared state: single writer; atomic page + cursor; fail closed | `Store::ingest` transaction; the `api` read-only (F22) | store tests | HOLDS |
| A22 | RUST — rate limiting keyed on a trusted address | `CF-Connecting-IP` (trusted behind the NetworkPolicy, F6), else the peer address; IPv6 by /64 | rate-limit tests | HOLDS |
| A23 | RUST — logging: no secrets (none exist); `Debug` of config holds no credentials | `println!` lines `<ISO> <message>`; config holds ids and URLs only | read | HOLDS |
| A24 | RUST — graceful shutdown: SIGTERM and SIGINT; bounded drain | `ingest`: the loop stops between requests, exit 0; `api`: stop accepting, 5 s grace (`GracefulShutdown`) | *serves_over_http_and_stops_on_shutdown*, *the_loop_polls_every_interval_and_stops_on_shutdown*; smoke test (exit code 0 on SIGTERM) | HOLDS |
| A25 | SC — package-ID semantics: event filters and type prefixes use the **original** ID; one ID per use; IDs traced to the latest on-chain version | `decode.rs` (`stream_filter`, `kind_of`); B.SC-1 | client-contract tests; live head page (event type prefix `0xd7ddaa94…`) | HOLDS (upgrade-added event types are out of reach, F7 decision) |
| A26 | SC — network / chain binding: IDs and RPC client from one `NETWORK`; node chain identifier checked; empty IDs throw | `config.rs`, `chain.rs`, `lib.rs` (`start`) | `chain.test.ts`, config tests; live start | HOLDS (F8, F16; `localnet` unchecked by design) |
| A27 | SC — events: cursor paging, ascending order, pruning expected, partial failure surfaced | `ingestor.rs` (`poll_one`, `is_pruned` recovery), `source.rs` (`PageBuilder`) | ingest tests (pagination, gap, independent streams) | HOLDS (F2, F15) |
| A28 | SC — read parsing against the full normalised type; look-alike types rejected; a missing field fails closed | `kind_of` (exact address/module/struct; generic types rejected) and strict JSON fields at ingest; the client parsers on every served row | *drops_events_of_another_package_module_or_struct*; client audits | HOLDS (F24) |
| A29 | SC — chain-access layering (ADR-0001): no inline BCS or PTB logic | the ingester decodes no BCS (A2); the testkit's BCS writer encodes fixtures only | A2; read | HOLDS |
| A30 | SC — ABI mirroring, funds in the PTB, capabilities, execution result, signatures, dry-run, client-side publish | no PTBs, no signing, no capabilities | — | N/A (read-only indexer) |
| A31 | IMG — base images pinned by digest, minimal runtime stage | `Dockerfile` `FROM` lines | published image | HOLDS (F14, F17) |
| A32 | IMG — build context and reproducible build: `.dockerignore`; `--locked`; pinned tools (cargo-chef 0.1.78, cargo-about 0.9.2); multi-stage | `.dockerignore`, `Dockerfile` | F10; local podman build | HOLDS |
| A33 | IMG — no secrets in layers or args | `Dockerfile` (labels only) | read; published image config | HOLDS |
| A34 | IMG — runtime configuration inventory: `PORT=8080`, `DB_PATH=/data/indexer.db`; deployment sets `NETWORK`, `POLL_MS` (both containers), `PORT` and `RATE_LIMIT_*` (`api`) | image `ENV`; manifest | image config; B.IMG-2 | HOLDS (nothing security-relevant is env-only) |
| A35 | IMG — health and resources: `api` probes on `/readyz`, `ingest` exec probe `--healthcheck`; per-container requests and limits | manifest | B.IMG-2; smoke test | HOLDS (manifest written 2026-10-19, not yet applied) |
| A36 | IMG — scan before sign and verification command pinned to the workflow identity | `rust-ci.yml`, `docker-publish.yml` | F10, B.IMG-1 | HOLDS |
| A37 | IMG — deployment pinned by digest from `config/images.yaml` | overlay; `verify-digests.sh` | B.IMG-2 | HOLDS (the overlay still pins 0.0.11; refresh before applying the two-container manifest — Section D) |
| A38 | The API never writes and never creates the database | `Reader::open` (read-only flag, `query_only`), read-only mount | F22 tests; smoke test | HOLDS |
| A39 | `/healthz` and `polledAt` reflect the ingester's real polls | heartbeat rows (`meta poll:<stream>`) | F23 tests | HOLDS |
| A40 | Stored `kind`/`gate_id`/`commission_mist` equal what the client parsers derive from the BCS | `decode.rs` | parity 19/19 on live testnet events (F24); fixture tests | HOLDS for the kinds seen live; the other kinds rest on the Move field names |
| A41 | Data continuity with 0.0.11: same schema, generation, cursor encoding; no reindex | `crates/store` `SCHEMA`; `PageBuilder`; `mark_rust_writer` | live cursor equality; *backs_up_an_earlier_writers_database_once_before_marking_it*; byte-identical API answers | HOLDS (rollout pending) |
| A42 | Cutover is reversible: a copy before the first Rust write | `Store::mark_rust_writer` (`VACUUM INTO …bak-pre-rust-<ms>`) | store and wiring tests | HOLDS |
| A43 | Request cost independent of history size for every query shape | counters (F1); indexes | load test | **DOES NOT HOLD for rare-kind filters** (F25, pre-existing; OQ5) |

---

## Section B — Supply-chain, publish-authority & capability matrix

### B.1 Dependency & CVE risk

**From 0.0.12 (Rust, 2026-10-19):** `Cargo.lock` committed; `cargo deny check` (RustSec advisories — the
database `cargo audit` reads — licences, bans, sources) clean locally and in CI (`rust-ci.yml`, the
`EmbarkStudios/cargo-deny-action`); duplicate versions only warned (`getrandom` 0.2/0.3, `syn` 2/3). Crates on
the security-relevant paths:

| Crate | Version | Path | Notes |
| --- | --- | --- | --- |
| `sui-rpc` (Mysten) | =0.4.0 | gRPC client: `LedgerService.ListEvents`, `GetServiceInfo` | official sui-rust-sdk; generated protobuf (prost 0.14) and tonic 0.14; Apache-2.0 |
| `tonic` / `prost` / `prost-types` | 0.14.6 / 0.14.4 | transport and decoding of node frames | 4 MiB default message cap |
| `rustls` (via tonic `tls-ring`), `webpki-roots` | per lock | TLS to the full node | no OpenSSL in the image |
| `hyper` / `hyper-util` | =1.12.0 / =0.1.21 | HTTP/1.1 server | header timeout, `max_buf_size` 32 KiB, graceful shutdown |
| `rusqlite` (bundled `libsqlite3-sys`) | =0.40.2 | the database | SQLite compiled in; no system library |
| `tokio` | =1.53.3 | runtime, timeouts, signals | |
| `serde_json`, `base64`, `bs58`, `getrandom` | per lock | JSON, cursors and rows, chain ids, generations | |

Dependabot tracks `cargo` (grouped minor/patch), `docker`, `github-actions`, and the CI-only `tests/contract`
npm project. The table below is the TypeScript era (≤ 0.0.11):

`npm audit --audit-level=high`: 0 (2026-10-09, again 2026-10-18). The CI Trivy image gate (fixable
CRITICAL/HIGH) runs on every build and on the published image before signing; a local Trivy run was not
possible here. The image SBOM does not list the bundled JavaScript; from 0.0.10 a second attested SBOM does
(F18).

| Dependency | Range (installed) | Liveness dependency? | Status | Notes |
| --- | --- | --- | --- | --- |
| `@mysten/sui` | `^2.33.2` (2.35.0) | gRPC, BCS, address normalisation | clean | one copy across the clients; baseline `^2.33.1` (ADR-0001) |
| `@meddleware/access-gate-client` | `^0.0.9` (0.0.9) | event parsing, deployments | clean | pre-v0.2: `^0.0.9` = exact; the latest published (2026-10-18) |
| `@meddleware/seal-client` | `^0.0.20` (0.0.20) | sealed-content parsing, deployments | clean | the latest published; brings `@mysten/seal` 1.4.19 (bundled only if reachable) |
| Base images | builder `node:24-trixie-slim@8ec5d755…`; runtime `distroless/nodejs24-debian13:nonroot@9eeb7f58…` (Node 24.21.0) | runtime | scanned by the CI Trivy gate | digest-pinned; Dependabot (docker, weekly, grouped) proposes bumps |

**TS lens shared-dependency matrix row (this repo):**

| Package | dependency | devDependency | peer |
| --- | --- | --- | --- |
| `@mysten/sui` | `^2.33.2` (baseline `^2.33.1`: same major.minor floor +1 patch; resolves 2.35.0) | — | — (bundled into the image; not a library) |
| `@mysten/walrus`, `@mysten/walrus-wasm`, `@mysten/wallet-standard` | — | — | — |
| `@mysten/seal`, `@mysten/bcs` | transitive via `seal-client` / `@mysten/sui` | — | — |
| `vue` | — | — | — |
| `typescript` | — | `~6.0.3` | — |
| `vitest` | — | `~5.0.2` | — |

First-party ranges are `^0.0.9` and `^0.0.20` (2026-10-18), which resolve to exactly the latest published versions, as
the versioning policy intends. This repo is not published to npm (`private: true`), so TS-M9 (peers) does
not apply; there is one copy of `@mysten/sui` in the bundle (`npm ls`). Dependabot: weekly, grouped
(`npm-minor-patch`, `docker`, `github-actions`); the 2026-10-09 triage declined the Node 25 / TypeScript 7 /
vitest 5 majors per the workspace decisions.

### B.2 Publish authority & CI

| Authority | Where | Custody | Gates |
| --- | --- | --- | --- |
| Push to `quay.io/meddleware-org`, `docker.io/meddleware` | `docker-publish.yml` (tag `v*`) | repo secrets (`QUAY_*`, `DOCKERHUB_*`) | releases; credentials inventory is a maintainer item (`OPERATOR_TASKS.md`) |
| Copy to self-hosted registry (`mirror-private`, by digest) | same | `PRIVATE_REGISTRY_*` | best-effort |
| Signing / attestation | same | OIDC keyless (`id-token: write`, `attestations: write`) | releases |

#### CI & release integrity

| Item | Holds? | Evidence |
| --- | --- | --- |
| Actions pinned to SHAs | Yes | all `uses:` carry a SHA and a version comment |
| Least privilege | Yes | `contents: read` default; write scopes on the public publish job only |
| Tag ↔ version | Yes | `version` job |
| `--locked` builds + `cargo deny` in CI and publish (0.0.12; `npm ci` + audit before) | Yes | `rust-ci.yml`, reused by the release |
| Release runs the full CI on the tagged commit | Yes | `verify: uses: ./.github/workflows/rust-ci.yml` (0.0.12; `node-ci.yml` before) |
| Image scan before sign (published image) | Yes | `docker-publish.yml` "Scan the published image" |
| Signature + SBOM + provenance, no `continue-on-error` on public registries | Yes | B.IMG-1 |
| SBOM covers the compiled code | Yes (0.0.10 npm; 0.0.12 Cargo per binary) | F18 (`cargo cyclonedx` attestations checked by `scripts/check-sbom.sh` in CI and the release) |
| Config scan / container smoke test | Yes | `rust-ci.yml` `image` job; `scripts/smoke.sh` (two containers) |
| Client contract against the shipped binary | Yes (0.0.12) | `rust-ci.yml` `contract` job (Node 26, the published client packages) |
| actionlint | No | not run here (sibling repos run it) |
| Dependabot | Yes | `.github/dependabot.yml` (cargo, npm for `tests/contract`, docker, github-actions; weekly, grouped) |

### B.RS-1 Build & release (from 0.0.12)

- `cargo fmt --check` and `cargo clippy --all-targets --locked -- -D warnings` in CI (`rust-ci.yml` `check`);
  the declared `rust-version` (1.88) is checked by the `msrv` job (`cargo check --locked --workspace
  --all-targets` on 1.88; also run locally 2026-10-19).
- The image: B.IMG-1/2 (digest-pinned bases, nonroot 65532, signature, image SBOM plus per-binary Cargo SBOMs,
  provenance).
- No crates are published (`publish = false`), so crates.io trusted publishing does not apply.
- Release profile: `overflow-checks = true`, `panic = "abort"`, LTO, one codegen unit, stripped.

### B.TS-1 Packaging (≤ 0.0.11)

Not published to npm (`private: true`). The deliverable is the image: one bundle,
`dist/main.mjs` (603 kB), with an ESM `createRequire` banner, plus `package.json`, `package-lock.json`
and `THIRD_PARTY_LICENSES` in `/app`.

### B.TS-2 Install-time code (≤ 0.0.11)

| Script | Package | Reason |
| --- | --- | --- |
| `postinstall` | `esbuild` (dev) | installs or validates the platform binary used by `npm run build` |
| install | `fsevents` (dev, optional) | macOS file watching (vitest); skipped on Linux |

There are no project lifecycle scripts and no `overrides` (`npm run licenses` and `check:licenses` are
explicit scripts, not lifecycle hooks).

### B.TS-3 Supply-chain gates (≤ 0.0.11)

Lockfile committed; `npm ci` everywhere (including the Dockerfile, `--no-audit` there, with the
audit in the workflow); audit at `high`, no allowlist. No npm publish.

### B.IMG-1 Publish & attestation

| Registry | Signature | SBOM attestation | Provenance | Notes |
| --- | --- | --- | --- | --- |
| Docker Hub | cosign keyless (referrer) | SPDX via cosign (referrer; annotation mislabelled — F10; content of the 0.0.9 attestation lacks the bundle; 0.0.10 adds a second, npm-only SBOM attestation — F18, both verified on the released digest 2026-10-18) | BuildKit SLSA (in-index) + GitHub attestation (referrer) | verified 2026-10-09 on `sha256:aea58153…1704` (referrers listed; signature identity not re-run) |
| quay.io | per workflow | per workflow | per workflow | index digest equals Docker Hub's (2026-10-09, `skopeo`); referrers not listed |
| Self-hosted | none | none | BuildKit only | best-effort mirror |

From 0.0.12 each public registry also gets two CycloneDX attestations (`--type cyclonedx`), one per binary,
next to the syft SPDX attestation; the self-hosted mirror receives them as referrers through `oras cp`.

The signer identity (Fulcio certificate SAN = this repo's `docker-publish.yml` at the tag ref) is
checked for every deployed image by `bootstrap/images/verify-digests.sh` (16/16 on 2026-10-09). The
command printed by the release is
`cosign verify <image>@<digest> --certificate-identity-regexp '^https://github.com/meddleware-org/sui-indexer/\.github/workflows/docker-publish\.yml@refs/tags/v' --certificate-oidc-issuer https://token.actions.githubusercontent.com`.
It was not re-run in this pass (cosign not installed).

### B.IMG-2 Cluster manifests (`post-bootstrap/sui-indexer`, read 2026-10-09; rewritten 2026-10-19 for 0.0.12, not yet applied)

| Item | Value |
| --- | --- |
| Workload | 1 replica, `Recreate`, RWO PVC `sui-indexer-data` (1 Gi Longhorn); image by digest from `config/images.yaml` via the overlay. 2026-10-19: two containers from the one image — `ingest` (`/app/ingest`, rw mount, no ports) and `api` (`/app/api`, port 8080, mount `readOnly: true`); `kubectl kustomize` renders both with the overlay digest |
| Pod security | `runAsNonRoot`, uid/gid/fsGroup 65532, `seccompProfile: RuntimeDefault`, `automountServiceAccountToken: false`; each container `allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: true`, `capabilities.drop: [ALL]` (unchanged) |
| Probes | `api`: readiness and liveness on `/readyz` (200 once the database opens); `ingest`: exec liveness `/app/ingest --healthcheck` (60 s initial delay, every 30 s, 3 failures); `/healthz` for monitoring only |
| Resources | 0.0.12: `ingest` requests 10m / 32 Mi, limits 500m / 128 Mi; `api` requests 20m / 32 Mi, limits 500m / 128 Mi (measured RSS 5–26 MiB, README). 0.0.11: requests 20m / 96 Mi, limits 500m / 256 Mi |
| Network | default-deny ingress; allow from the `nginx-ingress` namespace and `platform-probe` on 8080 only; Ingress rate limit 20 r/s burst 80 |
| Runtime env | `NETWORK=testnet`, `DB_PATH=/data/indexer.db`, `POLL_MS=15000` (both containers from 0.0.12), `PORT=8080`, `RATE_LIMIT_PER_SEC=10`, `RATE_LIMIT_BURST=40` (`api`); no `REINDEX_CONFIRM`, no secrets |
| Not present | a snapshot or backup policy for the PVC (F3); an egress NetworkPolicy; `GRPC_URL` is the default public full node |

### B.SC-1 ID-constant trace

| Location | Value | Kind | Source of truth | Latest on-chain? |
| --- | --- | --- | --- | --- |
| `config.rs` `DEFAULT_GRPC` (`config.ts` ≤ 0.0.11) | `https://fullnode.{testnet,mainnet}.sui.io:443` | default gRPC endpoints | Mysten | n/a |
| `config.rs` `DEPLOYMENTS` (testnet; `accessGateDeployment('testnet')` ≤ 0.0.11) | `0xd7ddaa94…88c9` (original-id = published-at) | original ID (event filter and type check) | copied from access-gate-client 0.0.9 `ACCESS_GATE_DEPLOYMENTS` (the same value 0.0.8 carried), from the access-gate-sui publish record | Yes: 11 live events indexed by the Rust ingester (2026-10-19) |
| `config.rs` `DEPLOYMENTS` (testnet; `sealPoliciesDeployment('testnet')` ≤ 0.0.11) | `0x0c8f7349…773d` (original-id = published-at) | original ID (event filter) | copied from seal-client 0.0.20 `SEAL_POLICIES_DEPLOYMENTS` | Yes: matches the client's record; still 0 sealed-content events (2026-10-19) |

From 0.0.12 the defaults are a copy in `config.rs`, not an import of the client packages (no JS at runtime):
a new client deployment record must be copied there (CLAUDE.md, README). The client-contract job pins the
API against the same client versions.

The superseded `0xa55789…` and `0x61c4aa…` packages are immutable and are not indexed any more.

### B.SC-2 Coupling table

N/A: the indexer builds no PTBs and calls no Move functions. Event decoding is delegated to the clients,
whose parsers match the full type string; the coupling test is the *client contract* suite
(`listAccessGateEvents` and `listSealedContent` through the handler).

### B.SC-3 Cross-implementation parity

Single implementation in service. The 2026-10-19 cutover was checked against the replaced TypeScript one:

| Behaviour | 0.0.11 (TS) | 0.0.12 (Rust) | Evidence |
| --- | --- | --- | --- |
| Stored `kind`, `gate_id`, `commission_mist` | client parsers on the BCS | type name + node JSON | 19/19 identical on live testnet events (F24) |
| Stored cursor for a page | `core.listEvents` `endCursor` | `PageBuilder` | same value (`KAFCDginh429ARDNzazFDxgA`) for the same page |
| API responses | `api.ts` | `handler.rs` | byte-identical for 10 routes and 6 error/redirect cases against the live instance |
| Client reading | through the handler (vitest) | through the binary over HTTP (`contract` job) | 5 contract tests with the published clients |

The clients and the indexer share `readIndexerEvents` semantics (https only, `redirect: 'error'`, 1 MiB cap,
page shape) in the clients; the contract job checks a canonical query is served without a redirect, a
non-canonical one is a `301` the client refuses, and a 100-event page stays under the cap.

---

## Section C — Test-coverage & hermetic/live split

### C.1 Coverage grade — A (0.0.12: 107 Rust tests, 90.7% lines; the startup wiring is tested)

**2026-10-19 (Rust):** the 0.0.11 cases are ported one for one (api 19 → `crates/api/src/tests.rs`, ingest
22 → `crates/{store,ingest}`, config 5 and chain 3 → `config.rs`/`chain.rs`, sbom 3 → the 10-case
`scripts/tests/check-sbom-test.sh`) and extended. **The `main` wiring gap is closed:** the startup order, the
loop and the healthcheck live in `crates/ingest/src/lib.rs` (`start`, `run_loop`, `healthcheck`) and the API's
in `server.rs`/`lib.rs` (`serve`, `context`), all tested (chain refusal, package refusal and confirmed reindex
with backups, the cutover backup, the poll interval and shutdown, the healthcheck threshold, serving over a
socket, header timeout, oversized head, graceful stop). What stays untested in-process is each `main.rs`
(environment, runtime, signal handler; 0% lines), which the container smoke test runs.

| Dimension | Assessment (0.0.12) |
| --- | --- |
| Happy path | as below, plus the real `sui-rpc` client against a local LedgerService stub, and the client contract against the binary |
| Error path | as below, plus unreachable node, chain mismatch, unreadable chain id, package change refused at start, undecodable JSON renderings (ten mutations), `503` before the database exists |
| Boundary | as below, plus page assembly from arbitrary frames (proptest), URL/query canonicalisation (6 proptests), commission sums beyond 64 bits, the 32 KiB request head |
| Security-relevant | the read-only database handle (a write is refused), the API never creating the database, the persisted WAL files for a read-only mount |

Below: the 0.0.10 assessment.

| Dimension | Assessment |
| --- | --- |
| Happy path | Ingest pages and resume; re-ingest no-op; null cursor; page budget; decode round trip; package binding (fresh, unchanged, changed, legacy rows); pages, filters, cursors, stats, coverage, health; client contract for both clients |
| Error path | Generation mismatch `410`; strict validation; timeout aborts a hung call; `405`; `429`; preflight; an undecodable event is counted and skipped; one failing stream does not stop the other; a pruned cursor restarts and counts a gap; other failures are not swallowed as a gap; refusal to clear history without confirmation |
| Boundary | Limits, cursor format, commission > 2^53, non-canonical spellings (redirect, no loops), weak / listed / wildcard `If-None-Match`, IPv6 /64 keys and the bucket cap, counters over 50k rows |
| Security-relevant | Validation, display-only, canonical URLs and the rate limiter are covered. **Not exercised in tests:** the process wiring (`main.ts`, including the startup chain-identity check, F16) and the live node. The container smoke test covers the image on `localnet` (no chain) |

Coverage: the first-pass figures (82.0% statements / 85.6% branches at 0.0.4, 30 tests) are superseded by
17 further tests; the coverage plugin is not installed, so the figure was not re-measured. The grade rises
from B because the first-pass error-path and boundary gaps (F1, F2, F6) are closed with tests.

**Test layers (0.0.12):**

| Layer | Files | In CI? |
| --- | --- | --- |
| Pure unit tests | `crates/store/src/{config,sui,time}.rs`, `crates/ingest/src/chain.rs`, `crates/api/src/{query,ratelimit}.rs` (incl. 6 proptests) | yes (`rust-ci.yml` `check`, reused by the release) |
| Store / ingest / handler tests (SQLite files, scripted full node) | `crates/{store,ingest,api}/src/tests.rs` (incl. 1 proptest) | yes |
| Local-socket integration | gRPC stub (`tonic` server) for `GrpcSource`; hyper server tests | yes |
| Client contract (published TS clients against the built `api` binary) | `tests/contract/contract.test.ts` (5) | yes (`contract` job) |
| SBOM gate | `scripts/tests/check-sbom-test.sh` (10 cases) and the real SBOMs | yes (`check` job and the release) |
| Image smoke test (two containers, read-only API mount) | `scripts/smoke.sh` | yes (`image` job) |
| Config and image scans | Trivy config scan, Trivy image scan | yes (CI, and on the published image before signing) |
| Local run against testnet | README → Development (manual; done 2026-10-19 with the parity check) | no |

Every test project that exists runs in CI. Up to 0.0.11: one vitest project (52 tests at 0.0.10).

### C.2 Hermetic vs. live paths

| Path | Hermetic? | Deferred to | Tracking |
| --- | --- | --- | --- |
| gRPC `ListEvents` semantics (ordering, cursors, timeout) | 0.0.12: scripted source, frame proptest, local LedgerService stub with the real client; cursor equality with `@mysten/sui` checked live once | manual testnet run before a release | F9, F12 |
| Startup chain-identity check and package binding | 0.0.12: yes (`start` tests with a scripted source) | live start of each release | F16 |
| JSON renderings of kinds not yet seen on testnet | no (fixtures written from the Move sources) | the first such events: re-run the parity check | F24 |
| Container runtime (nonroot, volume, probes) | smoke test covers nonroot, `/data`, `/readyz`, headers; probes and security context are read from the manifest | cluster | F10, B.IMG-2 |
| Cloudflare cache and rate rule | no | zone rate-limit rule not set (free plan, maintainer item in the infrastructure README) | F6 |
| Ingest on real events of the new packages | partly (live: 11 `access-gate` events on 2026-10-19, 0 `sealed-content`) | first sealed-content publication on `0x0c8f7349…` | B.SC-1 |

---

## Section D — Deployment-readiness gates

### pre-localnet

- [x] builds; type-check, lint and 47 tests green; no secrets in source or args — 2026-10-09 run (Scope,
  Environment); F14
- [x] every `FROM` pinned by digest; lockfile installs; no secrets in args — F14
- [x] strict API; atomic ingest; generations — F12, F13
- [x] `.dockerignore` covers `.env*` and `*.db*` — F10 (`ef68727`, released 0.0.7)
- [x] strict type-check green; no swallowed promises on security paths; untrusted parsers validate every
  field — A16–A20 (`noUncheckedIndexedAccess` on since 0.0.10: F21)
- [x] clippy -D warnings, fmt, cargo audit + cargo deny green; forbid(unsafe_code) or justified — 0.0.12,
  2026-10-19 (Environment table; `unsafe_code = "deny"` with one justified FFI call, A16)
- [x] no panics on attacker-reachable paths — A17 (0.0.12)

### pre-testnet *(live on testnet since 2026-10-03; all items now met)*

- [x] per-request cost independent of history size — F1 (0.0.5, `1973fca`)
- [x] ingest survives an undecodable event; streams independent; staleness visible on `/healthz` and the
  status page — F2 (0.0.5; status check 2026-10-09). The apps do not yet read `polledAt`: F19, a
  pre-mainnet row below
- [x] no irreversible clear without confirmation; backup before a clear — F3 (0.0.5). Volume backups are
  not set: accepted for testnet (display-only; the apps fall back to the node for its recent window), and a
  maintainer item recorded in F3
- [x] canonical URLs; IPv6-aware limiter; NetworkPolicy admits only `nginx-ingress` and `platform-probe` —
  F6
- [x] pod security context, probes, limits and digest pinning confirmed in the infrastructure workspace —
  B.IMG-2 (manifest read 2026-10-09)
- [x] integration against testnet: the live service indexes the 2026-10-09 packages; consumed IDs match the
  client records — B.SC-1; Deployment status
- [x] `SECURITY.md` present — Scope
- [x] lockfile committed; signature, SBOM and provenance on the image; CI gates green — B.IMG-1, B.2
  (the SBOM content gap was F18, closed in 0.0.10)
- [x] third-party licence notices in the image — F5
- [x] timeouts on every outbound call; response bodies bounded — A10, A19, A20 (0.0.12: 30 s per gRPC
  request, tonic's 4 MiB message cap, one 51-item page per request)
- [x] low-S and ed25519 rules match the Sui client lens; parity vectors green — N/A: the indexer verifies no
  signatures (A18); the TS↔Rust parity check passed 19/19 (F24, B.SC-3)
- [x] rate limit keyed on a trusted address; SIGTERM handled — A22, A24
- [ ] 0.0.12 released and rolled out: tag `v0.0.12` (CI green, signature and three SBOM attestations
  verified), `config/images.yaml` at 0.0.12 with the consensus digest and `sync-manifests.sh`, then the
  two-container deployment; after rollout the same events count and generation, a pre-cutover cursor still
  pages, `/healthz` 200 with both streams fresh, the `.bak-pre-rust-*` copy logged, treasury-ui and seal-ui
  list history, the status page's "Chain index" is green — maintainer item (release and cluster), D58

### pre-mainnet

- [x] SBOM includes the bundled dependencies (IMG-M8) — F18 (0.0.10, commit `4aab344`): second SPDX
  attestation from `npm sbom --omit dev`, checked by `scripts/check-sbom.mjs` and `tests/sbom.test.ts`;
  confirmed on the released 0.0.10 digest, 2026-10-18: two SPDX attestations (image SBOM, 17 packages; npm packages SBOM, 24 packages) verified with `cosign verify-attestation --type spdxjson`
- [ ] clients treat a stale head page as unavailable — F19; coordinated client change, gated on mainnet
- [ ] a mainnet deployment recorded in both clients (startup otherwise throws by design); then
  `NETWORK=mainnet` with the recorded chain id `35834a8a` — mainnet-blocked (access-gate-sui and
  seal-policies-sui publication, `OPERATOR_TASKS.md` "Mainnet release custody")
- [x] `noUncheckedIndexedAccess` enabled (F21) and the `Object.hasOwn` hygiene (F20) — 0.0.10, commit `4aab344`: flag on, `tsc` clean; `Map` lookups with tests
- [ ] volume snapshot or backup policy for `sui-indexer-data` (F3) — maintainer item; no `OPERATOR_TASKS.md`
  row yet
- [ ] Cloudflare zone rate-limit rule (F6) — maintainer item (free plan)
- [ ] external review — maintainer item
- [x] chain-identity check on `GRPC_URL` — F8, F16 (0.0.6)
- [x] image scan gate, scan of the published image before signing, image and manifest scans clean at the
  review date — F10, F17 (CI gate passed for 0.0.9; not re-run locally)
- [x] `npm audit --audit-level=high` clean — 0 vulnerabilities, 2026-10-09 (0.0.11; from 0.0.12 `cargo deny`
  advisories clean, 2026-10-19)
- [x] decoders fuzzed / property-tested; maximum message size enforced — A19 (7 proptests; prost/tonic
  4 MiB cap; 32 KiB request head)
- [x] concurrency limit sized against memory for the configured body cap — F26 (1024 × 32 KiB within the
  128 MiB limit; bodies are never read)
- [x] image per the IMG lens; crates.io trusted publishing — IMG rows A31–A37; no crates are published (N/A)
- [ ] rare-kind `kinds` filters bounded (an `events_kind (stream, kind, id)` index, F25, OQ5) — next patch
  after the 0.0.12 cutover
- [ ] the JSON-vs-BCS parity check re-run once sealed-content and gate-lifecycle events exist on the network
  (F24) — gated on those events

---

## Cross-project themes

- **The display-only contract needs a freshness half.**
  - Apps fall back on *errors*, but a stuck indexer (F2) or a slow one (F1) answers `200`.
  - The indexer half is done: head pages carry `polledAt` and `latestCheckpoint`, `/healthz` is per stream
    and the status page checks it (F2). The client half is open: access-gate-client
    (`listAccessGateEvents` indexer source) and seal-client (`listSealedContent`) do not read it (F19,
    pre-mainnet).
- **Pruning makes the indexer the system of record for old display history.** That is the reason
  F3 matters, and the measured testnet retention (about 5½ days) makes it stronger. The same applies to
  dao-ui and treasury-ui's AdminCap-based gate discovery, which exists because events are pruned.
- **Bundled images hide their dependencies.** F4 and F5 apply to every esbuild- or Vite-bundled
  image in the workspace (the static-server UIs ship bundles too). The licence-file half is solved here
  (`scripts/third-party-licenses.mjs`, F5), as in the other web images. The SBOM-from-lockfile half is not
  (F18) was open at 0.0.9; here it is fixed in 0.0.10 by a second SBOM attestation from the lockfile
  (`npm sbom --omit dev` + `scripts/check-sbom.mjs`), a pattern the sibling bundled images can copy.
- **Chain-access layering.** The indexer is a consumer of the domain clients (ADR-0001): it decodes
  nothing itself and holds no package IDs of its own; a republication reaches it through the clients'
  deployment records and a confirmed rebind (F3, F7; exercised on 2026-10-09).
- **Seal discovery is unauthenticated end to end.** seal-client F6 + F8 here: pointers come from the
  full node or the indexer, and neither is authenticated against an independent source.
- **Pre-v0.2 policy:** F1–F3 and F6 changed the API (added fields, redirects) in 0.0.5 as planned; the
  versioning is patch-only until go-live (0.0.12 for the Rust rewrite, a breaking internal change).
- **No JS/TS backends (D58, 2026-10-19).** The indexer is the first backend moved; the Rust patterns
  (cargo-chef + distroless `cc`, cargo-deny, cargo-about notices, Cargo SBOM gate in bash + jq, a CI contract
  job driving the published TS clients against the binary) come from gateway-rust and seal-key-server and are
  reusable for the next one (nft-gate's Workers gateway, plan Part E).

---

## Normative requirements (MUST / MUST NOT)

1. MUST keep per-request cost independent of the number of indexed events — **holds** (F1).
2. MUST keep ingesting both streams when one event cannot be decoded, and MUST expose staleness —
   **holds** at the indexer (F2, F15); the clients do not yet act on it (F19).
3. MUST NOT irreversibly delete indexed history on a configuration change without explicit
   confirmation or an archive — **holds** (F3: confirmation plus a backup on the same volume; volume loss
   is outside it).
4. MUST attach an SBOM that covers the bundled runtime code — **holds from 0.0.10** (F18); MUST ship its
   licence notices — **holds** (F4, F5).
5. MUST map equivalent requests to one cache key — **holds** (F6).

6. MUST keep the API process unable to write the index (0.0.12) — **holds** (F22).
7. MUST keep the stored columns equal to what the client parsers derive from the BCS — **holds** on every
   live event (F24); re-check when new kinds appear.

**RUST lens baseline (from 0.0.12):**

| ID | Holds? | Evidence |
| --- | --- | --- |
| RS-M1 | yes (`unsafe_code` denied; one justified FFI call; no panics on request paths) | A16, A17 |
| RS-M2 | N/A (no signatures) | A18 |
| RS-M3 | yes (the only hand-written parser — URL/query — is property-tested; protobuf is generated with tonic's message cap) | A19 |
| RS-M4 | yes (bodies never read; 32 KiB head; 1024 connections) | F26, A20 |
| RS-M5 | yes (30 s per gRPC request; response sizes bounded by the page size and tonic's cap) | A10, A20 |
| RS-M6 | yes (one writer, atomic page + cursor; the API read-only) | A21, F22 |
| RS-M7 | yes | A22 |
| RS-M8 | yes (no secrets; SIGTERM handled in both processes) | A23, A24 |
| RS-M9 | N/A in service (single implementation); the replaced TS implementation was compared (B.SC-3) | B.SC-3 |

**TS lens baseline (≤ 0.0.11):**

| ID | Holds? | Evidence |
| --- | --- | --- |
| TS-M1 | yes (`strict`; casts only on self-written rows and one network-union cast; `noUncheckedIndexedAccess` on since 0.0.10) | F9, F21 |
| TS-M2 | yes (strict request validation; client parsers for node data; canonical URLs) | F13, F6 |
| TS-M3 | yes (`commissionMist` as `bigint`) | F13 |
| TS-M4 | yes (poll errors recorded per stream; one stream's failure no longer skips the other) | F2 |
| TS-M5 | yes (30-second abortable gRPC timeout; URL parsed and https checked) | F12 |
| TS-M6 | yes (no secrets; generic 500; no dynamic code) | F13 |
| TS-M7 | N/A (not published to npm); image contents — F4, F5 | |
| TS-M8 | yes | B.TS-3 |
| TS-M9 | N/A (not a library; one copy of `@mysten/sui`) | B.1 |

**SUI_CLIENT lens baseline:**

| ID | Holds? | Evidence |
| --- | --- | --- |
| SC-M1 | yes (original IDs for event types) | F7 decision |
| SC-M2 | yes (client parsers match the full normalised type) | A2, A28 |
| SC-M3 | N/A (no transactions) | — |
| SC-M4 | yes (`bigint`; normalised addresses and IDs) | A19 |
| SC-M5 | yes (IDs and endpoint from one `NETWORK`; the node's chain identifier is checked at start) | F8, F16 |
| SC-M6 / SC-M7 / SC-M8 / SC-M9 | N/A (no signatures, no authorising events, no PTBs, no created objects) | — |
| SC-M10 | yes (parsing delegated to the domain clients) | A2, A29 |

**IMG lens baseline:**

| ID | Holds? | Evidence |
| --- | --- | --- |
| IMG-M1 | yes | F14, F17 |
| IMG-M2 | yes (`.dockerignore` covers `.env*`, `*.db`, `*.db-*`, `*.bak-*`) | F10 |
| IMG-M3 | yes | F14 |
| IMG-M4 | yes | front matter |
| IMG-M5 | yes (image 65532; pod context read from the manifest 2026-10-09) | B.IMG-2 |
| IMG-M6 | yes (digest from `config/images.yaml`; 16/16 images cosign-verified 2026-10-09 and again 2026-10-18) | B.IMG-2, B.IMG-1 |
| IMG-M7 | yes (signature, SBOM attestation and provenance present on Docker Hub; the private mirror is best-effort) | F10, B.IMG-1 |
| IMG-M8 | yes from 0.0.10 (scan before sign, licence notices, identity-pinned verification; the bundle is listed in a second attested SBOM; confirmed on the published 0.0.10 digest, 2026-10-18). 0.0.12: per-binary Cargo SBOMs, checked; to be confirmed on the published digest | F10, F5, F18 |

## Implementation suggestions (SHOULD / MAY)

- **S1** SHOULD keep per-stream stats (`fromCheckpoint`, `latestCheckpoint`, counts, per-gate
  counters, commission) in tables updated by `ingest()` (F1) — done in 0.0.5.
- **S2** SHOULD add `polledAt` and `latestCheckpoint` to every head page — done in 0.0.5. The clients
  SHOULD fall back when it is older than N poll intervals (F19, open).
- **S3** SHOULD make the undecodable path explicit and visible in `/coverage` (F2) — done (count and
  skip, OQ2).
- **S4** MAY add an `/v1/{network}/export` (or an offline `VACUUM INTO` CronJob) so pruned history
  can be restored into a fresh volume (F3). Not built; the volume backup is a maintainer item.
- **S5** MAY sample-verify new rows against a second full node, and expose a `verified` ratio in
  `/coverage` (F8). Not built; display-only by design.
- **S6** SHOULD generate the SPDX SBOM from the source tree and attest that (F18) — done in 0.0.10.
- **S7** SHOULD enable `noUncheckedIndexedAccess` and use `Object.hasOwn` / `Map` for operator-keyed lookups
  (F20, F21) — done in 0.0.10.
- **S8** SHOULD add `events_kind (stream, kind, id)` with a page-cost test for a rare kind (F25, OQ5).
- **S9** MAY run the client-contract job against the image too (today it runs against the binary built on
  the runner; the smoke test covers the image's processes).

## Open questions (`OQ#`)

The four first-pass questions are decided; OQ5 (2026-10-19) is open.

1. **OQ1** F3 — decided: a fresh publish clears the old package's rows after a `VACUUM INTO` backup;
   history is not kept queryable per package; deletion requires `REINDEX_CONFIRM` naming the stream.
2. **OQ2** F2 — decided: an undecodable event is counted and skipped (neither stored nor quarantined).
3. **OQ3** F7 — decided: this project republishes before v0.2 instead of adding event types by upgrade;
   revisit if mainnet packages are ever upgraded to add events.
4. **OQ4** — answered 2026-10-09: `/healthz` is wired to the status page ("Chain Access → Chain index",
   `OPERATOR_TASKS.md`); there is **no** volume snapshot or backup policy for `sui-indexer-data` (F3),
   and no `OPERATOR_TASKS.md` row for it yet — a maintainer item.
5. **OQ5** F25 — when to add `events_kind (stream, kind, id)`: (a) in the 0.0.12 cutover (one rollout, but
   the cutover would no longer be schema-identical); (b) in the next patch after the cutover has settled
   (recommended: the cutover stays a pure language change and the index is additive and rollback-safe);
   (c) not at all, relying on the edge cache and the limiter (the cost grows with history).

## Risks

- **Silent staleness:** the indexer side is now visible (`/healthz`, the status page) but the apps still
  show frozen history until the indexer recovers (F2, F19).
- **Irreversible history loss:** volume loss with the node's retention at about 5½ days; the backup sits on
  the same volume (F3).
- **Growth:** fixed in code (counters, canonical URLs); the Cloudflare zone rate rule is not set (F1,
  F6).
- **Supply-chain blind spot:** closed in 0.0.10: the bundled dependencies are in an attested SBOM (F18);
  `npm audit`, Dependabot and the Trivy gate cover the lockfile and the base image (F4, F17).
- **Upstream node trust:** a compromised full node can inject display rows (F8, display-only); from 0.0.12 it
  also renders the JSON the filter columns come from (F24).
- **Cutover (0.0.12):** the manifests must not be applied before the 0.0.12 digest is pinned; the
  `.bak-pre-rust-*` copy and the 0.0.11 digest are the rollback (workspace README). Rare-kind filters walk the
  stream (F25) as they did before.

---

## Re-verification log

- 2026-10-19 — Rust rewrite (0.0.12; local commits `1fcbbe9`, `76595ca`, `784250a`, `ddfc77d` and the docs
  commit, on `fb79cd7`; not released, not deployed).
  - **Lenses:** AUDIT_TEMPLATE.md + RUST + SUI_CLIENT + IMG at 2026-10-08; TS no longer applies to the service
    (only to the CI-only `tests/contract`). Section A's TS rows are replaced by RUST rows; A38–A43 added.
  - **Re-verified:** F1–F21 against the Rust code (a note on each). F11 → RESOLVED (the infrastructure
    retention wording fixed in the workspace manifests). F16's residual (startup path untested) closed. No
    disposition regressed.
  - **New:** F22 (read-only API process, Positive), F23 (health and liveness across two processes, RESOLVED),
    F24 (columns from the node's JSON, ADJUDICATED with the parity gate), F25 (rare-kind filters walk the
    stream, pre-existing, DEFERRED, OQ5), F26 (connection cap, RESOLVED in `ddfc77d`).
  - **C.1 resolved:** the startup wiring is tested (Section C).
  - **Measured:** 107/107 Rust tests, 90.7% lines; clippy/fmt clean; MSRV 1.88 checks; `cargo deny` clean;
    cargo-about notices generate; Cargo SBOMs pass the gate (63/63, 139/139); 5/5 client-contract tests
    against the binary; podman image 38.3 MB and the two-container smoke test passes; parity 19/19; API
    byte-identical to the live 0.0.11 for 16 checked requests; load figures in Environment.
  - **Totals F1–F26:** 15 RESOLVED, 2 MITIGATED, 3 ADJUDICATED, 0 ACCEPTED-RISK, 2 DEFERRED (F19, F25),
    4 Positive.
  - **Not run:** the GitHub workflows (publish, Trivy, cosign, actionlint), the cluster rollout (a Section D
    maintainer row), a live-DB copy diff (no cluster access in this pass; every live row is among the 19
    parity events, since the live index holds the same 11 events the node serves).
- 2026-10-18 — code gates closed in `0.0.10` (commit `4aab344`), on top of `d1910e0`.
  - **Fixed:** F18 (and F4): the release attests a second SPDX SBOM from `npm sbom --omit dev`, checked by
    `scripts/check-sbom.mjs` (CI `check` job and the release). F20: `Map` lookups for `NETWORK`. F21:
    `noUncheckedIndexedAccess` on, two `src/` and five test sites fixed. F11 residuals 1, 2 and the
    CHANGELOG part of 3.
  - **Dependencies:** `@meddleware/access-gate-client` ^0.0.9 and `@meddleware/seal-client` ^0.0.20 (the
    latest published); the bundle builds (603 kB) and the client-contract tests pass against them.
  - **Measured (local, Node 24.13.0):** 52/52 tests (api 19, ingest 22, config 5, chain 3, sbom 3); tsc
    (with the flag), eslint, `npm audit --audit-level=high` (0), `npm run build`, `check:licenses` and the
    SBOM check (23 production packages) all pass. `list-open-gates` for this repo: only mainnet and
    maintainer items remain, plus F19.
  - **Left open:** F19 (the change is in access-gate-client and seal-client; see the note on F19), F3 volume
    backup and the Cloudflare zone rule (maintainer items), the mainnet deployment record, external review.
  - **Not run:** the publish workflow, `cosign`, Trivy, `actionlint`, the container smoke test (no daemon
    access here; GitHub Actions runs them).
- 2026-10-09 — re-verification against `a3369bb` (tag `v0.0.9`; image `sha256:aea58153…1704`).
  - **Lenses:** AUDIT_TEMPLATE.md + TS + SUI_CLIENT + IMG at 2026-10-08. OPS, PLATFORM, PROXY, AUTH and the
    rest re-checked against the registry and still not triggered (no signing or chain-writing scripts; no
    forwarding; no credentials). Section A gained the TS, SUI_CLIENT and IMG category rows (A16–A37).
  - **Measured:** 47/47 tests (api 19, ingest 22, config 4, chain 2); tsc, eslint, `npm audit` (0) clean;
    bundle 603 kB; `check:licenses` passes (24 packages). `tsc --noEmit --noUncheckedIndexedAccess`: 2 errors
    in `src/`, 5 in tests (F21). Source is 1,136 lines in seven modules.
  - **Live (curl):** `/healthz` `ok: true` for both streams; `/v1/testnet/coverage` generation
    `e32e37afbbe9`, access-gate 3 events from checkpoint 392921227, 0 undecodable, 0 gaps, sealed-content 0
    events.
  - **Dispositions:** F1, F5, F6, F9, F10 RESOLVED; F2, F3, F4, F11 MITIGATED; F7, F8, F20 ADJUDICATED; F12–F14
    Positive (re-verified). Fixed after the first pass and added: F15 (pruned-cursor wedge, 0.0.5), F16
    (chain-identity startup bug, 0.0.6), F17 (libssl advisories, 0.0.8) — all RESOLVED. New and not fixed: F18
    (SBOM omits the bundle) and F19 (clients ignore `polledAt`), both DEFERRED to the pre-mainnet gate; F20
    (prototype-keyed lookups, ADJUDICATED); F21 (`noUncheckedIndexedAccess` off, ACCEPTED-RISK).
  - **Releases:** 0.0.5 (`1973fca`, fixes), 0.0.6 (`8573d0b`), 0.0.7 (`7fcd5cf` with `ef68727` CI gate), 0.0.8
    (`70fa4df`), 0.0.9 (`a3369bb`).
  - **Stale facts corrected:** package IDs (`access_gate` `0xd7ddaa94…88c9`, `seal_policies` `0x0c8f7349…773d`;
    the `0xa55789…` / `0x61c4aa…` packages are superseded and immutable), client versions (access-gate-client
    0.0.8, seal-client 0.0.19), image digest and tag, Node and distroless bases, test count, line count,
    testnet retention (5½ days, not 3 months).
  - **Decisions recorded:** OQ1–OQ4 (Open questions); new event types need a fresh publish (F7).
  - **Not checked:** the Actions run history; `cosign verify` of the signer identity in this session (done for
    all deployed images by `verify-digests.sh`, 16/16); Trivy or Syft on the image; the running pod; coverage.
  - **Open with a gate:** F18, F19 (pre-mainnet rows in Section D); volume backup (F3) and the Cloudflare
    zone rule (F6) are maintainer items. Nothing is open without a named gate.
- 2026-10-03 — first-pass baseline at `9aa3370` (tag `v0.0.4`; Docker Hub 0.0.4
  `sha256:47d928a5…0717`).
  - **Lenses:** AUDIT_TEMPLATE.md (2026-10-02) + TS (2026-10-03) + SUI_CLIENT (2026-09-30) + IMG
    (2026-09-30).
  - **Measured:** 30/30 tests; coverage 82.0 / 85.6 / 75 / 83.2; tsc, eslint and audit (0) clean;
    bundle 590 kB with no licence comments.
  - **Probes:** confirmed F2 and F6 (deleted afterwards). A benchmark confirmed F1 (deleted
    afterwards).
  - **SDK source:** confirmed the timeout and ordering semantics.
  - **Registry API:** confirmed the signature, provenance and SBOM present, and the SBOM's contents
    (F4).
  - **Not checked:** quay.io, the cosign signer identity, the running container, the infrastructure
    manifests, the Actions history.
  - **Recorded:** F1–F14; OQ1–OQ4.
  - **No findings resolved:** by maintainer instruction this pass only records findings. Remediation,
    including single-solution fixes under the resolve-inline rule, is to be applied separately, with
    each disposition moved to RESOLVED and the diff cited.
- 2026-10-18 — released and deployed (`v0.0.10` = `5251f38`, `0.0.10@sha256:b3484ce42a8484383efd598a3f21d3cb7077c7e3d4b73eb0df1c740b221d8a58`), verified live. Cosign keyless signature verified against the workflow-anchored identity (`verify-digests.sh` 16/16); the digest carries two SPDX attestations, the image SBOM (17 packages) and the npm packages SBOM (24 packages), both verified with `cosign verify-attestation --type spdxjson` on 2026-10-18 (F18 post-release check closed); rollout succeeded; `/healthz` ok on the same database generation `e32e37afbbe9` (re-read 2026-10-18). F19 (client change), F3 volume backup, the Cloudflare zone rule and the external review remain open.
- 2026-10-19 — dependency-bot status checked (read-only): `.github/dependabot.yml` carries the `ignore` rules and the repository has no open Dependabot pull requests (`gh pr list`, 2026-10-19). Versions, findings and gates were not re-examined in this pass.
  - Dependabot policy encoded in .github/dependabot.yml (majors declined); open PRs merged 2026-10-19
  - `docker-publish.yml`: the private rebuild jobs are replaced by one `mirror-private` job that copies the published quay.io image (and its cosign artifacts) to the self-hosted registry by digest and fails unless the digest and the signature match (build once, promote by digest; committed locally, not yet released; the `PRIVATE_REGISTRY_*` credentials are still unset).

## Pre-save consistency checklist (this pass)

- [x] Section A ↔ findings: A16–A24 are RUST rows (2026-10-19); A38–A42 HOLD (F22–F24, cutover); A43 does not
  hold (F25, DEFERRED with OQ5); A6 caveat (F3, backup on the same volume); A1 caveat (F19).
- [x] Finding header ↔ body: consistent (F1–F26; F11 header moved to RESOLVED with its note).
- [x] Template line: base + RUST + SUI_CLIENT + IMG with registry dates; untriggered lenses named, TS's
  remaining scope (CI-only contract project) stated.
- [x] Closing four-part structure present (normative, suggestions, open questions, risks).
- [x] Section D ↔ dispositions: every unticked item names a gate or a maintainer item (the 0.0.12 rollout,
  F25/OQ5, the F24 re-run, F19, mainnet, volume backup, edge rule, external review).
- [x] Executive summary ↔ dispositions and ceiling (Medium; realised Low); totals F1–F26 checked
  (2026-10-19: 15 / 2 / 3 / 0 / 2 / 4).
- [x] C.1 counts measured 2026-10-19 (107 Rust tests, 5 contract, 10 SBOM-gate cases).
- [x] Re-verification log entry added (2026-10-09, 2026-10-18, 2026-10-19).
