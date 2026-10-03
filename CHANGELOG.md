# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
