# Security Audit — `sui-indexer`

**Classification:** Internal security review (re-verified 2026-10-09; code gates closed 2026-10-18; re-verified 2026-10-19 against the Rust rewrite 0.0.12; F25 closed in 0.0.13; re-verified 2026-10-20 against the PostgreSQL storage 0.0.14; SQLite importer removed in 0.0.15, 2026-10-20 — awaiting external review)
**Project:** sui-indexer — a lean read-indexer for display data.

- It polls two streams from a Sui full node over gRPC into PostgreSQL (the platform's shared database tier; SQLite up to 0.0.13):
  - the `access_gate` module events;
  - `sealed_content::SealedContentPublished`.
- It serves cacheable GET pages behind Cloudflare.
- Apps (treasury-ui activity, seal-ui discovery) read history from it when `VITE_INDEXER_URL` is set,
  and fall back to the full node otherwise.

**Project type:** Rust service (from 0.0.12: a Cargo workspace with two binaries, `ingest` — the sole
writer — and `api` — read-only, `hyper` — in one image; 0.0.12-0.0.13 ran them as two containers of one pod over a
SQLite file, **0.0.14 runs them as two Deployments over the shared PostgreSQL tier, each with its own database
role**) + container image. Up to 0.0.11 it was a TS service (Node `node:http` + `node:sqlite`, one esbuild bundle);
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

- **2026-10-20: `0.0.15` (the one-off SQLite importer and `rusqlite` removed; no behaviour change for the API; F31) committed
  locally, not released.** Per the maintainer the cutover to PostgreSQL is done and settled: the legacy SQLite Deployment, the import Job,
  the old PVC, the legacy NetworkPolicies and the server backup of the old volume are deleted from the cluster (not re-observed by this review;
  `config/images.yaml` pins the 0.0.14 digest `sha256:ed5f66b2…0566`). In the workspace `post-bootstrap/sui-indexer/migration/` is deleted and
  `apply.sh` keeps only the first-apply check for the database Secrets and the CA. The overlay is not changed by this work: the maintainer tags
  `v0.0.15` and pins the digest.
- **2026-10-20 (earlier, historical): `0.0.14` (storage moved to the shared PostgreSQL tier, F27–F32, OQ7) committed locally, not released
  or deployed at that time.** Commits on top of `c0ab2fa`. `config/images.yaml` and the overlay pin 0.0.13
  (`sha256:82f631e6…8825`, the SQLite image; the maintainer's current image) and are not changed by this work. The
  workspace manifests are rewritten (`post-bootstrap/sui-indexer`: two Deployments, per-pod NetworkPolicies, no PVC;
  `migration/`: the one-off import Job, the cutover runbook and the legacy manifests for rollback) and **must not be
  applied before the 0.0.14 digest is pinned**: `apply.sh` and `migration/import.sh` refuse the 0.0.13 digest. The
  database side (`sui_indexer` database, roles, Secrets, NetworkPolicy) is authored in the workspace (D59/D60) and
  not applied. The index (17 events on 2026-10-20) is imported, not re-read: the full node keeps about 5½ days.
- **2026-10-19: `0.0.13` (F25: the `events_kind` index) committed locally on top of `0.0.12`, not released or
  deployed.** It changes no response and no rollout order; the ingester adds the index to the existing
  database at its first start.
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

**Review date:** 2026-10-03; re-verified 2026-10-09 (code at `a3369bb` = tag `v0.0.9`); code gates closed in 0.0.10 (`5251f38`), released and deployed 2026-10-18; re-verified 2026-10-19 against the Rust rewrite (0.0.12, local commits, not released); F25 fixed in 0.0.13 (local commit); re-verified 2026-10-20 against 0.0.14 (PostgreSQL storage, local commits, not released); 0.0.15 (importer removed) re-verified the same day
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
**2026-10-19 (0.0.13):** F25 is RESOLVED (an `events_kind (stream, kind, id)` index created by the ingester, a
query that seeks it once per kind, query-plan and page-cost tests; OQ5 decided). Totals for F1–F26:
16 RESOLVED, 2 MITIGATED, 3 ADJUDICATED, 0 ACCEPTED-RISK, 1 DEFERRED (F19, a client change), 4 Positive.
**2026-10-20 (0.0.14, storage in the shared PostgreSQL tier; D59 revised, D60; OQ7 decided):** F1–F26 re-verified
against the new storage (a "Re-verified 2026-10-20" note on each finding the change touches; the SQLite-specific
ones are marked N/A or carried over with evidence: the persisted-WAL `unsafe` call and the read-only mount are gone, F3's
volume loss becomes database-backup coverage). F27–F32 added: F27 (role separation and least privilege, Positive), F28
(shared fate with the auth PostgreSQL, ACCEPTED-RISK with mitigations), F29 (single writer by advisory lock, Positive),
F30 (migration safety, Positive), F31 (the temporary SQLite import path, MITIGATED, removal gated on 0.0.15), F32 (TLS
`verify-full` to the database and secret handling, Positive). F3 stays MITIGATED (the database tier's daily dump now covers
the index; the offsite copy is operator work and not configured). Totals for F1–F32: 16 RESOLVED, 3 MITIGATED (F2, F3, F31),
3 ADJUDICATED, 1 ACCEPTED-RISK (F28), 1 DEFERRED (F19, a client change), 8 Positive (F12, F13, F14, F22, F27, F29, F30, F32).
**2026-10-20 (0.0.15, the SQLite importer removed):** F31 is RESOLVED: the `import-sqlite` feature, `ingest import-sqlite`, `rusqlite`/`libsqlite3-sys` and the
crates only they pulled in, the importer and parity tests, the SQLite fixture builder, `tests/golden`, `migration/` and the legacy manifests are gone (evidence in F31).
A41, A42 and A48 are N/A (the data moved and the importer no longer exists); the Section D rows for the 0.0.14 cutover and the 0.0.15 removal are ticked. Totals for F1–F32:
17 RESOLVED, 2 MITIGATED (F2, F3), 3 ADJUDICATED, 1 ACCEPTED-RISK (F28), 1 DEFERRED (F19, a client change), 8 Positive (F12, F13, F14, F22, F27, F29, F30, F32).

**Front matter (RUST lens, from 0.0.12):**

**Toolchain:** `rust-version = "1.88"` (workspace; checked by the CI `msrv` job, and locally with 1.88 on
2026-10-19); builds on stable 1.98.1   **Edition:** 2024
**Crates published:** no (`publish = false`; shipped as an image)
**Runtime:** tokio — `ingest`: `current_thread` (`rt`, `time`, `signal`, `macros`); `api`: `multi_thread`
with `min(available_parallelism, 4)` workers (`net`, `time`, `signal`, `sync`)

| Crate | Binary | Direct dependencies (exact pins) |
| --- | --- | --- |
| `sui-indexer-store` | (library) | 0.0.14: `tokio-postgres =0.7.18`, `deadpool-postgres =0.14.2` (the API's pool), `tokio-postgres-rustls =0.14.0` with `rustls =0.23.46` (**ring** provider, the one `sui-rpc`/tonic already use: one TLS stack), `tokio`, `percent-encoding =2.3.2`, `sha2 =0.11.1`, `serde`, `serde_json`, `getrandom =0.4.3`; `rusqlite =0.40.2` (`bundled`) **only behind the cargo feature `import-sqlite`** (the 0.0.14 ingester image; removed in 0.0.15). Up to 0.0.13: `rusqlite` unconditionally |
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
| Runtime FS | 0.0.14: **no volume** (all state is in PostgreSQL; the CA is a read-only ConfigMap mount at `/etc/postgres-ca`). Up to 0.0.13: `/data` (SQLite, 1 Gi Longhorn RWO PVC), mounted read-only in the `api` container from 0.0.12. The manifests set `readOnlyRootFilesystem: true`, `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`, `runAsNonRoot` (65532), `seccompProfile: RuntimeDefault` and `automountServiceAccountToken: false`. |
| Deployed by | infrastructure workspace `post-bootstrap/sui-indexer` (read 2026-10-09); digest from `config/images.yaml` (`sha256:b3484ce4…8a58`, 0.0.10, since 2026-10-18; `sha256:aea58153…1704` at 0.0.9) stamped into the overlay |
| Build args | `VERSION`, `VENDOR`, `DESCRIPTION`, `SOURCE_URL`, `DOCUMENTATION_URL`, `IMAGE_URL` — all labels; no secrets |

**Location:** `sui-indexer/docs/audit/sui-indexer-audit.md`. This is the canonical audit (repo-local).

> **Access note:** `meddleware-org/sui-indexer` is not in this session's attached repository list.
> It is public, so it was cloned read-only. Nothing was pushed.

---

## Executive summary

**2026-10-20 — the one-off SQLite importer is removed (0.0.15, local commit; F31 RESOLVED).** Nothing of SQLite is left in the code, the image or the
supply chain: the `import-sqlite` cargo feature (store, ingest, api, testkit), `ingest import-sqlite`, `crates/store/src/import.rs`,
`crates/ingest/src/import_cmd.rs`, the testkit's SQLite fixture builder (`seed-fixture sqlite`), the importer tests (`crates/ingest/tests/import_sqlite.rs`, 9) and the
import-parity test (`crates/api/tests/import_parity.rs`, with `tests/golden/0.0.13.json` and `scripts/golden-from-0.0.13.sh`) are deleted, with
`rusqlite` and `libsqlite3-sys` and the crates only they pulled in (`rsqlite-vfs`, `sqlite-wasm-rs`, `hashlink`, `fallible-streaming-iterator`, `foldhash`,
`hashbrown` 0.16, `vcpkg`; the ingester's SBOM goes from 182 to 175 crates, the API's 116 is unchanged). The Dockerfile has no feature argument
(still two cargo invocations, one per binary); `rust-ci.yml`, `docker-publish.yml`, `scripts/check-sbom.sh` (its `CARGO_FEATURES` option) and `scripts/smoke.sh` (the importer step) lose
every feature reference. No behaviour change for the API: the HTTP surface, the schema and the migrations are untouched (`0001_init.sql` keeps its original text, including a
comment that names the old SQLite store, because a migration's checksum must never change). The import-parity test is **not** re-based on a PostgreSQL fixture:
it compared the PostgreSQL API with answers recorded from the SQLite API over a SQLite fixture, so without that comparison a recording of the PostgreSQL API
would only snapshot itself; the response contract stays pinned by the 43 handler tests and by the client-contract job (5 tests, the published clients against the
built `api`). Tests: 144 (store 65, ingest 36, api 43), the same as the default build of 0.0.14; the 10 importer and parity tests are gone. Section D: the
rows for the 0.0.14 cutover and the 0.0.15 removal are ticked (the cutover on the maintainer's report, not re-observed here).

**2026-10-20 — storage in the shared PostgreSQL tier (0.0.14, local commits; D59 revised, D60, OQ7).** The index
moves from the indexer's own SQLite file to its own database `sui_indexer` on the platform's `database` tier, with two
roles and two Deployments. The HTTP API is unchanged: 53 routes (heads, older pages, every filter shape, stats,
validation errors, redirects, 304s, preflight) answer **byte-identically** to a real 0.0.13 `api` binary over the same
data (`tests/golden/0.0.13.json`, regenerated by `scripts/golden-from-0.0.13.sh`; `crates/api/tests/import_parity.rs`; all three removed in 0.0.15), and a
fresh ingest from the public testnet node answered 97 routes identically to the live `https://sui-indexer.meddleware.co.uk`
(same 17 events, same ids; generation, ETag generation and poll times aside).

| Module | What changed |
| --- | --- |
| `store` (`db`, `migrate`, `writer`, `reader`, `read`, `import`, `testdb`, `config`) | the only crate with SQL, now on `tokio-postgres`: `Store` = the owner connection holding `pg_try_advisory_lock` (single writer), migrations under `pg_advisory_xact_lock`, atomic page + cursor + counters; `Reader` = the API's pool over the SELECT-only role, **one statement per request** (head, counters and page in one snapshot); `DATABASE_URL` parsing with `verify-full` TLS only; the temporary SQLite importer (feature `import-sqlite`; removed in 0.0.15) |
| `ingest` | `start` connects, takes the writer lock, migrates, binds packages (backup = a `bak_<ms>` schema copy in the same transaction as the clearing), checks the chain; `run_loop` ends non-zero if the connection (the lock) is lost; `--healthcheck` reads the heartbeat from PostgreSQL; `import-sqlite` (removed in 0.0.15) |
| `api` | async handler over `Reader`; `503` until the schema is at the needed version and `503 database unavailable` while PostgreSQL cannot be reached; liveness no longer depends on the database |
| workspace `post-bootstrap/sui-indexer` | two Deployments (`sui-indexer-ingest`, `sui-indexer-api`), per-pod NetworkPolicies, no PVC; `migration/` (import Job, runbook, legacy manifests for rollback; deleted in 0.0.15) |

- **Tests (2026-10-20):** 144 Rust tests against a real PostgreSQL 18 with TLS (store 65, ingest 36, api 43; the 111
  cases of 0.0.13 are ported, and the WAL-persistence, cutover-copy and counter-backfill ones are replaced), **154 with `--features import-sqlite`** (the importer: 9; the
  import parity: 1; both removed in 0.0.15, back to 144); 5 client-contract tests (the published clients against the built `api`, a seeded database, the API
  as the SELECT-only role); 12 SBOM-gate script cases (11 in 0.0.15); the container smoke test (PostgreSQL with TLS, two containers,
  role separation, a second ingester refused, database outage and recovery). Line coverage 89.4% (`cargo llvm-cov
  --all-features`; both `main.rs` and `seed-fixture` are thin and run by the smoke and contract jobs). Every database
  test runs as the two production roles in an isolated database; CI sets `REQUIRE_TEST_DATABASE=1` and fails on any
  ignored or skipped test.
- **Measured (shared 4-CPU laptop with other workloads, PostgreSQL 18 with TLS on the same host, pool of 8, 64 keep-alive
  connections from a closed-loop client competing for the cores — a floor, not a benchmark):** `api` ≈ 3.6-4k req/s for the
  head page, a rare-kind page and a gate page, 2.5k for two kinds, 9.6k for `/coverage`, with 17 rows **or** 300,000 (cost is
  independent of history), RSS ≈ 10 MB; `ingest` ≈ 6 MB. The 0.0.13 SQLite build did ≈ 18k / 14k req/s in-process: the
  network database is 4-5× slower per origin request. See F25 (re-verified) for the cause and why it does not matter
  behind Cloudflare's cache.

**2026-10-19 — Rust rewrite (0.0.12, local commits).** The TypeScript service (below) is replaced by a Cargo
workspace; the HTTP API and the database are unchanged:

| Crate / module | Role |
| --- | --- |
| `store` (`lib.rs`, `config.rs`, `sui.rs`, `time.rs`) | the 0.0.11 schema verbatim; `Store` (the writer: atomic page + cursor ingest, counters and their one-time backfill, confirm-gated package binding, gap reset, heartbeats, the cutover backup) and `Reader` (read-only: `SQLITE_OPEN_READ_ONLY` + `query_only`); environment → config; address/type normalisation |
| `ingest` (`source.rs`, `decode.rs`, `ingestor.rs`, `chain.rs`, `lib.rs`, `main.rs`) | `sui-rpc` `ListEvents` with the page semantics of `@mysten/sui` `core.listEvents`; kind from the type, gate and commission from the node's JSON (no BCS decoding); poll loop, gaps, heartbeats; startup order; `--healthcheck` |
| `api` (`handler.rs`, `query.rs`, `ratelimit.rs`, `server.rs`, `lib.rs`, `main.rs`) | routing, validation, canonical URLs, cache headers and ETags, rate limit; WHATWG-exact URL/query handling; hyper server with timeouts, a connection cap and graceful shutdown |
| `testkit` | fixtures and `seed-fixture` (CI only) |

- **Tests:** 111 Rust tests (store 36, ingest 35, api 40; 7 of them property tests; plus one `--ignored` measurement), 5 client-contract tests
  (the published client packages against the built binary), 10 SBOM-gate script cases, the two-container
  smoke test. Line coverage 90.8% (`cargo llvm-cov`; the two `main.rs` are thin and run by the smoke test).
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
| RESOLVED | F1 (full-stream scans), F25 (rare-kind `kinds` filters; `events_kind`, 0.0.13), F4 and F18 (SBOM of the bundled code: npm in 0.0.10, Cargo per binary in 0.0.12), F5 (licence notices), F6 (cache key and limiter), F9 (casts, ETags), F10 (CI and image pipeline), F11 (documentation drift; last residual fixed 2026-10-19), F15 (pruned-cursor wedge), F16 (chain-identity startup bug; the startup path is now tested), F17 (libssl advisories in the base image), F20 (operator-keyed lookups), F21 (`noUncheckedIndexedAccess`; TS-only, superseded), F23 (health across two processes, 0.0.12), F26 (connection cap, 0.0.12), F31 (the temporary SQLite import path; removed in 0.0.15) |
| MITIGATED | F2 (stuck ingest; clients ignore `polledAt`, see F19), F3 (history loss on config change fixed; loss of the storage is now covered by the database tier's daily dump, the offsite copy is operator work and not configured) |
| ADJUDICATED | F7 (new event types need a fresh publish), F8 (display-only trust in the node; chain identity now checked), F24 (stored columns from the node's JSON rendering; parity gate) |
| ACCEPTED-RISK | F28 (shared fate with the auth PostgreSQL; D59, mitigations in the finding) |
| DEFERRED | F19 (clients treat a stale head page as unavailable): pre-mainnet gate row in Section D; the change lives in access-gate-client and seal-client |
| Positive | F12, F13, F14, F22 (the API cannot write — by role since 0.0.14), F27 (role separation), F29 (single writer by advisory lock), F30 (migration safety), F32 (TLS `verify-full`, no secret in logs) |

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
- 2026-10-20: the storage becomes a network database. The boundary moves from "a read-only file mount" to "a database
  role that has no write grant" (F22, F27), the single-writer rule from "one process on one volume" to an advisory lock
  (F29), the schema from `CREATE IF NOT EXISTS` to versioned migrations (F30). New trust points: the shared PostgreSQL's
  availability (F28) and, for one release (0.0.14), the import path (F31, removed in 0.0.15). The single `unsafe` call of the SQLite era is gone.

---

## Threat model / trust boundaries

| Actor / source | Controls | Can do | Bounded by |
| --- | --- | --- | --- |
| Anonymous client (through Cloudflare) | URLs, headers, request rate | Read pages; force origin work with cache-miss URLs | Edge cache; per-IP bucket on `CF-Connecting-IP` (IPv6 by /64, bounded map); ingress limit 20 r/s; strict params; cacheable `301` to the canonical URL (F6); O(1) counters instead of scans (F1). The Cloudflare rate-limit rule is **not** set (free plan, maintainer item in the infrastructure README) |
| Client reaching the origin off-tunnel (in-cluster pod, mis-exposed Service) | `CF-Connecting-IP` | Forge the header to dodge the per-IP limit | SECURITY.md: only via the tunnel. The NetworkPolicy (default-deny ingress) admits only the `nginx-ingress` namespace and `platform-probe` on 8080 |
| Full node at `GRPC_URL` | the event stream (BCS and its JSON rendering) | Serve fabricated or omitted events; a JSON rendering that disagrees with the BCS; hang | https (rustls, webpki roots); 30-second timeout per request plus the SDK's 30-second body-idle watchdog; exact type match at the original ID; strict JSON field checks, else counted as undecodable (F24); the clients decode the BCS again; chain identifier must match `NETWORK` (F8, F16). **No authenticity of rows (F8, display-only)** |
| A compromised `api` process (from 0.0.12) | its own container and its database login | Serve wrong pages; read the index | cannot write or change the schema: the SELECT-only role (0.0.14; before: read-only mount, `SQLITE_OPEN_READ_ONLY`, `query_only`) (F22, F27); no chain access; its credential reaches only its own database (`REVOKE … FROM PUBLIC`) |
| A compromised `ingest` process (0.0.14) | the owner login of `sui_indexer`, the full node's stream | Write or destroy the index | confined to its own database (F27); the tier's dump (F3); nothing outside it is writable; no ingress; a second writer is refused (F29) |
| Another tenant of the PostgreSQL tier | connections, load, the tier's availability | Starve or take down the index | per-role connection limits and timeouts; the API answers `503` and the clients fall back (F28, accepted) |
| Operator / config (`NETWORK`, `GRPC_URL`, `*_ORIGINAL_ID`, `DATABASE_URL`) | what is indexed, where it is stored | A package-ID change clears a stream | Validated and normalised IDs; startup refuses until `REINDEX_CONFIRM` names the stream; `VACUUM INTO` backup first; generation rotation (F3). The backup sits on the same volume |
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
| Shared state store | the `sui_indexer` database in the shared PostgreSQL tier | one writer by advisory lock (F29); the `api` role is SELECT-only (F22, F27); versioned migrations by the owner only (F30); `verify-full` TLS (F32); the tier's role limits and dump (F28, F3) |
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
| CI build and publish jobs | the builds compile; the push job pushes; the merge job signs and attests | SHA-pinned actions; OIDC; full CI reused on the tagged commit; builds hold no registry credentials; Trivy scan by digest before sign (F10, D61) |
| Cluster manifests | runtime security context | `post-bootstrap/sui-indexer` read 2026-10-09: full restricted context, probes, limits, NetworkPolicy (Section B) |

---

## Severity scale

Critical / High / Medium / Low / Info / Positive.

## Scope

**In scope 2026-10-20 (0.0.15, the importer removed; local commit on top of `eb8e46a`):** the deletions listed in the Executive summary; `Cargo.toml`/`Cargo.lock`
(version, removed packages), the three crate manifests, `crates/store/src/{lib,writer,error,migrate,tests}.rs`, `crates/ingest/src/{lib,main}.rs`, `crates/testkit/src/{lib,main}.rs`,
`crates/api/src/handler.rs` (comments), `Dockerfile`, `about.hbs`, `scripts/{smoke.sh,check-sbom.sh,tests/check-sbom-test.sh}`, both workflows, `README.md`, `CLAUDE.md`, `AGENTS.md`,
`SECURITY.md`, `CHANGELOG.md`; workspace `post-bootstrap/sui-indexer/{apply.sh,README.md,base/kustomization.yaml}` (`migration/` deleted), `OPERATOR_TASKS.md`, `STORAGE.md`,
`docs/architecture/ADR-0001-chain-access-layering.md`, `docs/STATE.md`, `docs/audit/LENS_GROUNDING_LOG.md`.

**In scope 2026-10-20 (0.0.14, PostgreSQL; local commits on top of `c0ab2fa`):** the Rust workspace as for 0.0.12, with `crates/store/src/{db,error,migrate,read,reader,writer,import,testdb}.rs` and
`crates/store/migrations/0001_init.sql`, `crates/ingest/src/import_cmd.rs` and `crates/ingest/tests/`, `crates/api/tests/import_parity.rs`, `crates/testkit/src/sqlite.rs`,
`tests/golden/`, `scripts/{smoke.sh,test-postgres.sh,golden-from-0.0.13.sh,check-sbom.sh}`, `Dockerfile`, both workflows; workspace `post-bootstrap/sui-indexer/**`
(`base/`, `migration/`), `k8s/base/database/{README.md,scripts/provision.sh}`, `k8s/clusters/meddleware-org/database/{services.conf,networkpolicy-consumers.yaml}`, decisions D59/D60 (read-only).

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

**Environment / commands (2026-10-20, 0.0.15; `cargo +stable` 1.98.1, Linux x86_64, PostgreSQL 18 in rootless podman with TLS from `scripts/test-postgres.sh`):**

| Command | Result |
| --- | --- |
| `cargo +stable fmt --check` / `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean / clean (the second feature set no longer exists) |
| `cargo test --workspace --locked` | **144 passed** (store 65, ingest 36, api 43), 0 ignored, 0 skipped |
| `cargo llvm-cov --workspace --locked --summary-only` | lines 89.2%, functions 88.4%, regions 89.4% (0.0.14: 89.4 / 89.1 / 87.3 with the importer) |
| `cargo +1.88 check --locked --workspace --all-targets`; `cargo +1.88 test --locked --workspace` | passes; 144 passed (store 65, ingest 36, api 43); MSRV 1.88 unchanged |
| `cargo deny check` | advisories ok, bans ok, licenses ok, sources ok (every licence in `deny.toml` and `about.toml` is still in use; none was SQLite-only) |
| `cargo about generate --locked --fail` for both binaries | passes; the notices no longer mention SQLite |
| `cargo cyclonedx` + `scripts/check-sbom.sh`, no feature argument | `api` lists all 116 crates, `ingest` all 175 (was 182 with `rusqlite`); `scripts/tests/check-sbom-test.sh` 11 cases and `check-index-test.sh` 9 cases pass |
| `cargo build --release --locked -p sui-indexer-api -p sui-indexer-ingest` | release binaries: `api` 2,862,848 bytes (0.0.14: 2,847,120), `ingest` 4,327,128 bytes (0.0.14 with `rusqlite`: 6,414,464; -2.09 MB, -32.5%); built per binary as the Dockerfile does |
| `grep -rniE 'sqlite\|import-sqlite\|rusqlite\|indexer\.db\|DB_PATH\|VACUUM' crates scripts .github Dockerfile Cargo.lock` | one match in code: the comment of the immutable migration `0001_init.sql` (Executive summary) |
| Not run | the GitHub workflows (publish, Trivy, cosign, actionlint); the image build and the smoke test (no container build daemon in this session: the smoke script lost its importer step and was renumbered, checked with `bash -n`); the client-contract job; `kubectl` of any kind |

**Environment / commands (2026-10-20, 0.0.14; `cargo +stable` 1.98.1, Linux x86_64, 4 CPUs shared with other workloads; PostgreSQL 18.6 in rootless podman with TLS, a throwaway CA, `scripts/test-postgres.sh`; roles also created by the real `provision.sh`):**

| Command | Result |
| --- | --- |
| `cargo fmt --check` / `cargo clippy --workspace --all-targets --locked -- -D warnings` (default and `--all-features`) | clean / clean |
| `cargo test --workspace --locked` | **144 passed** (store 65, ingest 36, api 43), 0 ignored, 0 skipped |
| `cargo test --workspace --locked --all-features` | **154 passed** (+ importer 9, import parity 1) |
| `cargo llvm-cov --workspace --locked --all-features --summary-only` | lines 89.4%, functions 89.1%, regions 87.3% |
| `cargo +1.88 check --locked --workspace --all-targets --all-features`; `cargo +1.88 test --workspace --locked --all-features` | passes; 154 passed (MSRV 1.88 unchanged: no new dependency raised it) |
| `cargo deny check` (0.20.2) | advisories ok, bans ok (duplicates warned: `base64`, `block-buffer`, `const-oid`, `crypto-common`, `digest`, `getrandom`, `syn`), licenses ok, sources ok |
| `cargo cyclonedx` + `scripts/check-sbom.sh` | `api` SBOM lists all 116 crates of its graph; `ingest` (with `--features import-sqlite`) all 182; `check-sbom-test.sh` 12 cases pass |
| `tests/contract` (Node 26.12, `@meddleware/access-gate-client` 0.0.9, `@meddleware/seal-client` 0.0.20) | 5 passed against `target/release/api`, a seeded database, the API as the SELECT-only role |
| `podman build` + `DOCKER=podman bash scripts/smoke.sh` | image 39 MB; the smoke test passes (PostgreSQL with TLS, two containers, role separation, second ingester refused, outage and recovery) |
| Golden parity | `scripts/golden-from-0.0.13.sh` against a real 0.0.13 `api` over the importer's fixture: 53 routes; the imported PostgreSQL index reproduces all 53 byte for byte |
| Live (read-only) | `ingest` against `fullnode.testnet.sui.io` into a local PostgreSQL (17 events, the same as the live instance), `api` on it as the SELECT-only role: 97 routes identical to `https://sui-indexer.meddleware.co.uk` (generation, ETag generation and poll time aside) |
| Load (closed-loop client, 64 keep-alive connections, 8 s, pool 8, limiter raised, PostgreSQL on the same host) | F25 table: head page ≈ 3.6-4.0k req/s, rare kind ≈ 3.9k, gate ≈ 3.4k, two kinds ≈ 2.5k, `/coverage` ≈ 9.6k; `api` RSS ≈ 10 MB, `ingest` ≈ 6 MB |
| Not run | the GitHub workflows (publish, Trivy, cosign, actionlint); the cluster (database tier, cutover); `kubectl apply` of any manifest |

**Environment / commands (2026-10-19, Rust rewrite; `cargo +stable` 1.98.1, Linux x86_64, 4 CPUs):**

| Command | Result |
| --- | --- |
| `cargo fmt --check` / `cargo clippy --all-targets --locked -- -D warnings` | clean / clean |
| `cargo test --locked` | **111 passed** (store 36, ingest 35, api 40) and 1 ignored measurement (`f25_measurement_on_300k_rows`), 2026-10-19 on 0.0.13 |
| `cargo llvm-cov --summary-only` | lines 90.8%, functions 87.7%, regions 89.6% (`main.rs` of both binaries 0%: thin, run by the smoke test) |
| `cargo +1.88 check --locked --workspace --all-targets` | passes (MSRV 1.88; let-chains need it) |
| `cargo deny check` (0.20.2) | advisories ok, bans ok (duplicates warned: `getrandom`, `syn`), licenses ok, sources ok |
| `cargo about generate --fail` (0.9.2), per binary | passes; `api` 44 crates, `ingest` 137 crates |
| `cargo cyclonedx` (0.5.9) + `scripts/check-sbom.sh` | `api` SBOM lists all 63 crates of its graph, `ingest` all 139; a removed crate fails (`scripts/tests/check-sbom-test.sh`, 10 cases) |
| `tests/contract` (vitest, `@meddleware/access-gate-client` 0.0.9, `@meddleware/seal-client` 0.0.20) | 5 passed against `target/release/api` |
| `podman build` | image 38.3 MB; `DOCKER=podman bash scripts/smoke.sh` passes (two containers, API mount `:ro`) |
| Live (read-only) | `ingest` against `fullnode.testnet.sui.io` into a scratch DB (11 events), `api` on it: 10 routes and 6 error/redirect cases identical to `https://sui-indexer.meddleware.co.uk` (generation and poll time aside); parity 19/19 (F24) |
| Load (dev-only closed-loop client, 64 keep-alive connections, 10 s, limiter raised) | head page ≈ 18k req/s (11 rows), ≈ 14k req/s (300k rows); `gate=` filter ≈ 5.2k req/s; `kinds=AccessConsumed` on 300k rows **10 req/s** on 0.0.12 (F25). On 0.0.13 (same 300k rows, 16 keep-alive connections from a Python client, so the client bounds the top figure): head page ≈ 5.9k, `kinds=AccessConsumed` ≈ 5.9k (0.0.12 binary on the same data: 7), two kinds ≈ 5.2k (0.0.12: ≈ 5.8k; one seek per kind is a little dearer when both kinds are common), `gate=` ≈ 5.8k req/s. Single thread through `Reader::page`: 8 pages/s before, ≈ 33,000 after. Index build on 300k rows ≈ 0.5 s; 8,554 head-page reads ran meanwhile, the slowest 0.57 ms |
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

**Re-verified 2026-10-19 (0.0.12, Rust):** The counters are kept verbatim: `Store::ingest` (`crates/store/src/lib.rs`) bumps `stream_stats`, `gate_stats` and `gate_commission` in the page transaction (commission summed as `u128`, stored as decimal text), and `backfill_stats` rebuilds them once (`stats:v2`). `coverage`, `health`, `gate_stats` and `head_id` are single-row or index reads. Tests: *counters_stay_exact_across_duplicates_and_agree_with_a_recount*, *counters_are_rebuilt_…*, *coverage_over_50k_rows_is_a_constant_time_read*, *commission_totals_are_exact_beyond_64_bits*. Disposition unchanged (RESOLVED). One request shape still walked rows in 0.0.12: a `kinds` filter for a rare kind (F25, pre-existing); fixed in 0.0.13 (note on F25).

**Re-verified 2026-10-20 (0.0.14, PostgreSQL):** The counters are kept in the ingest transaction (`Store::ingest`: a `stream_stats` upsert with `LEAST`/`GREATEST`, `gate_stats`, and `gate_commission` as `numeric(39,0)` summed in SQL, exact beyond 64 bits). The one-time backfill (`stats:v2`) is **gone — N/A**: the schema is created with the counters and every row enters through `ingest`; the importer copies the old counters and re-verifies them against a recount of the copied rows (`Store::verify_counters`). `coverage`, `health`, `gate_stats` and `head_id` are single-row or index reads, and an events response is **one statement** (facts and page in one snapshot, one round trip). Tests: *counters_stay_exact_across_duplicates_and_agree_with_a_recount*, *commission_totals_are_exact_beyond_64_bits*, *a_recount_detects_counters_that_do_not_match_the_rows*, *coverage_over_50k_rows_is_a_constant_time_read* (timing and plan: the counter read never touches `events`), *the_events_statement_keeps_the_intended_index_for_every_shape* (under 300 buffers on 30,000 rows, `max(id)` included). Disposition unchanged (RESOLVED).

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

**Re-verified 2026-10-20 (0.0.14):** The heartbeats are rows of a new `heartbeats` table (`poll:<stream>`, `ingest:alive`; they were `meta` rows, moved because they are rewritten many times a minute: `fillfactor 50`, so updates stay heap-only), written by the ingester after each poll and read by `/healthz` in one statement (`Reader::polls`); the JSON and the 5 × `POLL_MS` rule are unchanged. New: with PostgreSQL unreachable, `/healthz` is `503` with every stream `ok: false` and `lastError: "database unavailable"` (the cause is logged, not sent). Tests: *polls_the_streams_independently_and_health_names_the_failing_one*, *heartbeats_round_trip*, *a_database_that_cannot_be_reached_is_503_database_unavailable_and_recovers*. Disposition unchanged (MITIGATED; the clients' use of `polledAt` is F19).

### F3 — Clearing a stream permanently loses history the full node has pruned

**Severity:** Low   **Disposition:** MITIGATED (the destructive-clear path RESOLVED in 0.0.5, `1973fca`, OQ1 decided; since 0.0.14 loss of the storage is covered by the database tier's backup, whose offsite copy is operator work and not configured)
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

**Re-verified 2026-10-20 (0.0.14, PostgreSQL):** *SQLite-specific parts.* The `VACUUM INTO` backup before a confirmed clear becomes a copy of the whole index into a new schema `bak_<epoch ms>` (`CREATE TABLE … AS`), **made in the same transaction as the clearing**: if the backup fails nothing is cleared (*the_backup_and_the_clearing_are_one_transaction*; the schema name is validated before any SQL, *copies_the_index_into_a_backup_schema_before_clearing_and_the_copy_restores* shows the restore statement, ids included, and that the API role cannot read the copy). The refusal without `REINDEX_CONFIRM`, the single-stream clear and the generation rotation are ported unchanged (the six *bind_packages* cases and *start_refuses_a_package_change_until_confirmed_then_backs_up_and_reindexes*). The `.bak-pre-rust-*` cutover copy is **N/A** (obsolete).
*Volume loss → database backup coverage.* There is no volume any more. What the workspace provides, read 2026-10-20 (`k8s/base/database/README.md`, `backup-cronjob.yaml`, `scripts/backup.sh`; **authored and tested against local containers only, not applied to the cluster**): the `postgres-backup` CronJob runs daily at 03:17 UTC, `pg_dump -Fc` per database (every non-template database, so `sui_indexer` and its `bak_` schemas are included) plus globals, to the in-cluster MinIO, verified by object size, a `COMPLETE` marker written last, the newest 14 sets kept; a documented restore runbook and `restore-drill.sh`; an optional offsite copy to R2, encrypted to a public certificate whose private key stays offline (newest 30 sets). **Still operator work:** the tier is not applied yet; the offsite copy is not configured (the job logs `OFFSITE NOT CONFIGURED`), and on this single node MinIO shares the disk with PostgreSQL, so **until it is, the dump protects against `DROP DATABASE`, a bad migration or a corrupted volume, not against losing the node**; the MinIO credentials are the root stopgap; no restore drill has been run on the cluster; no alert on backup age. Tracked in `OPERATOR_TASKS.md` ("Database tier") and Section D (pre-mainnet). The SQLite history older than the node's retention is irreplaceable until the first successful backup after the import: the cutover runbook (`post-bootstrap/sui-indexer/migration/README.md`) keeps two copies of the SQLite files off the cluster and leaves the old volume untouched for a day. Disposition unchanged (MITIGATED).

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

**Re-verified 2026-10-20 (0.0.14):** The CI now starts a PostgreSQL 18 with TLS (`scripts/test-postgres.sh`: the image the database tier runs, pinned by digest, a throwaway CA, started by a script rather than as a `services:` container because the tests need TLS) for the `check`, `msrv` and `contract` jobs; `REQUIRE_TEST_DATABASE=1` makes a missing server a failure, and the test step fails on any ignored or skipped test. The default build and the `--all-features` build (the temporary importer and the import-parity check) are both linted and tested (0.0.15: only the default build remains); the SBOM gate is per binary and per feature set (F18; 0.0.15: per binary); the image job runs the new smoke test (PostgreSQL, two containers). Not run here: the GitHub workflows themselves.

**Re-verified 2026-10-20 (release workflow, D61; commit 417b5ff):** `docker-publish.yml` no longer builds both architectures in one QEMU-emulated job (the 0.0.14 publish took about 1.5 hours, almost all of it the arm64 compile). It is now `version`, `verify` (`rust-ci.yml`) and `sbom-crates` (the two CycloneDX per-binary SBOMs, checked by `scripts/check-sbom.sh`), then `build` (a matrix: amd64 on `ubuntu-latest`, arm64 on the native `ubuntu-24.04-arm`; OCI archives, `provenance: mode=max`, **no registry credentials** in the job that compiles the dependencies), `publish-arch` (pushes `<commit>-<arch>` with `skopeo copy --all`, keeping each image's provenance), `merge-docker-public` (`docker buildx imagetools create` per registry, the quay.io and Docker Hub digests asserted equal, `scripts/check-index.sh` asserting both architectures and both attestations, **Trivy by digest, then** cosign sign, the syft SPDX and two CycloneDX attestations, the build-provenance attestation on both registries, output `digest`) and `mirror-private` (unchanged, still by digest). Tags, labels (the same `docker/metadata-action` configuration in each build), build arguments, `latest=auto` and the signer identity `docker-publish.yml@refs/tags/v*` are unchanged; `setup-qemu-action` is gone. The intermediate `<commit>-amd64` / `<commit>-arm64` tags are left in place, as in `nft-gate` and `seal-key-server` (the registries' push credentials cannot be assumed to delete, deleting a tag does not delete the manifests the index references, and `verify-digests.sh` ignores them: its newest-tag check reads only `N.N.N` tags). A `workflow_dispatch` run is a **dry run** (input `dry_run`, default on): both native builds, then the merge, `check-index.sh`, a label check and the Trivy scan against a throwaway `registry:3` service on the runner; no push, no signature, no secret. Evidence on 2026-10-20: `actionlint` clean; the merge step reproduced locally, with the installed `docker buildx` 0.37 against a local registry, from the two per-architecture indexes of the published `seal-key-server` (which uses the same OCI-archive plus `skopeo` flow): the merged digest `sha256:8609cbbe…5497134` equals the published `0.0.2` index byte for byte, so the merge is deterministic and keeps the provenance; `check-index.sh` passes on the published `sui-indexer` 0.0.13, `seal-key-server` 0.0.2 and `nft-gate-gateway` 0.0.24 indexes and fails on a per-architecture index; `scripts/tests/check-index-test.sh` (9 cases, run in `rust-ci.yml`); Trivy 0.74.0 scanned a localhost registry image by digest with `TRIVY_NON_SSL`. **Not yet exercised on GitHub:** the native arm64 build, the OCI-archive export of this Dockerfile, `skopeo` pushes with the real credentials, signing and attestation of the merged digest, and the dry-run job itself; the next tag (or a manual dry run once this is on the default branch) is their test.

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

**Re-verified 2026-10-20 (0.0.14, PostgreSQL):** Ascending reads, page + cursor in one transaction (`Store::ingest` on a `Transaction`), `ON CONFLICT (tx_digest, event_index) DO NOTHING`, the null end cursor, the page budget and the 30 s timeout are ported unchanged. *Row id = ledger order* now rests on an identity column written by **one connection that holds the writer lock** (F29) instead of one process on one volume: ids come from one sequence and are committed one transaction at a time, so a reader never sees a higher id without every lower id of the stream that will ever exist. The invariant is **order, not gapless**: an ignored duplicate and a rolled-back page consume numbers, and nothing reads an id as a count (the ETag is `"<generation>-<newest id>"`, cursors are `<generation>.<id>`, counts come from the counters). Test: *ids_are_ordered_not_gapless_and_nothing_depends_on_contiguity* (a duplicate and a failed page in the middle; paging, head id and counters unaffected). Preserved across the cutover: the importer keeps the old ids and generation, so a pre-cutover cursor `e32e37afbbe9.<id>` still means the same row (*imports_the_index_with_its_ids_generation_cursors_and_counters*, the golden parity). Positive stands.

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

**Re-verified 2026-10-20 (0.0.14):** Every route, status, header, ETag, cursor and error body is byte-compatible with 0.0.13: 53 routes (heads, older pages, every kind/gate combination, stats, 400/404/410/405/301/304/204) match a **real 0.0.13 `api` binary** over the importer's fixture (`tests/golden/0.0.13.json`), and 97 routes match the live production instance over the same 17 events ingested fresh from the testnet node. The order of checks is kept (not ready `503`, cursor syntax `400`, generation `410`, limit `400`). New answers for a new failure: `503 {"error":"database unavailable"}` with `no-store` (the cause logged at most every 10 s, never sent). Positive stands.

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

**Re-verified 2026-10-20 (0.0.14):** No volume in the image or the pods; `ENV PORT=8080` only (no `DB_PATH`); the database CA is mounted at run time and the credentials arrive as a Secret, none baked in. The Dockerfile built the API and the ingester in **two cargo invocations**, so the ingester's temporary `import-sqlite` feature never reached the API binary (cargo unifies features within one build; one invocation again since 0.0.15). A local podman build is 39 MB; `DOCKER=podman bash scripts/smoke.sh` passes against it with a real PostgreSQL (role separation, a second ingester refused, outage and recovery). Not run: the release workflow, cosign, Trivy. Positive stands.

**Re-verified 2026-10-20 (0.0.15):** the Dockerfile has no `CARGO_FEATURES` argument any more: the dependency layer and the build compile the two binaries in two cargo invocations as before (measured: one joint build is 16 KB larger for `api` and 36 KB for `ingest`, from cargo unifying dependency features), with no feature argument, and `cargo about` runs without a feature flag. No SQLite is compiled or linked, and the licence notices drop the SQLite sentence of `about.hbs`. The image was not rebuilt in this session (no container build daemon); the CI `image` job and the smoke test build and run it. Positive stands.

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

**Re-verified 2026-10-20 (0.0.14):** The Cargo SBOM gate is feature-aware: `scripts/check-sbom.sh` takes `CARGO_FEATURES` and the workflows describe the ingester with `--features import-sqlite` (it links SQLite in the 0.0.14 image) and the API without (116 and 182 crates, both pass). *check-sbom-test.sh* gained the cases that the variable reaches `cargo metadata` (12 cases). Disposition unchanged (RESOLVED).

**Re-verified 2026-10-20 (0.0.15):** the feature-aware gate is simplified back: the `CARGO_FEATURES` option of `scripts/check-sbom.sh` and both workflows' `--features import-sqlite` are removed (no binary has a feature any more). `api` lists all 116 crates, `ingest` all 175 (was 182 with `rusqlite`); *check-sbom-test.sh* has 11 cases (the stub-cargo case now asserts `--locked` and the release platform). Disposition unchanged (RESOLVED).

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

**Re-verified 2026-10-20 (0.0.14, PostgreSQL) — the mechanism changes, the property is kept and strengthened.** The three layers of the SQLite era (read-only mount, `SQLITE_OPEN_READ_ONLY`, `query_only`) are replaced by: (1) the API connects as a database role with **no write, no DDL and no sequence-usage grant** and `default_transaction_read_only = on` (F27); (2) every pooled connection also runs `SET default_transaction_read_only = on`; (3) the API's SQL is `SELECT`-only code in `crates/store/src/read.rs` and it has no chain access. The role is the control that survives a compromised process: *the_reader_cannot_write_cannot_change_the_schema_and_sees_new_commits* shows `INSERT`, `UPDATE`, `DELETE`, `TRUNCATE`, `CREATE TABLE`, `DROP TABLE`, `ALTER TABLE` and `CREATE INDEX` all refused with `42501` **even after the client switches the read-only default off**, while reading every index table works. The persisted-WAL `unsafe` FFI call and its read-only-mount workaround are **gone: the workspace has no `unsafe` code** (`unsafe_code = "deny"`, no exception: A16). The ingester and the API are separate Deployments with separate credentials. Before the ingester has migrated, the API answers `503` and creates nothing (*before_the_ingester_migrates_everything_but_options_is_503_and_the_api_creates_nothing*: no table exists afterwards); smoke steps 2 and 6. Positive stands.

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

**Re-verified 2026-10-20 (0.0.14):** The rows live in the `heartbeats` table (F2). `ingest --healthcheck` now opens a fresh short-lived connection and fails when PostgreSQL does not answer within 4 s, when the row is missing, or when it is older than `5 × POLL_MS + 30 s`; a full node that is down still does not fail it. The API's **liveness probe is a TCP check** (a database outage must not restart it) while readiness stays `/readyz`, which now reports the schema version and database reachability (`503 index not created yet` / `503 database unavailable`). Tests: *the_healthcheck_follows_the_liveness_heartbeat_and_the_database* (including an unreachable database), *the_loop_stops_when_the_connection_carrying_the_writer_lock_is_lost*, *a_restart_continues_from_the_persisted_heartbeat*. Disposition unchanged (RESOLVED).

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

**Re-verified 2026-10-20 (0.0.14):** `decode.rs` is unchanged. `commission_mist` is now a `bigint` with `CHECK (commission_mist >= 0)`, so the 63-bit rule is also enforced by the column (an out-of-range value was already undecodable before it reached a statement). Live check: the 17 events of the current `access_gate` package ingested by 0.0.14 from `fullnode.testnet.sui.io` equal the rows the live 0.0.13 instance serves (97 routes byte-identical, F13). Disposition unchanged (ADJUDICATED).

### F25 — A `kinds` filter for a rare kind walks the stream (found 2026-10-19 by the load test)

**Severity:** Low   **Disposition:** RESOLVED (2026-10-19, 0.0.13, commit 5411b13; pre-existing in 0.0.4–0.0.12; OQ5 decided)
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

**Remediation (applied in 0.0.13):** `CREATE INDEX IF NOT EXISTS events_kind ON events (stream, kind, id)` in
`SCHEMA`, which only the ingester runs at startup, so a fresh database and an existing one both get it and
the read-only API never creates it. It is additive (0.0.11 and 0.0.12 ignore it, so a rollback still works).
The index alone was not enough, so the page query changed too (`page_sql`, `crates/store/src/lib.rs`):
- With the index, SQLite uses it for one kind but still walks `events_stream` for `kind IN (a, b)` (it prefers
  that to sorting), and the `AccessMinted,AccessConsumed` request the treasury and DAO apps send is exactly
  that shape. Several kinds (no gate) now run one bounded seek per kind (`UNION ALL` of `ORDER BY id DESC
  LIMIT n+1` arms, merged by id): cost O(kinds × limit), independent of history.
- Where the index would hurt, it is kept out: with a `gate`, the planner chose `events_kind` over `events_gate`
  for `kinds=AccessMinted&gate=…` and would walk a common kind to find one gate's rows. `+kind` makes the
  kinds a plain row filter there, so a `gate=` page uses `events_gate` as before.
- Repeated kinds are ignored (a repeat would duplicate rows in the merge); the API already canonicalises them.

**Evidence (2026-10-19):**
- Query plans (`EXPLAIN QUERY PLAN`, pinned by *every_filter_shape_uses_the_intended_index*):

  | Shape | 0.0.12 | 0.0.13 |
  | --- | --- | --- |
  | none, or `before` only | `events_stream` | `events_stream` |
  | one kind (± `before`) | `events_stream` (walk) | `events_kind (stream=? AND kind=? [AND id<?])`, no sort |
  | several kinds (± `before`) | `events_stream` (walk) | `MERGE (UNION ALL)` of one `events_kind` seek per kind |
  | `gate` | `events_gate` | `events_gate` |
  | `gate` + kinds | `events_gate` | `events_gate` (kinds filter the gate's rows) |

- Page cost: *a_rare_kind_page_over_300k_rows_costs_a_handful_of_steps* (300,000 rows, 5 of the rare kind):
  under 5,000 SQLite VM steps for one and for two kinds, with and without a cursor; without the index the
  same page takes over 100,000 (the test fails if the index is dropped).
- Equivalence: *merged_kind_seeks_return_what_the_filter_returns* compares 200 random queries (kinds incl.
  repeats, gate, cursor, limit) against the plain `kind IN` filter, rows and `next`.
- Migration: *the_ingester_adds_the_index_to_an_older_database_and_it_is_idempotent*: an older database
  (index dropped) is read correctly by the read-only `Reader` without it, gains it on `Store::open`, and a
  second open changes nothing; a fresh database has it. The 0.0.11 TypeScript store ignores it.
- Measured (`f25_measurement_on_300k_rows`, `--release --ignored`): one rare kind 8 → ≈ 33,000 pages/s
  (single thread); two kinds, one rare ≈ 9,000/s; over HTTP on the same data ≈ 5.9k req/s against 7 for the
  0.0.12 binary (client-bound, see Environment). **Creating the index on 300,000 rows took ≈ 0.51 s inside
  `Store::open`** (about 11 MB more in the file and WAL); readers keep reading under WAL: 8,554 head pages were
  served meanwhile, the slowest in 0.57 ms. The ingester does not poll for that time, which is far inside the
  `5 × POLL_MS + 30 s` liveness window; the live index (11 rows) builds instantly.
- Residual, accepted: a `gate` filter on a very large gate with a rare kind still reads that gate's rows
  (bounded by the gate, as in every earlier release); two common kinds cost two short seeks instead of one
  (≈ 10% in the HTTP figures above). If the API starts before the ingester has created the index (both containers
  start together, so for seconds), a rare-kind page is slow as in 0.0.12; multi-kind pages then run one walk
  per kind, so up to the number of kinds slower than before, until the index exists.

**Re-verified 2026-10-20 (0.0.14, PostgreSQL) — the same three indexes, a different planner.** `events_stream`, `events_gate` and `events_kind (stream, kind, id)` are created by migration 1. The query shapes are kept (one bounded seek per kind merged by `UNION ALL`; a gate with kinds filters the gate's rows — `(kind || '')` instead of SQLite's `+kind` keeps the planner off `events_kind`); the plan tests are now `EXPLAIN` based: *every_filter_shape_uses_the_intended_index* (ten shapes on 30,000 mixed rows after `ANALYZE`: an index, never a table walk or a sort; a rare *stream* — 1 row in 1000 — is read through `events_stream`, not by walking the primary key), *the_events_statement_keeps_the_intended_index_for_every_shape* (the statement an events request runs, buffers under 300), *a_rare_kind_page_over_300k_rows_touches_a_handful_of_buffers* (under 200 buffers and no filtered rows for one and two kinds, with and without a cursor; without the index the page filters past the table), *merged_kind_seeks_return_what_the_filter_returns* (200 random queries against the plain filter). **New PostgreSQL hazard, found while writing the tests:** the one-kind statement is shared by every kind (`kind = $3`), and its *generic* plan (`EXPLAIN (GENERIC_PLAN)`) walks the primary key backward with a filter, which is a table walk for a rare kind; `plan_cache_mode = auto` may adopt a generic plan after five executions. It did not on the test data, but that is a cost estimate, so the API's pooled connections run `SET plan_cache_mode = force_custom_plan` (values known, statistics used): *pooled_connections_use_custom_plans_so_a_rare_kind_keeps_its_index*. **Measured (2026-10-20, shared 4-CPU laptop, PostgreSQL 18 with TLS on the same host, pool of 8, 64 keep-alive connections from the closed-loop client, 8 s each; the 0.0.13 figures were in-process SQLite):**

| Request (rows in the stream) | 0.0.13 SQLite | 0.0.14 PostgreSQL |
| --- | --- | --- |
| head page (17 rows) | ≈ 18k req/s | ≈ 3.8-4.0k req/s |
| head page (300,000 rows) | ≈ 14k | ≈ 3.6k |
| `kinds=AccessConsumed` (300,000 rows, 5 of them) | ≈ 5.9k (Python client) | ≈ 3.9k |
| two kinds, one rare (300,000 rows) | ≈ 5.2k | ≈ 2.5k |
| `gate=` (300,000 rows, 300 gates) | ≈ 5.2-5.8k | ≈ 3.4k |
| `/coverage` | n/a | ≈ 9.6k |
| RSS of `api` (peak) | 8-26 MB | ≈ 10 MB |

The origin is 4-5× slower per request than the embedded engine, and the cost does not grow with history (17 rows or 300,000 give the same rate). The backends' planning of the one combined statement under forced custom plans (≈ 0.25 ms) is the largest share, then TLS and the 4 cores shared with the load generator and PostgreSQL; a split into a facts statement and a page statement in a pipelined transaction would save perhaps a third and was not taken (it adds a transaction to every request and a cancellation hazard). **Does it matter behind Cloudflare?** No: the origin sees cache misses only (head page `s-maxage=30`, older pages `immutable` for a year, `301`s cacheable), the per-IP bucket allows 10 r/s and the ingress 20 r/s, and the public endpoint is rate limited far below 3.6k req/s per replica; the figure that bounds the service is the edge, not the database. Disposition unchanged (RESOLVED); A43 holds.

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

**Re-verified 2026-10-20 (0.0.14):** `server.rs` is unchanged (1024 connections, 32 KiB request head). The pool adds the database-side bound: `DB_POOL_SIZE` (default 8) connections per replica, a 3 s wait for a free one (then `503 database unavailable`), a 5 s connect timeout, and the role's `CONNECTION LIMIT 20` (`services.conf`), so replicas × pool plus a rolling update's surge must stay within 20 (one replica: 16 during a rollout). Test: *a_request_that_times_out_does_not_poison_the_pool*. Disposition unchanged (RESOLVED).

### F27 — Positive: role separation and least privilege at the database (0.0.14)

**Severity:** Positive
**Where:** `crates/store/src/{writer,reader,testdb}.rs`; workspace `k8s/clusters/meddleware-org/database/services.conf`, `k8s/base/database/scripts/provision.sh`, `networkpolicy-consumers.yaml`; `post-bootstrap/sui-indexer/base`.

- **Two roles, two credentials, two pods.** `sui_indexer_ingest` owns the database `sui_indexer` (DDL and writes; used only by the
  ingester and the one-off import Job); `sui_indexer_api` has `CONNECT`, `USAGE` on the schema and `SELECT` on tables and
  sequences only (default privileges follow tables the owner creates later), plus `default_transaction_read_only = on`. Each pod
  mounts only its own Secret (`sui-indexer-ingest-db`, `sui-indexer-api-db`). The control is the grant, not a flag every future
  deployment must remember (F22).
- **Own data only.** `provision.sh` runs `REVOKE ALL ON DATABASE … FROM PUBLIC` and on the `public` schema, so neither role can
  connect to another service's database; per-role `CONNECTION LIMIT` (8 / 20), `statement_timeout` (60 s / 5 s) and
  `idle_in_transaction_session_timeout` bound a runaway client. The database namespace admits 5432 from the two pods by namespace
  **and** label; the ingester has no ingress at all (default-deny).
- **Evidence.** The test harness provisions every test database the same way (owner, SELECT-only role, `REVOKE … FROM PUBLIC`, the
  same timeouts), so the API tests run as the SELECT-only role. *the_reader_cannot_write_cannot_change_the_schema_and_sees_new_commits*
  (writes and DDL refused with `42501` even with the read-only default off; every index table readable),
  *the_owner_can_migrate_and_the_api_role_cannot*, *a_role_cannot_connect_to_another_services_database* (both roles of one database are
  refused by another), *copies_the_index_into_a_backup_schema_…* (the API role cannot read a `bak_` schema); smoke step 6 against the
  real image; and the roles were created by the **real `provision.sh`** against a local PostgreSQL 18 (2026-10-20) for the live-data
  run: the ingester wrote as the owner, the API read as the SELECT-only role.
- **Residual (workspace, not this repo): closed for the auth databases, applied on the cluster 2026-10-20 (provision job: three `hardened … keeps CONNECT` lines; Kratos, Hydra and Keto reconnected afterwards).** PostgreSQL grants `CONNECT` on
  every database to `PUBLIC` by default, so a new role could open a connection to the adopted auth databases and read catalog metadata (not data).
  The three `harden` lines of `services.conf` are now enabled (`REVOKE ALL ON DATABASE … FROM PUBLIC`, explicit `CONNECT` for the owner;
  proved against a local PostgreSQL 18 with the real `init.sh`: the auth owners keep working, `sui_indexer_*` and arbitrary roles are refused, a re-run
  converges; `k8s/base/database/tests/harden-backup-test.sh`). What remains: the maintenance database `postgres` (and `template1`) still grant
  `CONNECT` to `PUBLIC`; it holds no data (`harden postgres` is commented in `services.conf`, safe to enable).

### F28 — Shared fate with the auth PostgreSQL (D59, 2026-10-20)

**Severity:** Low   **Disposition:** ACCEPTED-RISK (decided in D59 revised; mitigations below; revisit at the 3-node HA step)
**Where:** workspace `k8s/base/database` (one StatefulSet, one Longhorn volume, one node); D59, D60, OQ7.

**Issue:** the chain index now shares a PostgreSQL instance with the Ory auth stack. An outage, a full volume, a connection storm or a bad
change in the tier takes the index with it; a runaway indexer takes capacity from auth.

**Why accepted:** the index is display data and its clients fall back to the full node (ADR-0001 D15), so an outage degrades lists,
not authorisation; the move buys least privilege by role (F27), one backup/upgrade/restore story (D60) and independent scheduling of
the two pods, at the cheapest moment (17 testnet rows).

**Mitigations (verified):**
- Per-role `CONNECTION LIMIT` 8 / 20 against `max_connections` 100 (the provisioner warns when the sum approaches the limit), `statement_timeout`
  60 s / 5 s, `idle_in_transaction_session_timeout`; the ingester uses **one** connection and the API's pool is bounded (F26).
- The API connects lazily and answers `503 database unavailable`; its liveness is a TCP check, so a database outage causes no restart storm
  (*a_database_that_cannot_be_reached_is_503_database_unavailable_and_recovers*; smoke step 8); it recovers by itself.
- The ingester exits non-zero when its connection (the writer lock) is lost and Kubernetes restarts it (*the_loop_stops_when_the_connection_carrying_the_writer_lock_is_lost*).
- Cloudflare serves cached pages meanwhile (head 30 s, older pages immutable); clients fall back to the full node.
- A bad migration of this service is bounded by the owner role (own database) and made reversible by the tier's dump; `bak_` schema copies precede a confirmed clear.
- Cost control on the tier side: the index is small; the heartbeats table is tuned for heap-only updates (no bloat from the many writes).

**Residual (accepted):** one node, one disk; durability is the tier's backup, whose offsite copy is operator work (F3); a co-tenant's
outage is the index's outage. Revisit with the CloudNativePG step (the tier README keeps service name, roles, Secrets and the dump format).

### F29 — Positive: exactly one writer, enforced by an advisory lock held for the life of the connection (0.0.14)

**Severity:** Positive
**Where:** `crates/store/src/writer.rs` (`Store::connect_with`, `WRITER_LOCK_KEY`); `crates/ingest/src/lib.rs` (`run_loop`).

- The ingester takes `pg_try_advisory_lock(WRITER_LOCK_KEY)` on their one connection and hold it for the life of the
  process; another holder makes the start fail (after a 5 s grace for a predecessor's session still closing) with a message naming the
  writer lock and a non-zero exit. The lock is per database and per session: it vanishes with the connection, and the ingester treats a
  closed connection as fatal (`run_loop` returns `Err`, the process exits 1) rather than reconnecting into a state it cannot prove it owns.
- This replaces "one process on one RWO volume" as the thing that keeps `events.id` in ledger order (F12). A second pod, a hand-run
  tool, or a restarted pod overlapping its predecessor cannot interleave ids. The pod strategy stays `Recreate` as defence in depth.
- Tests: *a_second_writer_is_refused_while_the_first_lives_and_admitted_after*, *the_writer_lock_is_visible_as_an_advisory_lock_of_the_owner_session*
  (the lock row belongs to the owner role's backend), *start_refuses_a_second_ingester_while_one_is_running*, *the_loop_stops_when_the_connection_carrying_the_writer_lock_is_lost*; smoke step 5 against the real image.
- Residual: a network partition that leaves a stale backend holding the lock keeps the next start waiting out the server's detection (TCP
  keepalive 30 s idle is set on the client): it fails closed (a second writer is refused, never admitted). A superuser can still write
  (by design; the tier's superuser is not used by the service).

### F30 — Positive: migration safety (0.0.14)

**Severity:** Positive
**Where:** `crates/store/src/migrate.rs`, `crates/store/migrations/0001_init.sql`.

- Versioned, embedded, ordered migrations recorded in `schema_migrations` with a SHA-256 of each file; **applied by the ingester as the owner
  only**, each in its own transaction under `pg_advisory_xact_lock`, so two starts never apply the same one. A released
  migration edited afterwards is detected at the next start (checksum mismatch: refuses to run). A database migrated by a **newer** build is
  refused by an older ingester; the API accepts a newer schema (it needs `version >= SCHEMA_VERSION`).
- **The API cannot migrate** (no DDL grant) and does not try: `/readyz` and `/v1` answer `503` until the schema is at its version and the
  generation exists; it creates nothing.
- Rule recorded in README/CLAUDE.md: a migration keeps the previous release's queries working (add, then contract in a later release), because the previous
  image's API runs while the new ingester migrates.
- Tests: *migrations_apply_once_and_restarts_do_not_reapply*, *two_migrators_racing_on_an_empty_database_apply_each_migration_once*,
  *a_migration_applied_from_different_sql_is_refused*, *a_schema_newer_than_the_build_is_refused_by_the_ingester_and_served_by_the_api*,
  *the_owner_can_migrate_and_the_api_role_cannot*, *the_reader_is_not_ready_until_the_writer_has_migrated_and_started*, *the_column_types_and_constraints_refuse_what_the_code_never_writes*.
- Residual: the owner role's `statement_timeout` is 60 s; a future migration that builds a large index must `SET LOCAL statement_timeout` itself (migration 1 runs on empty tables).

### F31 — The SQLite import path (temporary, 0.0.14 only)

**Severity:** Low   **Disposition:** RESOLVED (0.0.15, 2026-10-20: the importer, its feature, `rusqlite` and the cutover tooling are removed; the import it served was verified before it committed and is reported done by the maintainer; commit 333fe69)
**Where (before 0.0.15):** `crates/store/src/import.rs`, `crates/ingest/src/import_cmd.rs` (feature `import-sqlite`), `crates/testkit/src/sqlite.rs`, `post-bootstrap/sui-indexer/migration/`.

**Risk (0.0.14):** a one-off tool that reads a live database file and writes the index is a place for silent data loss (a wrong import looks like a healthy empty
or partial index), it must preserve ids and the generation or every cached cursor breaks, it links SQLite C code into the ingester image for a release,
and it runs with the owner's credentials.

**Mitigations while it existed (verified in 0.0.14):** it read the file read-only in one transaction after `PRAGMA quick_check`; wrote in one PostgreSQL transaction on the writer
connection (so it held the writer lock) into an empty target only; copied ids, generation, package bindings, gap notes, cursors and counters; moved the identity sequence; and verified
row counts, every cursor, the commission total, the largest id, a SHA-256 over `id:tx_digest:event_index` and a recount of the counters before it committed (*a_verification_failure_rolls_everything_back*).
The fixture index answered 53 routes byte-identically to a real 0.0.13 binary. The image built the API without the feature.

**Resolution (0.0.15):**
- The cutover is done and settled on the maintainer's report: the legacy SQLite Deployment, the import Job, the old PVC, the legacy NetworkPolicies and the server backup of the old volume are
  deleted. This review did not re-observe the cluster; `config/images.yaml` pins the 0.0.14 digest.
- Code removed: the `import-sqlite` feature of all four crates, `ingest import-sqlite` (an unknown argument now exits 2: *unknown argument … (use --healthcheck or none)*), `crates/store/src/import.rs`,
  `crates/ingest/src/import_cmd.rs`, `crates/testkit/src/sqlite.rs` and `seed-fixture sqlite`, the `client_mut`/`set_generation` hooks that only the importer used, `crates/ingest/tests/import_sqlite.rs` (9 tests),
  `crates/api/tests/import_parity.rs` (1), `tests/golden/`, `scripts/golden-from-0.0.13.sh`. `Cargo.lock` loses `rusqlite`, `libsqlite3-sys` and `rsqlite-vfs`, `sqlite-wasm-rs`, `hashlink`,
  `fallible-streaming-iterator`, `foldhash`, `hashbrown` 0.16 and `vcpkg` (checked: `cargo deny check` passes, every licence entry is still in use, no SQLite-only licence entry existed).
- Pipeline removed: the Dockerfile `CARGO_FEATURES` argument, `--features import-sqlite` in `rust-ci.yml` and `docker-publish.yml` (clippy, tests, coverage, MSRV, the SBOM generation), the `CARGO_FEATURES` option of
  `scripts/check-sbom.sh` and its test, the importer step of `scripts/smoke.sh`.
- Workspace removed: `post-bootstrap/sui-indexer/migration/` (import Job, `import.sh`, `legacy/` manifests, runbook); `apply.sh` keeps only the first-apply check of the database Secrets and the CA.
- The parity test is not carried over (see the Executive summary): its oracle was the SQLite API. The remaining tests pin the same wire contract (handler tests, client contract).
- Not changed on purpose: `crates/store/migrations/0001_init.sql` still mentions the SQLite store in a comment; editing a released migration changes its SHA-256 and the ingester would refuse an
  already migrated database (F30).

Search 2026-10-20: `grep -rniE 'sqlite|import-sqlite|rusqlite|indexer\.db|DB_PATH|VACUUM|\bWAL\b' crates scripts .github Dockerfile Cargo.lock` has one match (the migration comment above); the remaining
mentions in the docs are history (this audit's older sections and log, the CHANGELOG).

### F32 — Positive: TLS to the database is verified, secrets stay out of logs (0.0.14)

**Severity:** Positive
**Where:** `crates/store/src/db.rs`; `crates/store/src/config.rs` (`database`); workspace Secrets and the `postgres-ca` ConfigMap.

- `DATABASE_URL` must carry `sslmode=verify-full` and `sslrootcert`; `disable`, `allow`, `prefer`, `require` and `verify-ca` are refused (they encrypt without authenticating the server),
  as are `sslcert`, `sslkey`, `options`, `host`, `hostaddr`, `channel_binding`, `target_session_attrs` in the URL, several hosts, Unix sockets, a missing user/database and an **empty password**.
  `ALLOW_INSECURE_DATABASE=1` (tests only) admits `sslmode=disable` and an empty password and nothing else. The CA file is read at startup (an unreadable or empty file is a startup error);
  the chain and the host name are verified by rustls; the TLS provider is **ring**, the one the gRPC client uses (one TLS stack).
- **No secret is logged.** `DatabaseSettings` has no `Display`, its `Debug` omits the password, and every URL error is a fixed message that does not echo the URL (the config `Debug` was checked too).
  The log line at startup names host, port, database and user, never the password. Passwords arrive only through the per-pod Secret.
- Tests: `db::tests` (seven cases incl. *nothing_prints_the_password*, *requires_verify_full_and_a_root_certificate*, *refuses_an_empty_password_unless_insecure*), the config cases
  (*the_database_url_is_required_and_validated_without_echoing_it*), *tls_is_verified_against_the_given_ca_when_the_server_offers_it* (the right CA connects, another CA and a missing or junk file are refused),
  and every database test of the suite runs over `verify-full` against a TLS server with a throwaway CA.
- Residual (workspace): the tier's server also accepts plaintext connections unless the optional `pg_hba` hardening is applied; this client never sends one. The Ory stack used `sslmode=require`; moving it to `verify-full` is
  applied 2026-10-20 (`pg_stat_ssl` shows ssl=true for the three Ory roles and both indexer roles): Deployments mount `postgres-ca`, `k8s/scripts/set-auth-dsn-tls.sh` rewrites only the DSN parameters, and
  the real Kratos, Hydra and Keto images were proved against `verify-full` (and refused a wrong CA, a host name outside the SANs and a missing CA). A hostname-mismatch refusal is verified by rustls' contract and by the CA test, not by a dedicated test (the test server's certificate carries
  the names it is reached by).

---

## Section A — Invariant verification matrix

Re-verified 2026-10-19 against the Rust code (0.0.12): file references below are the Rust ones where the
row says so; the TS-lens rows A16–A24 are replaced by RUST-lens rows (A16–A24 now) and A38–A43 are new.
Re-verified 2026-10-20 against 0.0.14 (PostgreSQL): rows whose mechanism was SQLite-specific (A3, A4, A6, A13, A16, A21, A34, A35, A37–A42)
are rewritten; A44–A50 are new.

| # | Invariant | Enforced at | Proven by | Status |
| --- | --- | --- | --- | --- |
| A1 | Display-only; rows verbatim; clients decode again and fall back | design; `serialize` | client-contract tests | HOLDS (clients do not yet use `polledAt` — F19) |
| A2 | One parser (the client packages); the indexer never decodes BCS | `crates/ingest/src/decode.rs` (type name + node JSON, BCS stored verbatim) | ingest tests; client-contract job; parity 19/19 (F24) | HOLDS (upgrade-defined types need a fresh publish — F7, decision) |
| A3 | Ledger order = row id; pages bounded by `before` immutable | `events.id` identity written by the one connection holding the writer lock (`writer.rs`, A45); ascending reads (`ingestor.rs`) | *ids_are_ordered_not_gapless_and_nothing_depends_on_contiguity*; ingest tests | HOLDS (ordered, not gapless: A49) |
| A4 | Page + cursor + counters atomic | `Store::ingest` (one `Transaction`) | *ids_are_ordered_…* (a failed page leaves the cursor); store tests | HOLDS |
| A5 | Generation rotates on rebuild or rebind; old cursors `410` | `Store`, `parseBefore` | tests; live generation rotated by the 2026-10-09 rebind | HOLDS |
| A6 | Package binding never mixes packages, and never deletes history unconfirmed | `Store::bind_packages`, `start` | the *bind_packages* tests (refuse, confirm, backup atomic with the clearing, legacy rows); *start_refuses_a_package_change_…* | HOLDS (F3; the backup is a schema in the same database, covered by the tier's dump) |
| A7 | Strict input; one cache entry per query | `api.ts` | validation and canonical-URL tests | HOLDS (F6) |
| A8 | Cheap under load | counters; cache headers; limiter | *Store counters* (50k-row constant-time read) | HOLDS (F1) |
| A9 | Ingest progresses despite a bad event; one stream cannot stall the other; a pruned cursor recovers | `Ingestor.pollOne` | resilience tests | HOLDS (F2, F15) |
| A10 | Full-node calls bounded in time | `tokio::time::timeout` 30 s per request (`Ingestor::poll_stream`), 30 s at startup (`CHAIN_CHECK_TIMEOUT`); the SDK's body-idle watchdog | *bounds_every_full_node_request*; *an_unreachable_node_is_an_error_not_a_hang* | HOLDS |
| A11 | Image SBOM covers what runs | publish workflow (syft image SBOM + one CycloneDX SBOM per binary) | `scripts/check-sbom.sh` + `scripts/tests/check-sbom-test.sh` in CI and the release | HOLDS (F18; 0.0.12 not yet released) |
| A12 | Licence obligations met in the image | `THIRD_PARTY_LICENSES-{api,ingest}.txt` (cargo-about `--fail`), CI `notices` stage | local `cargo about` run; CI | HOLDS (F5) |
| A13 | Least privilege at runtime | Dockerfile; manifests; the database roles (F27) | manifests rendered with `kubectl kustomize` 2026-10-20; image config; smoke test | HOLDS (non-root, read-only root FS, no volume, caps dropped, seccomp, no SA token, per-pod NetworkPolicy, per-pod database credentials) |
| A14 | The node serves the configured network | `crates/ingest/src/{chain,lib}.rs` (`start`) | chain tests; *start_refuses_a_node_of_another_chain*; live start against testnet | HOLDS (F8, F16; the startup path is now tested) |
| A15 | Operator-keyed lookups cannot resolve unintended entries | `config.rs`, `chain.rs` (exact-match slices) | *treats_network_as_data*, *has_no_inherited_entries* | HOLDS (F20) |
| A16 | RUST — memory safety: `unsafe_code = "deny"` workspace-wide, no exception | `Cargo.toml` `[workspace.lints]`; the persisted-WAL FFI call of 0.0.12-0.0.13 is gone with SQLite | clippy (`--all-targets`; no feature set is left); `grep unsafe crates` finds none | HOLDS (0.0.14: no `unsafe` code at all) |
| A17 | RUST — panics: none on attacker-reachable paths; release `overflow-checks = true`, `panic = "abort"` | request parsing validates before `parse()` (1–4 / 1–15 digits); indexing only after length checks; poisoned locks recovered with `into_inner` (the API's limiter and pool) | proptests (query parsing, page assembly); handler tests | HOLDS (the `expect`s are on startup, serialisation of owned structs and pre-validated digits) |
| A18 | RUST — crypto-crate usage | none: no signatures, keys or MACs; TLS is rustls with the **ring** provider, for the gRPC client (webpki roots) and for PostgreSQL (`verify-full` against the tier's CA, F32) — one stack | — | N/A |
| A19 | RUST — hand-rolled decoding: bounded, property-tested | the URL/query parser (`query.rs`) is the only hand-written parser; gRPC/protobuf is generated (prost, tonic 4 MiB message cap) | 6 proptests in `query.rs`, 1 in `tests.rs` (page assembly) | HOLDS |
| A20 | RUST — HTTP hygiene: bodies, header sizes, concurrency, outbound timeouts, https | bodies never read; 32 KiB request head; 1024 connections; 5 s / 10 s timeouts; gRPC 30 s per request, https enforced by `config.rs` | *an_oversized_request_head_is_refused*, *a_client_that_never_finishes_its_headers_is_cut_off*, config tests | HOLDS (F26) |
| A21 | RUST — shared state: single writer (advisory lock); atomic page + cursor; fail closed | `Store::connect_with`, `Store::ingest` transaction, `run_loop` (A45); the `api` role (F22) | store and ingest tests | HOLDS |
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
| A34 | IMG — runtime configuration inventory: image `ENV PORT=8080`; the Deployments set `NETWORK`, `POLL_MS`, `DATABASE_URL` (from the per-pod Secret), and for the API `PORT`, `DB_POOL_SIZE`, `RATE_LIMIT_*`; no `DB_PATH` | image `ENV`; manifests | image config; B.IMG-2 | HOLDS (the only secret is `DATABASE_URL`, never logged: F32) |
| A35 | IMG — health and resources: `api` readiness `/readyz`, liveness TCP; `ingest` exec probe `--healthcheck` (database + heartbeat); per-pod requests and limits | manifests | B.IMG-2; smoke test | HOLDS (manifests rewritten 2026-10-20, not yet applied) |
| A36 | IMG — scan before sign and verification command pinned to the workflow identity | `rust-ci.yml`, `docker-publish.yml` | F10, B.IMG-1 | HOLDS |
| A37 | IMG — deployment pinned by digest from `config/images.yaml` | overlay; `verify-digests.sh`; `config/images.yaml` is at 0.0.14; the old-digest refusal of `apply.sh` and `migration/import.sh` was removed in 0.0.15 with the SQLite era | B.IMG-2; `verify-digests.sh` | HOLDS (the overlay pins the 0.0.14 digest; the maintainer pins 0.0.15 after publishing — Section D) |
| A38 | The API never writes and never creates anything in the database | the SELECT-only role (F27), `SET default_transaction_read_only = on` on every pooled connection, SELECT-only code | F22 tests; smoke steps 2 and 6 | HOLDS |
| A39 | `/healthz` and `polledAt` reflect the ingester's real polls | heartbeat rows (`heartbeats poll:<stream>`) | F23 tests | HOLDS |
| A40 | Stored `kind`/`gate_id`/`commission_mist` equal what the client parsers derive from the BCS | `decode.rs` | parity 19/19 on live testnet events (F24); fixture tests | HOLDS for the kinds seen live; the other kinds rest on the Move field names |
| A41 | Data continuity across the storage move: ids, generation, package bindings, cursors and counters of the 0.0.13 SQLite index reproduce in PostgreSQL; cursor encoding unchanged; no reindex | (was `crates/store/src/import.rs`; `PageBuilder`) | (were *imports_the_index_with_its_ids_generation_cursors_and_counters*, *the_imported_index_answers_like_the_sqlite_one*, live parity 97 routes) | N/A since 0.0.15 (the move is done and the importer is deleted; the cursor encoding stays pinned by the `PageBuilder` and handler tests; the cutover is reported done by the maintainer, F31) |
| A42 | The cutover is reversible: the legacy volume is never written; off-cluster copies are kept | (was the read-only importer and `migration/README.md`) | (was *reads_the_sqlite_file_without_changing_it*) | N/A since 0.0.15 (the legacy volume, the import Job and the backup of the old volume are deleted on the maintainer's instruction; no rollback to SQLite is intended or possible, F31) |
| A43 | Request cost independent of history size for every query shape | counters (F1); indexes; custom plans on pooled connections | `EXPLAIN` plan and buffer-count tests; load test (17 rows and 300,000 give the same rate) | HOLDS since 0.0.13 (F25), re-verified on PostgreSQL 2026-10-20; it did not hold for rare-kind filters in 0.0.12 |
| A44 | Only the owner role writes; the API role has no write, DDL or sequence-usage grant, and cannot read another service's database | `provision.sh` grants; `Reader` setup | F27 tests; smoke step 6 | HOLDS |
| A45 | Exactly one writer, for the life of its connection; a lost connection stops the ingester | `pg_try_advisory_lock` (`writer.rs`); `run_loop` | F29 tests; smoke step 5 | HOLDS |
| A46 | Migrations are versioned, checksummed, owner-only and race-free; the API serves only a schema at its version | `migrate.rs`; `Reader::check` | F30 tests | HOLDS |
| A47 | TLS to the database is `verify-full` only; no secret in logs or errors | `db.rs`; `config.rs` | F32 tests | HOLDS |
| A48 | An imported index equals its source (ids, order, counts, cursors, commission, counters) or nothing is committed | (was `import.rs`) | (were the F31 tests) | N/A since 0.0.15 (the importer is removed) |
| A49 | Ids are ordered, not gapless; no ETag, cursor or paging logic assumes contiguity | `read.rs`, `handler.rs` | *ids_are_ordered_not_gapless_and_nothing_depends_on_contiguity* | HOLDS |
| A50 | A pooled API connection is read-only, never holds an open transaction, and never uses a generic plan | `Reader` (one statement per request; `SESSION_SETUP`) | *a_cancelled_request_leaves_the_pool_usable*, *pooled_connections_start_read_only_…*, *pooled_connections_use_custom_plans_…* | HOLDS |

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
| Least privilege | Yes | `contents: read` default; `id-token`/`attestations: write` on the merge job only; the build jobs hold no registry credentials (D61) |
| Tag ↔ version | Yes | `version` job |
| `--locked` builds + `cargo deny` in CI and publish (0.0.12; `npm ci` + audit before) | Yes | `rust-ci.yml`, reused by the release |
| Release runs the full CI on the tagged commit | Yes | `verify: uses: ./.github/workflows/rust-ci.yml` (0.0.12; `node-ci.yml` before) |
| Image scan before sign (published image) | Yes | `docker-publish.yml` `merge-docker-public`: "Scan the published image" by the merged index digest, before "Sign images" |
| Native per-architecture builds, merged by digest | Yes (next release; D61) | `build` matrix (`ubuntu-latest`, `ubuntu-24.04-arm`) → `publish-arch` → `merge-docker-public`; `scripts/check-index.sh` (tests: `scripts/tests/check-index-test.sh`); the `<commit>-<arch>` tags are intermediates (F10) |
| Release dry run (no push) | Yes (not yet run on GitHub) | `workflow_dispatch` input `dry_run` (default on): `merge-dry-run` against a local registry |
| Signature + SBOM + provenance, no `continue-on-error` on public registries | Yes | B.IMG-1 |
| SBOM covers the compiled code | Yes (0.0.10 npm; 0.0.12 Cargo per binary) | F18 (`cargo cyclonedx` attestations checked by `scripts/check-sbom.sh` in CI and the release) |
| Config scan / container smoke test | Yes | `rust-ci.yml` `image` job; `scripts/smoke.sh` (two containers) |
| Client contract against the shipped binary | Yes (0.0.12) | `rust-ci.yml` `contract` job (Node 26, the published client packages) |
| actionlint | No | not run here (sibling repos run it) |
| Dependabot | Yes | `.github/dependabot.yml` (cargo, npm for `tests/contract`, docker, github-actions; weekly, grouped) |

### B.RS-1 Build & release (from 0.0.12)

- `cargo fmt --check` and `cargo clippy --workspace --all-targets --locked -- -D warnings` (there is no feature set beyond
  the default) in CI (`rust-ci.yml` `check`); the declared `rust-version` (1.88) is checked by the `msrv` job
  (`cargo check --locked --workspace --all-targets` and, from 0.0.14, the tests, against PostgreSQL, on 1.88;
  the check also run locally 2026-10-20: no new dependency raised the floor).
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

### B.IMG-2 Cluster manifests (`post-bootstrap/sui-indexer`; rewritten 2026-10-20 for 0.0.14, not yet applied)

| Item | Value |
| --- | --- |
| Workloads | `sui-indexer-ingest` (1 replica, `Recreate`, `/app/ingest`, no port, no Service, **no volume**) and `sui-indexer-api` (1 replica, stateless, `/app/api` :8080; more replicas are safe, the HA path is in the README); the same image by digest from `config/images.yaml` via the overlay; Service `sui-indexer` selects the API (name unchanged for the Ingress and the status probe) |
| Credentials | `DATABASE_URL` from Secret `sui-indexer-ingest-db` (owner) and `sui-indexer-api-db` (SELECT-only), key `url`, one Secret per pod; the CA from ConfigMap `postgres-ca` at `/etc/postgres-ca` (read-only); no other secret |
| Pod security | `runAsNonRoot`, uid/gid 65532, `seccompProfile: RuntimeDefault`, `automountServiceAccountToken: false`; each container `allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: true`, `capabilities.drop: [ALL]`; no writable mount at all |
| Probes | `api`: readiness `/readyz` (schema at the needed version + database answers), liveness TCP (a database outage must not restart it); `ingest`: exec liveness `/app/ingest --healthcheck` (60 s initial delay, every 30 s, 3 failures); `/healthz` for monitoring only |
| Resources | `ingest` requests 10m / 32 Mi, limits 500m / 128 Mi; `api` requests 20m / 32 Mi, limits 500m / 128 Mi (measured RSS ≈ 6 / 10 MB, F25) |
| Network | per-pod default-deny ingress; API: allow from `nginx-ingress` and `platform-probe` on 8080; ingester: nothing; the database namespace admits `app in (sui-indexer-ingest, sui-indexer-api)` of namespace `apps` on 5432 (`networkpolicy-consumers.yaml`); Ingress rate limit 20 r/s burst 80; no egress policy in `apps` |
| Runtime env | `NETWORK=testnet`, `POLL_MS=15000`, `DATABASE_URL` (both); `PORT=8080`, `DB_POOL_SIZE=8`, `RATE_LIMIT_PER_SEC=10`, `RATE_LIMIT_BURST=40` (`api`); no `REINDEX_CONFIRM`, no `DB_PATH` |
| First apply | `apply.sh` refuses the first apply (no `sui-indexer-ingest` Deployment yet) while the two database Secrets or the `postgres-ca` ConfigMap are missing. The cutover tooling (`migration/`, the legacy-Deployment, legacy-volume and old-digest guards) was removed in 0.0.15 |
| Not present | a PodDisruptionBudget (pointless for one replica on one node; the HA path names it); an egress NetworkPolicy; the cluster has not been touched |

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

### C.1 Coverage grade — A (0.0.15: 144 Rust tests against a real PostgreSQL; the startup wiring is tested)

**2026-10-20 (0.0.15):** the 10 tests of the importer and the import parity are removed with the code they covered (144 remain: store 65, ingest 36, api 43, the same as the default run of 0.0.14); 5 contract tests, 11 SBOM-gate cases and the smoke test are unchanged in kind. Coverage: lines 89.2%, functions 88.4%, regions 89.4% (0.0.14: 89.4 / 89.1 / 87.3 with the importer).

**2026-10-20 (0.0.14, PostgreSQL):** every database test creates its own database provisioned like production and runs the writer as the owner and the
API as the SELECT-only role (`sui_indexer_store::testdb`); there is no mock. 144 tests (store 65, ingest 36, api 43), 154 with `--features import-sqlite`
(+ the importer's 9 and the import parity). The 111 cases of 0.0.13 are ported; the WAL-persistence, `bak-pre-rust` cutover and `stats:v2` backfill cases have no
PostgreSQL counterpart and are replaced by the writer-lock, migration, role-separation, TLS, one-snapshot, cancellation and importer cases. `cargo llvm-cov
--workspace --all-features`: lines 89.4%, functions 89.1%, regions 87.3% (the two `main.rs` and `seed-fixture` are 0%: thin, run by the smoke and contract jobs;
`import_cmd.rs` 78%). Without `TEST_DATABASE_URL` a database test prints `SKIPPED` and returns; CI sets `REQUIRE_TEST_DATABASE=1` (a missing server panics) and
fails on any ignored or skipped test, so the layers below cannot silently shrink.

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
| Store / ingest / handler tests (a real PostgreSQL 18 with TLS, one database and the two production roles per test; scripted full node) | `crates/{store,ingest,api}/src/tests.rs`, `crates/store/src/index_tests.rs` (query plans), `crates/store/src/testdb.rs` (harness) (incl. 1 proptest) | yes (`REQUIRE_TEST_DATABASE=1`) |
| Local-socket integration | gRPC stub (`tonic` server) for `GrpcSource`; hyper server tests | yes |
| Client contract (published TS clients against the built `api` binary, a seeded PostgreSQL, the API as the SELECT-only role) | `tests/contract/contract.test.ts` (5) | yes (`contract` job) |
| SBOM gate | `scripts/tests/check-sbom-test.sh` (11 cases) and the real SBOMs (per binary) | yes (`check` job and the release) |
| Image smoke test (PostgreSQL with TLS, the ingester and the API as two containers, role separation, outage and recovery) | `scripts/smoke.sh` | yes (`image` job) |
| Config and image scans | Trivy config scan, Trivy image scan | yes (CI, and on the published image before signing) |
| Local run against testnet | README → Development (manual; done 2026-10-19 with the parity check) | no |

Every test project that exists runs in CI. Up to 0.0.11: one vitest project (52 tests at 0.0.10).

### C.2 Hermetic vs. live paths

| Path | Hermetic? | Deferred to | Tracking |
| --- | --- | --- | --- |
| gRPC `ListEvents` semantics (ordering, cursors, timeout) | 0.0.12: scripted source, frame proptest, local LedgerService stub with the real client; cursor equality with `@mysten/sui` checked live once | manual testnet run before a release | F9, F12 |
| Startup chain-identity check and package binding | 0.0.12: yes (`start` tests with a scripted source) | live start of each release | F16 |
| JSON renderings of kinds not yet seen on testnet | no (fixtures written from the Move sources) | the first such events: re-run the parity check | F24 |
| Container runtime (nonroot, read-only root, probes, PostgreSQL over TLS) | smoke test covers nonroot, read-only root, `/readyz`, headers, the exec probe, the writer lock, the outage; probes and security context are read from the manifests (rendered, not applied) | cluster | F10, B.IMG-2 |
| The database tier on the cluster | no (the roles created by the real `provision.sh` on a local PostgreSQL); the cutover from SQLite is done and its tooling removed (0.0.15) | the cluster | F28 |
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
  list history, the status page's "Chain index" is green — maintainer item (release and cluster), D58.
  *2026-10-20: `config/images.yaml` pins 0.0.13; whether these post-rollout checks were done is not recorded here,
  and the settled 0.0.14 cutover (ticked below) makes the live checks (events, generation, `/healthz`, the consuming apps, the status page) the maintainer's to confirm.*
- [x] **0.0.14 (PostgreSQL) released and cut over** — D59/D60, F27–F32. Evidence: `config/images.yaml` pins the 0.0.14 digest `sha256:ed5f66b2…0566`; the maintainer reports (2026-10-20) that the cutover is done and settled and that the legacy SQLite
  Deployment, the import Job, the old PVC, the legacy NetworkPolicies and the server backup of the old volume are deleted. Not re-observed by this review (no cluster access); the per-step checks of the former runbook (import report, pre-cutover cursor, `/healthz`,
  consuming apps, status page) are therefore the maintainer's, not recorded here.
- [x] **the SQLite importer removed (0.0.15)** — F31: the `import-sqlite` feature, `rusqlite`, the importer, the fixture builder, `tests/golden` and `migration/` are deleted (Executive summary, F31; commit 333fe69); the 0.0.15 image is built and pinned by the maintainer's release (tag, `refresh-digests.sh`, `sync-manifests.sh`), which `OPERATOR_TASKS.md` tracks as "sui-indexer 0.0.15 release"

### pre-mainnet

- [x] SBOM includes the bundled dependencies (IMG-M8) — F18 (0.0.10, commit `4aab344`): second SPDX
  attestation from `npm sbom --omit dev`, checked by `scripts/check-sbom.mjs` and `tests/sbom.test.ts`;
  confirmed on the released 0.0.10 digest, 2026-10-18: two SPDX attestations (image SBOM, 17 packages; npm packages SBOM, 24 packages) verified with `cosign verify-attestation --type spdxjson`
- [ ] clients treat a stale head page as unavailable — F19; coordinated client change, gated on mainnet
- [ ] a mainnet deployment recorded in both clients (startup otherwise throws by design); then
  `NETWORK=mainnet` with the recorded chain id `35834a8a` — mainnet-blocked (access-gate-sui and
  seal-policies-sui publication, `OPERATOR_TASKS.md` "Mainnet release custody")
- [x] `noUncheckedIndexedAccess` enabled (F21) and the `Object.hasOwn` hygiene (F20) — 0.0.10, commit `4aab344`: flag on, `tsc` clean; `Map` lookups with tests
- [ ] backup of the index with an offsite copy, and a restore drill that includes `sui_indexer` (F3, F28): the tier's `postgres-backup` CronJob covers the database
  once the tier is applied; the offsite copy (R2, encrypted to an offline key) is **not configured**, and until it is, losing the node loses the dump with the data —
  maintainer item (`OPERATOR_TASKS.md` "Database tier" steps 4-5); supersedes the volume-snapshot row for `sui-indexer-data`, which no longer exists
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
- [x] rare-kind `kinds` filters bounded — F25, OQ5: `events_kind (stream, kind, id)` created by the ingester,
  one seek per kind; query-plan and page-cost tests; 8 → ≈ 33,000 pages/s on 300k rows (0.0.13, commit 5411b13)
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
- **Shared storage tier (D59/D60).** The indexer is the tier's second consumer after the Ory stack and the template for the next: own database, an owner and a
  SELECT-only role, `verify-full` TLS, the CA as a ConfigMap, per-pod NetworkPolicy by label, the tier's dump as the backup. The same checklist (tier README "Onboarding")
  applies to every service that follows; the shared-fate risk (F28) is the tier's, not each consumer's.
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
   confirmation or an archive — **holds** (F3: confirmation plus a `bak_<ms>` schema copy made in the same transaction; loss of the
   storage is covered by the database tier's dump, whose offsite copy is not configured).
4. MUST attach an SBOM that covers the bundled runtime code — **holds from 0.0.10** (F18); MUST ship its
   licence notices — **holds** (F4, F5).
5. MUST map equivalent requests to one cache key — **holds** (F6).
6. MUST have exactly one writer, enforced by the database — **holds** (F29: advisory lock for the life of the connection; a lost connection stops the ingester).
7. MUST give the API no write or DDL privilege, and the ingester no access to other services' data — **holds** (F27: SELECT-only role; `REVOKE … FROM PUBLIC`).
8. MUST verify the database server's identity and MUST NOT log the connection URL or password — **holds** (F32: `verify-full` only; redacted settings).
9. MUST apply schema changes only by versioned, checksummed migrations run by the owner, and the API MUST NOT serve a schema older than it needs — **holds** (F30).
10. MUST keep ids ordered by ledger order and MUST NOT depend on their contiguity — **holds** (F12, A49).
11. ~~MUST NOT commit an import that does not verify against its source, and MUST remove the importer after the cutover~~ — **met and closed** (F31): the importer verified before it committed, and it was removed in 0.0.15.

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
- **S8** SHOULD add `events_kind (stream, kind, id)` with a page-cost test for a rare kind (F25, OQ5) — done in 0.0.13.
- **S9** MAY run the client-contract job against the image too (today it runs against the binary built on
  the runner; the smoke test covers the image's processes).

- **S10** MAY split the events statement into a facts statement and a page statement run in a pipelined transaction to save planning time (about a third of the
  per-request cost, F25) — not taken: it adds a transaction and a cancellation hazard to every request for a gain the edge cache makes immaterial; revisit if the origin ever
  serves uncached traffic at scale.
- **S11** SHOULD enable the tier's `harden` lines and the TLS-only `pg_hba` (workspace, F27/F32 residuals) and move the Ory stack to `verify-full`. 2026-10-20: the `harden` lines and the `verify-full` move are authored (applied: see log);
  the TLS-only `pg_hba` component stays an operator choice (it restarts PostgreSQL).

## Open questions (`OQ#`)

The four first-pass questions and OQ5-OQ7 are decided (OQ6 was superseded by OQ7 on the same day).

1. **OQ1** F3 — decided: a fresh publish clears the old package's rows after a `VACUUM INTO` backup;
   history is not kept queryable per package; deletion requires `REINDEX_CONFIRM` naming the stream.
2. **OQ2** F2 — decided: an undecodable event is counted and skipped (neither stored nor quarantined).
3. **OQ3** F7 — decided: this project republishes before v0.2 instead of adding event types by upgrade;
   revisit if mainnet packages are ever upgraded to add events.
4. **OQ4** — answered 2026-10-09: `/healthz` is wired to the status page ("Chain Access → Chain index",
   `OPERATOR_TASKS.md`); there is **no** volume snapshot or backup policy for `sui-indexer-data` (F3),
   and no `OPERATOR_TASKS.md` row for it yet — a maintainer item.
5. **OQ5** F25 — decided 2026-10-19: (b), in the patch after the 0.0.12 cutover (0.0.13), so the cutover stayed a
   pure language change. The index is additive and rollback-safe, and the ingester creates it at startup
   (about 0.5 s on 300,000 rows, readers unaffected). Options (a) (in the cutover) and (c) (not at all, relying on
   the edge cache and the limiter; the cost grows with history) were not taken.
6. **OQ6** storage layer — decided 2026-10-20 (workspace D59, first text) and **superseded the same day by OQ7**: the index stays in the indexer's own embedded SQLite on its
   PVC and is not moved into the shared PostgreSQL of the `database` namespace. That PostgreSQL is a single replica for the
   Ory auth stack (ingress only from `auth`, only deployed with `ENABLE_AUTH=true`, same single-replica Longhorn storage,
   no backup), so it would add no durability, couple the chain index to the auth stack, need a role/TLS/secret and a rewrite
   of `crates/store`, and put a network hop in front of a display-only cache. The gap is backup (F3, `OPERATOR_TASKS.md`),
   not the engine. Revisit for 2+ API replicas on the 3-node cluster or a shared HA PostgreSQL with PITR; the swap is
   contained in `crates/store`. *(Superseded by OQ7: that conclusion weighed the move only against durability and the auth stack.)*
7. **OQ7** storage layer — decided 2026-10-20 (workspace D59 revised, D60): **the index is stored in the shared PostgreSQL tier**, in its own database
   `sui_indexer` with an owner role (the ingester) and a SELECT-only role (the API). Reasons: least privilege enforced by the database instead of a mount flag
   and an open mode (F22, F27); one backup, upgrade and restore story for all relational data (F3); independently schedulable pods and a stateless,
   replicable API instead of a two-container pod around one RWO file; the cheapest moment for the rewrite of `crates/store` (17 testnet rows, a contained change);
   the tier is a platform tier deployed independently of `ENABLE_AUTH`. Costs, accepted: shared fate with the auth PostgreSQL (F28), a network hop (4-5× lower origin
   throughput per replica, immaterial behind the edge cache: F25), a one-off import (F31, done and removed in 0.0.15). Not claimed: durability — one node, MinIO on the same disk; only the offsite
   copy is a backup (F3, Section D).

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
- **Cutover (0.0.14):** done and settled (Section D); the legacy volume was the rollback and has been deleted on the maintainer's instruction, so there is no way back to SQLite (F31).
- **Shared fate (F28):** an outage of the PostgreSQL tier is an outage of the index (display data; clients fall back). Durability rests on a dump on the same disk until the
  offsite copy exists (F3).
- **Cutover (0.0.12, superseded):** the manifests must not be applied before the 0.0.12 digest is pinned; the
  `.bak-pre-rust-*` copy and the 0.0.11 digest are the rollback (workspace README). Rare-kind filters walked the
  stream in 0.0.12 (F25); 0.0.13 adds the index.

---

## Re-verification log

- 2026-10-20 — `0.0.15`: the one-off SQLite importer and `rusqlite` removed (local commit on top of `eb8e46a`; not released). No behaviour change for the API.
  - **Scope:** every file named in the Scope block; the repository was searched for `sqlite|import-sqlite|rusqlite|indexer.db|DB_PATH|VACUUM|WAL` and the workspace for `sqlite|indexer.db|import-sqlite|sui-indexer-data`.
  - **Re-verified:** F31 → RESOLVED (evidence in the finding); F18 (SBOM gate: no feature option, 116 / 175 crates, 11 cases) and F14 (Dockerfile in one build step) carry a note; F29 and F30 lose the importer wording; A16, A37 reworded; A41, A42, A48 N/A; B.1 loses the `rusqlite` row;
    B.IMG-2's "Cutover" row becomes "First apply"; C.1, the test layers and C.2 updated; Section D: the 0.0.14 cutover and the 0.0.15 removal rows ticked; normative requirement 11, OQ7's cost list and the Risks cutover bullet closed.
  - **Found:** nothing new. `0001_init.sql` keeps one comment naming the SQLite store (a released migration is immutable: F30).
  - **Not carried over:** the import-parity/golden test (its oracle was the SQLite API; the handler and client-contract tests remain the wire-contract pins).
  - **Measured:** 144 Rust tests (store 65, ingest 36, api 43); `fmt`, `clippy -D warnings`, `cargo deny check` pass; MSRV 1.88: passes; 144 passed (store 65, ingest 36, api 43); MSRV 1.88 unchanged; SBOMs 116 / 175 crates, 11 + 9 gate cases; release binaries: release binaries: `api` 2,862,848 bytes (0.0.14: 2,847,120), `ingest` 4,327,128 bytes (0.0.14 with `rusqlite`: 6,414,464; -2.09 MB, -32.5%); built per binary as the Dockerfile does.
  - **Totals F1-F32:** 17 RESOLVED, 2 MITIGATED (F2, F3), 3 ADJUDICATED, 1 ACCEPTED-RISK (F28), 1 DEFERRED (F19), 8 Positive (F12, F13, F14, F22, F27, F29, F30, F32).
  - **Not run:** the GitHub workflows; the image build and smoke test (no container build daemon here); the client-contract job; the cluster.

- 2026-10-20 — workspace-side database-tier hardening (no change in this repository's code).
  - **Authored and then applied on the cluster the same day:** `harden` of `kratos_db`, `hydra_db`, `keto_db` (closes the F27 residual
    for the auth databases), Ory `sslmode=verify-full` (closes the F32 residual), a dedicated `postgres_backup` login for the backup job and a
    bucket-scoped MinIO user for its uploads. This repository's roles (`sui_indexer_ingest`, `sui_indexer_api`) are unaffected except that they can no longer
    open the auth databases, and the backup role now reads `sui_indexer` instead of the superuser. Local proofs: `k8s/base/database/tests/`.

- 2026-10-20 — storage moved to the shared PostgreSQL tier in `0.0.14` (local commits on top of `c0ab2fa`; not released, not deployed; D59 revised, D60; OQ7).
  - **Scope:** the Rust workspace re-verified against the new storage (Scope, 2026-10-20 block); the workspace manifests and the cutover runbook
    (`post-bootstrap/sui-indexer/{base,migration}`); the database tier's provisioning and backup documentation (read-only).
  - **Re-verified:** F1-F26 (a note on F1, F2, F3, F10, F12, F13, F14, F18, F22, F23, F24, F25, F26; the others are unaffected). SQLite-specific findings:
    F1's backfill and F3's `VACUUM INTO` / `bak-pre-rust` cutover copy are N/A or replaced (a `bak_<ms>` schema copy, atomic with the clearing; no backfill);
    F22's three SQLite layers are replaced by the SELECT-only role and the persisted-WAL `unsafe` call is gone (no `unsafe` code left: A16); F3's volume loss becomes
    database-backup coverage, with what exists and what is operator work recorded (offsite copy not configured). No disposition regressed.
  - **New:** F27 (role separation, Positive), F28 (shared fate with the auth PostgreSQL, ACCEPTED-RISK), F29 (single writer by advisory lock, Positive), F30 (migration
    safety, Positive), F31 (the temporary SQLite import path, MITIGATED), F32 (TLS `verify-full`, no secret in logs, Positive). OQ7 decided (OQ6 superseded). A44-A50 added; A3,
    A4, A6, A13, A16, A21, A34, A35, A37-A42 rewritten; B.IMG-2 rewritten; C.1 and the test layers updated; Section D gained the 0.0.14 cutover, the 0.0.15 importer
    removal and the backup/offsite rows (the volume-snapshot row is superseded).
  - **Found while testing (fixed):** the generic-plan hazard of the shared one-kind statement (pooled connections force custom plans, F25); the importer's `setval` parameter typed as `integer` by PostgreSQL (caught by the importer tests; now cast), `CREATE DATABASE` refused inside the harness's multi-statement
    batch, and a smoke-test step that tested the read-only default instead of the grants (now switches the default off first).
  - **Measured (2026-10-20):** 144 Rust tests (154 with `--all-features`), lines 89.4%; `fmt`, `clippy -D warnings` (both feature sets), `cargo +1.88 check`, `cargo deny check`, the
    12 SBOM-gate cases and the SBOMs (116 / 182 crates) pass; 5/5 client-contract tests (Node 26); podman image 39 MB and the smoke test pass; golden parity 53/53 routes byte-identical to a real
    0.0.13 `api`; live parity 97/97 routes against the production instance's data; load figures in F25 (4-5× below the in-process SQLite build, immaterial behind the edge cache).
  - **Totals F1-F32:** 16 RESOLVED, 3 MITIGATED (F2, F3, F31), 3 ADJUDICATED, 1 ACCEPTED-RISK (F28), 1 DEFERRED (F19), 8 Positive (F12, F13, F14, F22, F27, F29, F30, F32).
  - **Not run:** the GitHub workflows; `kubectl` against the cluster (nothing applied, the cutover runbook is untested on the PVC); the database tier on the cluster (its own "not tested" list stands);
    the hostname-mismatch refusal of rustls with a dedicated certificate.
- 2026-10-20 — OQ6 decided: the indexer keeps its embedded SQLite rather than the shared `database` namespace (workspace D59, first text; **superseded the same day by OQ7**, below).

- 2026-10-19 — F25 closed in `0.0.13` (local commit on top of the 0.0.12 commits; not released, not deployed).
  - **Fixed:** F25 → RESOLVED: `events_kind (stream, kind, id)` (ingester-created, additive) and a page query
    with one seek per kind; OQ5 decided (b). A43 now HOLDS; S8 done; Section D row ticked.
  - **Evidence:** query plans for every filter shape (before/after table on F25), page cost on 300,000 rows
    (under 5,000 VM steps against over 100,000 without the index), migration of an older database,
    200-query equivalence against the plain filter; measured 8 → ≈ 33,000 pages/s, index build ≈ 0.5 s with
    readers unaffected.
  - **Measured (2026-10-19, 0.0.13):** 111 Rust tests (store 36) + 1 ignored measurement, lines 90.8%;
    `cargo fmt --check`, `clippy --all-targets --locked -D warnings`, `cargo +1.88 check`, `cargo deny check`
    and the 10 SBOM-gate cases pass; 5/5 client-contract tests against the built `api` (Node 26).
  - **Totals F1–F26:** 16 RESOLVED, 2 MITIGATED, 3 ADJUDICATED, 0 ACCEPTED-RISK, 1 DEFERRED (F19), 4 Positive.
  - **Not run:** the GitHub workflows; the rollout (the new image digest must be pinned first).
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
- 2026-10-20 — release workflow restructured (D61; commit 417b5ff, not released): `docker-publish.yml` builds amd64 and arm64 on native runners, merges the per-architecture indexes, scans by digest, then signs and attests the merged digest; `mirror-private`, the per-binary CycloneDX SBOMs, the syft SPDX SBOM, build provenance on both registries, the version check and the `rust-ci.yml` gate are kept; a manual run is a dry run. No code, image or version change (CI-only; `CHANGELOG.md` `[Unreleased]`). Evidence and the list of what only the next tag can test: F10, *Re-verified 2026-10-20 (release workflow)*. No finding changes disposition.

## Pre-save consistency checklist (this pass)

- [x] Section A ↔ findings: A16–A24 are RUST rows (2026-10-19); A38–A42 HOLD (F22–F24, cutover); A43 does not
  hold in 0.0.12 and holds since 0.0.13 (F25, RESOLVED, OQ5 decided); A6 caveat (F3, backup on the same volume); A1 caveat (F19).
- [x] Finding header ↔ body: consistent (F1–F26; F11 header moved to RESOLVED with its note).
- [x] Template line: base + RUST + SUI_CLIENT + IMG with registry dates; untriggered lenses named, TS's
  remaining scope (CI-only contract project) stated.
- [x] Closing four-part structure present (normative, suggestions, open questions, risks).
- [x] Section D ↔ dispositions: every unticked item names a gate or a maintainer item (the 0.0.12 rollout,
  the F24 re-run, F19, mainnet, volume backup, edge rule, external review).
- [x] Executive summary ↔ dispositions and ceiling (Medium; realised Low); totals F1–F26 checked
  (2026-10-19, 0.0.13: 16 / 2 / 3 / 0 / 1 / 4).
- [x] C.1 counts measured 2026-10-19 (111 Rust tests on 0.0.13, 5 contract, 10 SBOM-gate cases).
- [x] Re-verification log entry added (2026-10-09, 2026-10-18, 2026-10-19).
- [x] 2026-10-20 (0.0.14): Section A ↔ findings: A44-A50 HOLD (F27, F29, F30, F32, F31, F12, F22); rewritten rows A3, A4, A6, A13, A16, A21, A34, A35, A37-A42 cite the PostgreSQL mechanism.
- [x] 2026-10-20: finding header ↔ body: F1-F32 consistent (F3's header updated; F28 ACCEPTED-RISK, F31 MITIGATED carry their mitigations and the removal gate).
- [x] 2026-10-20: Section D ↔ dispositions: the new unticked rows (the 0.0.14 cutover, the importer removal, the offsite backup) name a maintainer item or a release; counts in the executive summary,
  the findings table and the log agree (16 / 3 / 3 / 1 / 1 / 8 for F1-F32).
- [x] 2026-10-20: C.1 counts measured (144 / 154 Rust tests, 5 contract, 12 SBOM-gate cases, coverage 89.4% lines); load figures labelled as a floor on a shared laptop.
- [x] 2026-10-20: re-verification log entry added.
- [x] 2026-10-20 (release workflow, D61): B.2 rows and the IMG supply-chain table updated; re-verification log entry added; no finding counts change.
- [x] 2026-10-20 (0.0.15): finding header ↔ body: F31 RESOLVED in the header, the table and the body; Section A rows A41, A42, A48 N/A; Section D ↔ dispositions: the 0.0.14 cutover and the importer-removal rows ticked with evidence, no new unticked row; counts in the Status, the Executive summary and the findings table agree (17 / 2 / 3 / 1 / 1 / 8 for F1-F32).
- [x] 2026-10-20 (0.0.15): C.1 counts measured (144 Rust tests, 11 SBOM-gate cases); re-verification log entry added.
