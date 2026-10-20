//! **Temporary.** The one-off import of a 0.0.13 SQLite index into PostgreSQL (`ingest import-sqlite`),
//! compiled only with the `import-sqlite` feature, which exists in the 0.0.14 image and is removed in
//! 0.0.15 together with `rusqlite`.
//!
//! What it does, in ONE PostgreSQL transaction on the ingester's writer connection (so it holds the writer
//! lock and cannot overlap a running ingester):
//!
//! 1. reads the SQLite file strictly read-only, in one read transaction (a consistent snapshot, WAL
//!    included);
//! 2. refuses a target that already holds index data (`--replace-existing` first copies it to a
//!    `bak_<ms>` schema, then empties it);
//! 3. copies `meta` (generation, package bindings, gap notes; `schema_writer` becomes `postgres`),
//!    `cursors` (the same base64 strings), `events` (**with their ids**: cursors served before the cutover
//!    are `<generation>.<id>` and must keep meaning the same row, and cached `ETag`s are
//!    `"<generation>-<head id>"`), and the counters `stream_stats`, `gate_stats`, `gate_commission`; then
//!    moves the identity sequence past the largest id;
//! 4. **verifies before it commits**: row counts per stream, every stream cursor, the commission total,
//!    the maximum id, a SHA-256 over `id:tx_digest:event_index` in id order (computed on both sides from
//!    the data itself), and that the copied counters equal a recount of the copied rows. Any difference
//!    rolls everything back.
//!
//! Heartbeats (`poll:*`, `ingest:alive`) and `stats:v2` are not copied: the ingester writes fresh
//! heartbeats on its first poll, and the PostgreSQL counters need no backfill.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// The SQLite binding, re-exported so the importer's tests can read and damage fixtures.
pub use rusqlite;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use tokio_postgres::types::ToSql;

use crate::error::{Result, StoreError};
use crate::writer::{Store, backup_into, verify_counters};
use crate::{SCHEMA_WRITER_KEY, Stream};

/// How the import treats a target that already holds data.
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// Copy the existing index into the schema `backup_schema`, empty it, and import anyway.
    pub replace_existing: bool,
    /// The backup schema name for `replace_existing` (`bak_<epoch ms>`).
    pub backup_schema: String,
}

/// What was imported and checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportReport {
    pub generation: String,
    pub events: BTreeMap<String, i64>,
    pub cursors: BTreeMap<String, String>,
    pub max_id: i64,
    pub commission_total: u128,
    pub order_hash: String,
    pub meta_keys: Vec<String>,
    pub skipped_meta_keys: Vec<String>,
    pub gate_stats_rows: usize,
    pub gate_commission_rows: usize,
    /// The schema the previous contents were copied to, with `replace_existing`.
    pub backup: Option<String>,
}

impl fmt::Display for ImportReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "import-sqlite: verified and committed")?;
        writeln!(f, "  generation        {}", self.generation)?;
        for (s, n) in &self.events {
            writeln!(f, "  events {s:<15} {n}")?;
        }
        for (s, c) in &self.cursors {
            writeln!(f, "  cursor {s:<15} {c}")?;
        }
        writeln!(f, "  max event id      {}", self.max_id)?;
        writeln!(f, "  commission total  {} mist ({} gates)", self.commission_total, self.gate_commission_rows)?;
        writeln!(f, "  gate_stats rows   {}", self.gate_stats_rows)?;
        writeln!(f, "  order hash        {} (sha256 over id:tx_digest:event_index, id order)", self.order_hash)?;
        writeln!(f, "  meta copied       {}", self.meta_keys.join(", "))?;
        if !self.skipped_meta_keys.is_empty() {
            writeln!(f, "  meta not copied   {}", self.skipped_meta_keys.join(", "))?;
        }
        if let Some(b) = &self.backup {
            writeln!(f, "  previous contents copied to schema {b}")?;
        }
        Ok(())
    }
}

struct EventRow {
    id: i64,
    stream: String,
    tx_digest: String,
    event_index: i64,
    checkpoint: Option<i64>,
    sender: String,
    event_type: String,
    bcs: Vec<u8>,
    kind: String,
    gate_id: Option<String>,
    commission_mist: Option<i64>,
}

