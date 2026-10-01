# AGENTS.md — sui-indexer

Read [CLAUDE.md](CLAUDE.md) first; its invariants are binding.

| Task | Command |
| --- | --- |
| Type-check | `npm run type-check` |
| Lint | `npm run lint` |
| Tests | `npm test` |
| Bundle | `npm run build` (→ `dist/main.mjs`) |
| Run locally | `DB_PATH=./dev.db npm start` |
| Image | `podman build -t sui-indexer .` |

## Layout

```text
src/config.ts     environment → Config (package ids default to the client deployments)
src/store.ts      SQLite schema, atomic page+cursor ingest, pages, coverage, stats
src/ingest.ts     ascending poller per stream, decoding with the client parsers
src/api.ts        request → response (routing, validation, cache headers, rate limit)
src/ratelimit.ts  per-IP token buckets
src/main.ts       wiring: node:http server, poll loop, graceful shutdown
tests/            vitest; fixtures encode events independently of the parsers
```

Release: bump `version`, push, tag `v<version>`. `docker-publish.yml` checks tag == version, then
builds, signs and attests the image.
