# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.6] - 2026-10-08

### Changed

- Fix: startup compared the node's base58 chain identifier with the short id and refused testnet; it now maps the digest to its first four bytes (0.0.5 never became ready)

## [0.0.5] - 2026-10-08

### Fixed

- **The indexer no longer wedges when its cursor falls below the full node's retention.** A stream
  whose cursor stopped advancing (no events for days) retried the same pruned cursor forever
  ("requested data below earliest available") and `/healthz` stayed red. It now restarts from the
  node's earliest data and counts a `gap`. Streams are polled independently, so one failing no longer
  stops the other.
- An event that does not decode no longer halts ingest: it is counted (`undecodable`), logged loudly
  and skipped.

### Changed

- Coverage, gate stats and commission come from counters kept in the ingest transaction instead of
  full-table scans (a coverage read was ~150 ms at 300k rows on the single request thread). Older
  databases are rebuilt once at startup.
- A package change no longer deletes history on start: it stops startup until `REINDEX_CONFIRM` names the
  stream, backs the database up first (`VACUUM INTO`) and then clears it.
- `/healthz` reports each stream (`ok`, `lastPollAgeMs`, `lastError`); head pages carry `latestCheckpoint`
  and `polledAt`; `/coverage` adds `undecodable` and `gaps`.
- Non-canonical spellings of a query get a cacheable `301` to the canonical form (one cache entry per
  query); IPv6 clients share a rate-limit bucket per /64; the bucket map is capped; weak, listed and
  wildcard `If-None-Match` revalidate.
- Startup refuses a full node whose chain identifier does not match `NETWORK`; the node client is no
  longer cast to the slice the indexer uses, so tsc checks it.
- The image ships `package.json` + `package-lock.json` (so SBOM scanners see the bundled npm packages)
  and `THIRD_PARTY_LICENSES`; CI checks the notices.
- `@meddleware/seal-client` ^0.0.16, `@meddleware/access-gate-client` ^0.0.6.

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