/// `(stream, from_checkpoint, latest_checkpoint, events, undecodable, gaps)`.
type StreamStatsRow = (String, Option<i64>, Option<i64>, i64, i64, i64);

/// Everything read from the SQLite file.
struct Source {
    meta: Vec<(String, String)>,
    cursors: Vec<(String, Option<String>)>,
    events: Vec<EventRow>,
    stream_stats: Vec<StreamStatsRow>,
    gate_stats: Vec<(String, String, i64)>,
    gate_commission: Vec<(String, String)>,
}

fn sqlite_err(e: rusqlite::Error) -> StoreError {
    StoreError::Data(format!("reading the SQLite file failed: {e}"))
}

fn read_source(path: &Path) -> Result<Source> {
    if !path.is_file() {
        return Err(StoreError::Data(format!("{} is not a file", path.display())));
    }
    // Read-only, never creating or checkpointing anything. WAL content is read through the -wal file.
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(sqlite_err)?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000; BEGIN").map_err(sqlite_err)?;
    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(sqlite_err)?;
    if check != "ok" {
        return Err(StoreError::Data(format!("the SQLite file failed its integrity check: {check}")));
    }
    let meta = conn
        .prepare("SELECT key, value FROM meta ORDER BY key")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(sqlite_err)?;
    let cursors = conn
        .prepare("SELECT stream, cursor FROM cursors ORDER BY stream")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(sqlite_err)?;
    let events = conn
        .prepare(
            "SELECT id, stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind, gate_id, commission_mist
             FROM events ORDER BY id",
        )
        .and_then(|mut s| {
            s.query_map([], |r| {
                Ok(EventRow {
                    id: r.get(0)?,
                    stream: r.get(1)?,
                    tx_digest: r.get(2)?,
                    event_index: r.get(3)?,
                    checkpoint: r.get(4)?,
                    sender: r.get(5)?,
                    event_type: r.get(6)?,
                    bcs: r.get(7)?,
                    kind: r.get(8)?,
                    gate_id: r.get(9)?,
                    commission_mist: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(sqlite_err)?;
    let stream_stats = conn
        .prepare(
            "SELECT stream, from_checkpoint, latest_checkpoint, events, undecodable, gaps FROM stream_stats ORDER BY stream",
        )
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(sqlite_err)?;
    let gate_stats = conn
        .prepare("SELECT gate_id, kind, n FROM gate_stats ORDER BY gate_id, kind")
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(sqlite_err)?;
    let gate_commission = conn
        .prepare("SELECT gate_id, total FROM gate_commission ORDER BY gate_id")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(sqlite_err)?;
    conn.execute_batch("COMMIT").map_err(sqlite_err)?;
    Ok(Source { meta, cursors, events, stream_stats, gate_stats, gate_commission })
}

/// SHA-256 over `id:tx_digest:event_index\n` for rows in id order.
fn order_hash<'a>(rows: impl Iterator<Item = (i64, &'a str, i64)>) -> String {
    let mut h = Sha256::new();
    for (id, digest, index) in rows {
        h.update(format!("{id}:{digest}:{index}\n").as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn is_copied_meta(key: &str) -> bool {
    key == "generation" || key.starts_with("package:") || key.starts_with("gap:")
}

/// Import the SQLite index at `path` into the store's (empty) database. See the module docs.
pub async fn import_sqlite(store: &mut Store, path: &Path, opts: &ImportOptions) -> Result<ImportReport> {
    let src = read_source(path)?;
    let generation = src.meta.iter().find(|(k, _)| k == "generation").map(|(_, v)| v.clone()).ok_or_else(|| {
        StoreError::Data("the SQLite file has no generation: it is not an index of this service".into())
    })?;
    if !(generation.len() == 12 && generation.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) {
        return Err(StoreError::Data(format!("unexpected generation {generation:?} in the SQLite file")));
    }
    for e in &src.events {
        if Stream::parse(&e.stream).is_none() {
            return Err(StoreError::Data(format!("event {} is in the unknown stream {:?}", e.id, e.stream)));
        }
    }
    let want_hash = order_hash(src.events.iter().map(|e| (e.id, e.tx_digest.as_str(), e.event_index)));
    let want_events: BTreeMap<String, i64> = src.events.iter().fold(BTreeMap::new(), |mut m, e| {
        *m.entry(e.stream.clone()).or_insert(0) += 1;
        m
    });
    let want_cursors: BTreeMap<String, String> =
        src.cursors.iter().filter_map(|(s, c)| c.clone().map(|c| (s.clone(), c))).collect();
    let want_max_id = src.events.iter().map(|e| e.id).max().unwrap_or(0);
    let want_commission: u128 = src
        .gate_commission
        .iter()
        .map(|(_, t)| {
            t.parse::<u128>().map_err(|_| StoreError::Data(format!("a commission total {t:?} is not an integer")))
        })
        .sum::<Result<u128>>()?;

    let tx = store.client_mut().transaction().await?;

    // 1. The target must be empty (or be explicitly replaced, after a copy).
    let held = tx
        .query_one(
            "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM cursors), (SELECT count(*) FROM stream_stats),
                    (SELECT count(*) FROM gate_stats), (SELECT count(*) FROM gate_commission)",
            &[],
        )
        .await?;
    let held: Vec<i64> = (0..5).map(|i| held.get(i)).collect();
    let mut backup = None;
    if held.iter().any(|n| *n > 0) {
        if !opts.replace_existing {
            return Err(StoreError::Data(format!(
                "the target database already holds index data (events {}, cursors {}, stream_stats {}, gate_stats {}, \
                 gate_commission {}): refusing to import over it. Importing is a first-start step; if this data is \
                 disposable, run again with --replace-existing (it copies the current contents to a bak_<ms> schema first)",
                held[0], held[1], held[2], held[3], held[4]
            )));
        }
        backup_into(&tx, &opts.backup_schema).await?;
        backup = Some(opts.backup_schema.clone());
        tx.batch_execute("TRUNCATE events, cursors, stream_stats, gate_stats, gate_commission RESTART IDENTITY")
            .await?;
    }

    // 2. meta.
    let mut meta_keys = Vec::new();
    let mut skipped = Vec::new();
    for (k, v) in &src.meta {
        if is_copied_meta(k) {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                &[k, v],
            )
            .await?;
            meta_keys.push(k.clone());
        } else {
            skipped.push(k.clone());
        }
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ($1, 'postgres') ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        &[&SCHEMA_WRITER_KEY],
    )
    .await?;
    meta_keys.push(SCHEMA_WRITER_KEY.to_owned());

    // 3. cursors.
    for (stream, cursor) in &src.cursors {
        tx.execute("INSERT INTO cursors (stream, cursor) VALUES ($1, $2)", &[stream, cursor]).await?;
    }

    // 4. events, with their ids, in id order, in batches.
    for batch in src.events.chunks(500) {
        let ids: Vec<i64> = batch.iter().map(|e| e.id).collect();
        let streams: Vec<&str> = batch.iter().map(|e| e.stream.as_str()).collect();
        let digests: Vec<&str> = batch.iter().map(|e| e.tx_digest.as_str()).collect();
        let indexes: Vec<i64> = batch.iter().map(|e| e.event_index).collect();
        let checkpoints: Vec<Option<i64>> = batch.iter().map(|e| e.checkpoint).collect();
        let senders: Vec<&str> = batch.iter().map(|e| e.sender.as_str()).collect();
        let types: Vec<&str> = batch.iter().map(|e| e.event_type.as_str()).collect();
        let bcs: Vec<&[u8]> = batch.iter().map(|e| e.bcs.as_slice()).collect();
        let kinds: Vec<&str> = batch.iter().map(|e| e.kind.as_str()).collect();
        let gates: Vec<Option<&str>> = batch.iter().map(|e| e.gate_id.as_deref()).collect();
        let commissions: Vec<Option<i64>> = batch.iter().map(|e| e.commission_mist).collect();
        let params: [&(dyn ToSql + Sync); 11] =
            [&ids, &streams, &digests, &indexes, &checkpoints, &senders, &types, &bcs, &kinds, &gates, &commissions];
        tx.execute(
            "INSERT INTO events (id, stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind, gate_id, commission_mist)
             OVERRIDING SYSTEM VALUE
             SELECT * FROM unnest($1::bigint[], $2::text[], $3::text[], $4::bigint[], $5::bigint[], $6::text[], $7::text[],
                                  $8::bytea[], $9::text[], $10::text[], $11::bigint[])
             ORDER BY 1",
            &params,
        )
        .await?;
    }
    // The next identity value follows the largest imported id (a fresh sequence would start at 1 and
    // collide with every imported row).
    tx.execute(
        "SELECT setval(pg_get_serial_sequence('public.events', 'id'), GREATEST($1::bigint, 1), $1::bigint > 0)",
        &[&want_max_id],
    )
    .await?;

    // 5. counters, as the old writer kept them.
    for (stream, from, latest, events, undecodable, gaps) in &src.stream_stats {
        tx.execute(
            "INSERT INTO stream_stats (stream, from_checkpoint, latest_checkpoint, events, undecodable, gaps)
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[stream, from, latest, events, undecodable, gaps],
        )
        .await?;
    }
    for (gate, kind, n) in &src.gate_stats {
        tx.execute("INSERT INTO gate_stats (gate_id, kind, n) VALUES ($1, $2, $3)", &[gate, kind, n]).await?;
    }
    for (gate, total) in &src.gate_commission {
        tx.execute("INSERT INTO gate_commission (gate_id, total) VALUES ($1, $2::text::numeric)", &[gate, total])
            .await?;
    }

    // 6. Verify what is in the transaction against what was read. Nothing is committed unless it all holds.
    let mut problems: Vec<String> = Vec::new();
    let got_events: BTreeMap<String, i64> = tx
        .query("SELECT stream, count(*) FROM events GROUP BY stream", &[])
        .await?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    if got_events != want_events {
        problems.push(format!("event counts differ: sqlite {want_events:?}, postgres {got_events:?}"));
    }
    let got_cursors: BTreeMap<String, String> = tx
        .query("SELECT stream, cursor FROM cursors WHERE cursor IS NOT NULL", &[])
        .await?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    if got_cursors != want_cursors {
        problems.push(format!("cursors differ: sqlite {want_cursors:?}, postgres {got_cursors:?}"));
    }
    let got_max: i64 = tx.query_one("SELECT coalesce(max(id), 0) FROM events", &[]).await?.get(0);
    if got_max != want_max_id {
        problems.push(format!("max event id differs: sqlite {want_max_id}, postgres {got_max}"));
    }
    let rows = tx.query("SELECT id, tx_digest, event_index FROM events ORDER BY id", &[]).await?;
    let got_hash = order_hash(rows.iter().map(|r| (r.get::<_, i64>(0), r.get::<_, &str>(1), r.get::<_, i64>(2))));
    if got_hash != want_hash {
        problems.push(format!(
            "the (id, tx_digest, event_index) order hash differs: sqlite {want_hash}, postgres {got_hash}"
        ));
    }
    let got_commission: String =
        tx.query_one("SELECT coalesce(sum(total), 0)::numeric(39,0)::text FROM gate_commission", &[]).await?.get(0);
    if got_commission != want_commission.to_string() {
        problems.push(format!("commission totals differ: sqlite {want_commission}, postgres {got_commission}"));
    }
    let seq_next: i64 = tx
        .query_one("SELECT CASE WHEN is_called THEN last_value + 1 ELSE last_value END FROM events_id_seq", &[])
        .await?
        .get(0);
    if seq_next <= want_max_id {
        problems
            .push(format!("the identity sequence would hand out {seq_next}, not above the largest id {want_max_id}"));
    }
    let generation_now: String = tx.query_one("SELECT value FROM meta WHERE key = 'generation'", &[]).await?.get(0);
    if generation_now != generation {
        problems.push(format!("generation {generation_now} is not the imported {generation}"));
    }
    problems.extend(verify_counters(&tx).await?);
    if !problems.is_empty() {
        // Dropping `tx` rolls the whole import back.
        return Err(StoreError::Data(format!(
            "the import did not verify and was rolled back: {}",
            problems.join("; ")
        )));
    }
    tx.commit().await?;
    store.set_generation(generation.clone());
    Ok(ImportReport {
        generation,
        events: want_events,
        cursors: want_cursors,
        max_id: want_max_id,
        commission_total: want_commission,
        order_hash: want_hash,
        meta_keys,
        skipped_meta_keys: skipped,
        gate_stats_rows: src.gate_stats.len(),
        gate_commission_rows: src.gate_commission.len(),
        backup,
    })
}
