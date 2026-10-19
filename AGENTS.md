# AGENTS.md — sui-indexer

Read [CLAUDE.md](CLAUDE.md) first; its invariants are binding.

| Task | Command |
| --- | --- |
| Format | `cargo fmt --check` |
| Lint | `cargo clippy --all-targets --locked -- -D warnings` |
| Tests | `cargo test --locked` |
| Coverage | `cargo llvm-cov --locked --summary-only` |
| Supply chain | `cargo deny check`; `bash scripts/tests/check-sbom-test.sh` |
| Build | `cargo build --release --locked` (→ `target/release/{ingest,api}`) |
| Run locally | `DB_PATH=./dev.db ./target/release/ingest` and `DB_PATH=./dev.db ./target/release/api` |
| Client contract | `cargo build --release -p sui-indexer-api -p sui-indexer-testkit && (cd tests/contract && npm ci && npm test)` |
| Image | `podman build -t sui-indexer .` then `DOCKER=podman bash scripts/smoke.sh sui-indexer` |

## Layout

```text
crates/store     SQLite schema, the writer (Store) and the read-only view (Reader); config.rs (env → config,
                 recorded package deployments), sui.rs (address/type normalisation), time.rs
crates/ingest    the writer: source.rs (sui-rpc ListEvents/GetServiceInfo, page assembly from frames),
                 decode.rs (kind from the type, gate/commission from the node's JSON; no BCS decoding),
                 ingestor.rs (poll loop, gaps, heartbeats), chain.rs (chain ids), lib.rs (startup order,
                 loop, --healthcheck), main.rs
crates/api       the read API: handler.rs (routing, validation, cache headers), query.rs (URL parsing and
                 canonical forms), ratelimit.rs (per-IP buckets), server.rs (hyper, timeouts, shutdown), main.rs
crates/testkit   test fixtures (BCS written independently of the client parsers) and `seed-fixture`
tests/contract   CI only: the published client packages against the built `api` binary
scripts/         smoke.sh (image), check-sbom.sh (+ tests/) for the release SBOM gate
```

Release: bump `version` in `[workspace.package]` of `Cargo.toml` (and `Cargo.lock`), add a CHANGELOG entry,
push, tag `v<version>`. `docker-publish.yml` checks tag == version, then runs the full CI and builds, signs
and attests the image.
