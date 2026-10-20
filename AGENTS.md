# AGENTS.md — sui-indexer

Read [CLAUDE.md](CLAUDE.md) first; its invariants are binding.

| Task | Command |
| --- | --- |
| Format | `cargo fmt --check` |
| Lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` (and with `--all-features`) |
| Tests | `. <(scripts/test-postgres.sh start …)` then `cargo test --workspace --locked` (needs `TEST_DATABASE_URL`; `--all-features` adds the temporary importer) |
| Coverage | `cargo llvm-cov --workspace --locked --all-features --summary-only` |
| Supply chain | `cargo deny check`; `bash scripts/tests/check-sbom-test.sh` (the ingester's SBOM is checked with `CARGO_FEATURES=sui-indexer-ingest/import-sqlite`) |
| Build | `cargo build --release --locked` (→ `target/release/{ingest,api}`; the image also builds `ingest` with `--features import-sqlite` in 0.0.14) |
| Run locally | `DATABASE_URL=… ./target/release/ingest` (owner role) and `DATABASE_URL=… ./target/release/api` (SELECT-only role); `ALLOW_INSECURE_DATABASE=1` only for a plaintext local database |
| Client contract | `cargo build --release -p sui-indexer-api -p sui-indexer-testkit && (cd tests/contract && npm ci && TEST_DATABASE_URL=… npm test)` |
| Image | `podman build -t sui-indexer .` then `DOCKER=podman bash scripts/smoke.sh sui-indexer` (starts its own PostgreSQL) |

## Layout

```text
crates/store     PostgreSQL: the ONLY crate with SQL. db.rs (DATABASE_URL, verify-full TLS), migrate.rs +
                 migrations/ (versioned schema), writer.rs (Store: the owner connection holding the writer lock),
                 reader.rs (the API's SELECT-only pool; one statement per request), read.rs (shared queries,
                 page_sql/events_sql), import.rs (feature import-sqlite, temporary), testdb.rs (per-test
                 databases, feature testing), config.rs (env -> config, recorded package deployments), sui.rs, time.rs
crates/ingest    the writer: source.rs (sui-rpc ListEvents/GetServiceInfo, page assembly from frames),
                 decode.rs (kind from the type, gate/commission from the node's JSON; no BCS decoding),
                 ingestor.rs (poll loop, gaps, heartbeats), chain.rs (chain ids), lib.rs (startup order,
                 loop, --healthcheck), import_cmd.rs (feature import-sqlite), main.rs
crates/api       the read API: handler.rs (routing, validation, cache headers), query.rs (URL parsing and
                 canonical forms), ratelimit.rs (per-IP buckets), server.rs (hyper, timeouts, shutdown), main.rs
crates/testkit   test fixtures (BCS written independently of the client parsers), `seed-fixture`
                 (provision + seed a throwaway database for the contract job), the 0.0.13 SQLite fixture (feature)
tests/contract   CI only: the published client packages against the built `api` binary
tests/golden     the recorded answers of a real 0.0.13 API (importer parity; removed with the importer in 0.0.15)
scripts/         smoke.sh (image + PostgreSQL), test-postgres.sh (pinned TLS PostgreSQL for tests), check-sbom.sh
                 (+ tests/) for the release SBOM gate, golden-from-0.0.13.sh
```

Release: bump `version` in `[workspace.package]` of `Cargo.toml` (and `Cargo.lock`), add a CHANGELOG entry,
push, tag `v<version>`. `docker-publish.yml` checks tag == version, then runs the full CI and builds, signs
and attests the image.
