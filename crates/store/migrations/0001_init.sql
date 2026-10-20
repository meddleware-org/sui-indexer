-- sui-indexer schema, version 1. Applied by the ingester (the owner role) at startup, under an advisory
-- lock, in one transaction (see crates/store/src/migrate.rs). The API role cannot run DDL.
--
-- The logical schema of the SQLite store of 0.0.4-0.0.13, with proper column types and constraints in
-- place of SQLite's STRICT tables: bytea for BCS, bigint for every integer, NOT NULL and CHECK where
-- the code relies on them. The `events.id` identity is the ledger order (single writer, see the
-- writer lock): the invariant is ORDER, not gaplessness (a rolled-back insert or an ignored duplicate
-- consumes a number; nothing reads ids as a count).

CREATE TABLE meta (
  key   text PRIMARY KEY,
  value text NOT NULL
);

CREATE TABLE cursors (
  stream text PRIMARY KEY CHECK (stream IN ('access-gate', 'sealed-content')),
  cursor text
);

CREATE TABLE events (
  id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  stream          text   NOT NULL CHECK (stream IN ('access-gate', 'sealed-content')),
  tx_digest       text   NOT NULL,
  event_index     bigint NOT NULL CHECK (event_index >= 0),
  checkpoint      bigint CHECK (checkpoint >= 0),
  sender          text   NOT NULL,
  event_type      text   NOT NULL,
  bcs             bytea  NOT NULL,
  kind            text   NOT NULL,
  gate_id         text,
  commission_mist bigint CHECK (commission_mist >= 0),
  UNIQUE (tx_digest, event_index)
);
CREATE INDEX events_stream ON events (stream, id);
CREATE INDEX events_gate ON events (stream, gate_id, id);
-- A `kinds` filter for a rare kind reads that kind's rows, not the stream (F25).
CREATE INDEX events_kind ON events (stream, kind, id);

-- Counters maintained inside the ingest transaction, so serving a page never scans a stream.
CREATE TABLE stream_stats (
  stream            text PRIMARY KEY CHECK (stream IN ('access-gate', 'sealed-content')),
  from_checkpoint   bigint,
  latest_checkpoint bigint,
  events            bigint NOT NULL DEFAULT 0 CHECK (events >= 0),
  undecodable       bigint NOT NULL DEFAULT 0 CHECK (undecodable >= 0),
  gaps              bigint NOT NULL DEFAULT 0 CHECK (gaps >= 0)
);

CREATE TABLE gate_stats (
  gate_id text   NOT NULL,
  kind    text   NOT NULL,
  n       bigint NOT NULL CHECK (n >= 0),
  PRIMARY KEY (gate_id, kind)
);

-- A sum of u64 commissions can exceed 64 bits: numeric(39,0) holds a u128 exactly.
CREATE TABLE gate_commission (
  gate_id text PRIMARY KEY,
  total   numeric(39, 0) NOT NULL CHECK (total >= 0)
);

-- The ingester's liveness and per-stream poll heartbeats (`poll:<stream>`, `ingest:alive`), read by the
-- API's /healthz and the exec liveness probe. Rewritten many times a minute, so they live apart from
-- `meta` (a small hot table) with room for heap-only updates.
CREATE TABLE heartbeats (
  key   text PRIMARY KEY,
  value text NOT NULL
) WITH (fillfactor = 50, autovacuum_vacuum_scale_factor = 0, autovacuum_vacuum_threshold = 50);
