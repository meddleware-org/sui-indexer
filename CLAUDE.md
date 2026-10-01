# CLAUDE.md — sui-indexer

A read-indexer for display data. See [README.md](README.md) for the API and configuration.

## Invariants

- **Display-only.**
  - No endpoint may be used, or documented, as an authorisation source.
  - Rows are the full node's events verbatim, and the clients decode and type-check them again.
  - Clients fall back to the full node.
- **One parser.** Events are decoded with `@meddleware/access-gate-client` and
  `@meddleware/seal-client` only. Do not copy their BCS layouts here.
- **Ledger order = row id.**
  - Ingest reads each stream ascending, and a page and its cursor commit in one transaction.
  - That is what makes a page bounded by `before` immutable (cached for a year).
  - Never insert rows out of order: no parallel backfill, and no descending reads into the table.
- **Generation.** A new database gets a new random generation. Cursors carry it and are refused
  (`410`) across generations, so cached pages of an old database stay self-consistent.
- **Cache headers are the cost model.**
  - Older pages: `public, max-age=31536000, immutable`.
  - The newest page: `public, max-age=10, s-maxage=30`, with an ETag.
  - Keep `Access-Control-Allow-Origin: *` (public read-only data). Cloudflare ignores `Vary`, so
    a reflected origin would be cached for the wrong site.
- **Strict input.** Unknown query parameters, bad ids and limits get `400`. That also stops
  cache-busting parameters from reaching the origin as new cache keys.
- **Lean.**
  - Node 24 built-ins (`node:sqlite`, `node:http`) plus the client packages, as one esbuild bundle
    on distroless nonroot.
  - Do not add a web framework, an ORM or a second process.
- **Single writer.** One replica with an RWO volume. SQLite WAL allows concurrent reads within the
  process.

## Tests

`npm test` runs:

- **ingest** — against a fake full node, with independently encoded events;
- **API** — the handler without a socket;
- **client contract** — `listAccessGateEvents` and `listSealedContent` through the handler;
- **config and rate-limit.**

Before a release, run the bundle locally against testnet (README → Development).
