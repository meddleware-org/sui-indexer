# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
