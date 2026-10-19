//! SQLite storage shared by the two sui-indexer processes.
//!
//! - [`Store`] is the **writer**, owned by the ingester (the only process that writes).
//! - [`Reader`] is the **read-only** view the API opens (`SQLITE_OPEN_READ_ONLY` + `PRAGMA query_only`).
//!
//! The schema, pragmas, meta keys, generation semantics, counters and the one-time counter backfill
//! are byte-for-byte those of the TypeScript `src/store.ts` (sui-indexer ≤ 0.0.11), so the live
//! database carries over without a reindex.
//!
//! Rows get ascending ids in ingest order, and ingest reads the ledger in ascending order, so `id` is
//! ledger order: a page bounded by `before_id` never changes once served. `generation` is random per
//! database, so cursors from a rebuilt database are refused.

pub mod config;
pub mod sui;
pub mod time;

use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub use rusqlite::Error as SqlError;

/// An event stream this indexer keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stream {
    AccessGate,
    SealedContent,
}

impl Stream {
    /// Every stream, in poll order.
    pub const ALL: [Stream; 2] = [Stream::AccessGate, Stream::SealedContent];

    /// The stream's name as stored (`access-gate`, `sealed-content`).
    pub fn as_str(self) -> &'static str {
        match self {
            Stream::AccessGate => "access-gate",
            Stream::SealedContent => "sealed-content",
        }
    }

    /// The stream named `s`, if any.
    pub fn parse(s: &str) -> Option<Stream> {
        match s {
            "access-gate" => Some(Stream::AccessGate),
            "sealed-content" => Some(Stream::SealedContent),
            _ => None,
        }
    }
}

impl fmt::Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One indexed event: the full node's entry verbatim, plus the columns queries filter on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedEvent {
    pub tx_digest: String,
    pub event_index: u32,
    pub checkpoint: Option<u64>,
    pub sender: String,
    pub event_type: String,
    pub bcs: Vec<u8>,
    /// Event kind (`AccessMinted`, …, or `SealedContentPublished`).
    pub kind: String,
    pub gate_id: Option<String>,
    /// Commission of an `AccessMinted` event (fits in 63 bits: at most 10% of a u64 price).
    pub commission_mist: Option<i64>,
}

/// A stored row as served: the event entry the clients decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub id: i64,
    pub event_type: String,
    pub sender: String,
    pub bcs: Vec<u8>,
    pub checkpoint: Option<String>,
    pub transaction_digest: String,
    pub event_index: i64,
}

/// A page request: newest first, optionally filtered.
#[derive(Clone, Debug, Default)]
pub struct PageQuery {
    pub kinds: Option<Vec<String>>,
    pub gate_id: Option<String>,
    /// Only rows with a smaller id (older).
    pub before_id: Option<i64>,
    pub limit: u32,
}

/// A page of rows and the id to pass as `before_id` for older rows (`None` at the end).
#[derive(Clone, Debug)]
pub struct Page {
    pub rows: Vec<EventRow>,
    pub next: Option<i64>,
}

/// Oldest and newest indexed checkpoints and the event count of a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    pub from_checkpoint: Option<String>,
    pub latest_checkpoint: Option<String>,
    pub events: i64,
}

/// Events skipped as undecodable, and cursor resets after pruning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamHealth {
    pub undecodable: i64,
    pub gaps: i64,
}

/// Per-gate counts by kind and the commission its mints paid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateStats {
    pub counts: Vec<(String, i64)>,
    pub commission_mist: u128,
}

impl GateStats {
    /// The count of `kind` (0 when absent).
    pub fn count(&self, kind: &str) -> i64 {
        self.counts.iter().find(|(k, _)| k == kind).map(|(_, n)| *n).unwrap_or(0)
    }
}

/// The ingester's per-stream heartbeat (`meta poll:<stream>`), read by the API's `/healthz` and head
/// pages. `lastAttemptAt` is the end of the latest poll whatever its outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollState {
    /// Epoch ms of the last poll of this stream that completed (0 before the first).
    pub last_success_at: i64,
    /// The error of the last poll, or `None` after a success.
    pub last_error: Option<String>,
    /// Epoch ms of the end of the last poll, successful or not (0 before the first).
    #[serde(default)]
    pub last_attempt_at: i64,
}

/// `meta` key of the ingester's liveness heartbeat (epoch ms, written at least once per request).
pub const ALIVE_KEY: &str = "ingest:alive";
/// `meta` key recording which implementation last opened the database as writer.
pub const SCHEMA_WRITER_KEY: &str = "schema_writer";

/// Refused package change: clearing the streams would discard history the full node may already
/// have pruned, so it needs an explicit confirmation (`REINDEX_CONFIRM=<stream>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageChangedError {
    pub streams: Vec<Stream>,
}

impl fmt::Display for PackageChangedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = self.streams.iter().map(|s| s.as_str()).collect::<Vec<_>>();
        write!(
            f,
            "the package indexed by {} changed (or its rows predate package binding). \
             Re-indexing deletes the stored history, which the full node may have pruned. \
             Set REINDEX_CONFIRM={} to back up the database and re-index from the start.",
            list.join(", "),
            list.join(",")
        )
    }
}

impl std::error::Error for PackageChangedError {}

/// Errors of [`Store::bind_packages`].
#[derive(Debug)]
pub enum BindError {
    PackageChanged(PackageChangedError),
    Sql(rusqlite::Error),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::PackageChanged(e) => e.fmt(f),
            BindError::Sql(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for BindError {}

impl From<rusqlite::Error> for BindError {
    fn from(e: rusqlite::Error) -> Self {
        BindError::Sql(e)
    }
}

/// The schema of `src/store.ts` (0.0.4–0.0.11), unchanged: same STRICT tables, columns and indexes.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS cursors (stream TEXT PRIMARY KEY, cursor TEXT) STRICT;
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY,
  stream TEXT NOT NULL,
  tx_digest TEXT NOT NULL,
  event_index INTEGER NOT NULL,
  checkpoint INTEGER,
  sender TEXT NOT NULL,
  event_type TEXT NOT NULL,
  bcs BLOB NOT NULL,
  kind TEXT NOT NULL,
  gate_id TEXT,
  commission_mist INTEGER,
  UNIQUE (tx_digest, event_index)
) STRICT;
CREATE INDEX IF NOT EXISTS events_stream ON events (stream, id);
CREATE INDEX IF NOT EXISTS events_gate ON events (stream, gate_id, id);
-- Counters maintained inside the ingest transaction, so serving a page never scans a stream.
CREATE TABLE IF NOT EXISTS stream_stats (
  stream TEXT PRIMARY KEY,
  from_checkpoint INTEGER,
  latest_checkpoint INTEGER,
  events INTEGER NOT NULL DEFAULT 0,
  undecodable INTEGER NOT NULL DEFAULT 0,
  gaps INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE IF NOT EXISTS gate_stats (
  gate_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  n INTEGER NOT NULL,
  PRIMARY KEY (gate_id, kind)
) STRICT;
CREATE TABLE IF NOT EXISTS gate_commission (gate_id TEXT PRIMARY KEY, total TEXT NOT NULL) STRICT;
";

/// Six random bytes in hex: a new database generation.
fn new_generation() -> String {
    let mut b = [0u8; 6];
    getrandom::fill(&mut b).expect("the OS random source is available");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )?
    .execute(params![key, value])?;
    Ok(())
}

fn get_meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.prepare_cached("SELECT value FROM meta WHERE key = ?")?.query_row([key], |r| r.get(0)).optional()
}

/// Run `f` in a transaction (`BEGIN` … `COMMIT`, `ROLLBACK` on error).
fn in_tx<T>(conn: &Connection, f: impl FnOnce() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    conn.execute_batch("BEGIN")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Keep the `-wal` and `-shm` files when the writer closes, so the API's read-only connection (whose
/// volume is mounted read-only and cannot create them) can still open the database while the
/// ingester is stopped (SQLite: a read-only WAL database opens only if both files exist).
#[allow(unsafe_code)]
fn persist_wal(conn: &Connection) -> rusqlite::Result<()> {
    let mut on: std::os::raw::c_int = 1;
    // SAFETY: `conn.handle()` is a live connection for the duration of the call, the database name
    // is a NUL-terminated literal, and SQLITE_FCNTL_PERSIST_WAL takes a pointer to an int that SQLite
    // reads and writes only during the call.
    let rc = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_PERSIST_WAL,
            (&mut on as *mut std::os::raw::c_int).cast(),
        )
    };
    if rc == rusqlite::ffi::SQLITE_OK || rc == rusqlite::ffi::SQLITE_NOTFOUND {
        // NOTFOUND: an in-memory database has no file (tests).
        Ok(())
    } else {
        Err(rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rc), Some("SQLITE_FCNTL_PERSIST_WAL".into())))
    }
}

/// The writer: SQLite (WAL), owned by the ingester. Every write path of the indexer is here.
pub struct Store {
    conn: Connection,
    generation: String,
    /// Whether the database existed with a generation before this open (an earlier writer's database).
    preexisting: bool,
}

impl Store {
    /// Open (creating if needed) the database at `path` (`:memory:` for an in-memory one), apply the
    /// schema, read or create the generation, and build the counters once for older databases.
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Store> {
        let conn = Connection::open(path)?;
        Store::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> rusqlite::Result<Store> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;")?;
        persist_wal(&conn)?;
        conn.execute_batch(SCHEMA)?;
        let (generation, preexisting) = match get_meta(&conn, "generation")? {
            Some(g) => (g, true),
            None => {
                let g = new_generation();
                conn.execute("INSERT INTO meta (key, value) VALUES ('generation', ?)", [&g])?;
                (g, false)
            }
        };
        let store = Store { conn, generation, preexisting };
        store.backfill_stats()?;
        Ok(store)
    }

    /// The underlying connection (tests and the parity tooling read it directly).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Build the counters from existing rows once (databases created before they existed).
    fn backfill_stats(&self) -> rusqlite::Result<()> {
        if get_meta(&self.conn, "stats:v2")?.is_some() {
            return Ok(());
        }
        in_tx(&self.conn, || {
            self.conn.execute_batch(
                "DELETE FROM stream_stats; DELETE FROM gate_stats; DELETE FROM gate_commission;
                 INSERT INTO stream_stats (stream, from_checkpoint, latest_checkpoint, events)
                   SELECT stream, MIN(checkpoint), MAX(checkpoint), COUNT(*) FROM events GROUP BY stream;
                 INSERT INTO gate_stats (gate_id, kind, n)
                   SELECT gate_id, kind, COUNT(*) FROM events WHERE stream = 'access-gate' AND gate_id IS NOT NULL GROUP BY gate_id, kind;",
            )?;
            let mut totals: Vec<(String, u128)> = Vec::new();
            {
                let mut stmt = self.conn.prepare(
                    "SELECT gate_id, commission_mist FROM events WHERE stream = 'access-gate' AND gate_id IS NOT NULL AND commission_mist IS NOT NULL ORDER BY id",
                )?;
                let mut rows = stmt.query([])?;
                while let Some(r) = rows.next()? {
                    let g: String = r.get(0)?;
                    let c: i64 = r.get(1)?;
                    let c = u128::try_from(c).unwrap_or(0);
                    match totals.iter_mut().find(|(k, _)| *k == g) {
                        Some((_, t)) => *t += c,
                        None => totals.push((g, c)),
                    }
                }
            }
            let mut put = self.conn.prepare("INSERT INTO gate_commission (gate_id, total) VALUES (?, ?)")?;
            for (g, total) in &totals {
                put.execute(params![g, total.to_string()])?;
            }
            self.conn.execute("INSERT INTO meta (key, value) VALUES ('stats:v2', '1')", [])?;
            Ok(())
        })
    }

    /// Random per index build; API cursors carry it, and a cursor from another generation is refused.
    pub fn generation(&self) -> &str {
        &self.generation
    }

    /// The first open by this (Rust) implementation of a database an earlier writer (the TypeScript
    /// indexer ≤ 0.0.11) built: copy it to `backup_path` (`VACUUM INTO`) before writing anything else,
    /// then record `meta schema_writer = rust`, so the cutover can be rolled back to the previous image
    /// with that copy. A new database, or one already marked, is only marked (no copy).
    ///
    /// Returns whether a backup was written.
    pub fn mark_rust_writer(&self, backup_path: &str) -> rusqlite::Result<bool> {
        if get_meta(&self.conn, SCHEMA_WRITER_KEY)?.as_deref() == Some("rust") {
            return Ok(false);
        }
        let backup = self.preexisting;
        if backup {
            self.vacuum_into(backup_path)?;
        }
        set_meta(&self.conn, SCHEMA_WRITER_KEY, "rust")?;
        Ok(backup)
    }

    fn vacuum_into(&self, path: &str) -> rusqlite::Result<()> {
        self.conn.execute_batch(&format!("VACUUM INTO '{}'", path.replace('\'', "''")))
    }

    /// Bind each stream to the package (original id) it indexes. A stream recorded under a different
    /// package — or holding rows from before packages were recorded — would have to be cleared to
    /// re-read its new package. That discards history the full node may already have pruned, so it is
    /// refused with [`PackageChangedError`] unless the stream is listed in `confirm`; confirming
    /// first copies the database to `backup_path` (`VACUUM INTO`), then clears the stream's rows,
    /// cursor and counters and rotates the generation (so cursors served from the old index are
    /// refused). Call once at startup, before ingesting.
    ///
    /// Returns the streams that were cleared.
    pub fn bind_packages(
        &mut self,
        packages: &[(Stream, String)],
        confirm: &[Stream],
        backup_path: Option<&str>,
    ) -> Result<Vec<Stream>, BindError> {
        let mut pending = Vec::new();
        for (stream, pkg) in packages {
            let recorded = get_meta(&self.conn, &format!("package:{stream}"))?;
            if recorded.as_deref() == Some(pkg.as_str()) {
                continue;
            }
            let rows: i64 =
                self.conn.query_row("SELECT COUNT(*) FROM events WHERE stream = ?", [stream.as_str()], |r| r.get(0))?;
            if recorded.is_some() || rows > 0 || self.cursor(*stream)?.is_some() {
                pending.push(*stream);
            }
        }
        let confirmed: Vec<Stream> = pending.iter().copied().filter(|s| confirm.contains(s)).collect();
        if pending.len() != confirmed.len() {
            let refused = pending.into_iter().filter(|s| !confirmed.contains(s)).collect();
            return Err(BindError::PackageChanged(PackageChangedError { streams: refused }));
        }
        if !confirmed.is_empty()
            && let Some(path) = backup_path
        {
            self.vacuum_into(path)?;
        }
        let mut generation = None;
        in_tx(&self.conn, || {
            for stream in &confirmed {
                self.conn.execute("DELETE FROM events WHERE stream = ?", [stream.as_str()])?;
                self.conn.execute("DELETE FROM cursors WHERE stream = ?", [stream.as_str()])?;
                self.conn.execute("DELETE FROM stream_stats WHERE stream = ?", [stream.as_str()])?;
                if *stream == Stream::AccessGate {
                    self.conn.execute_batch("DELETE FROM gate_stats; DELETE FROM gate_commission;")?;
                }
            }
            for (stream, pkg) in packages {
                set_meta(&self.conn, &format!("package:{stream}"), pkg)?;
            }
            if !confirmed.is_empty() {
                let g = new_generation();
                set_meta(&self.conn, "generation", &g)?;
                generation = Some(g);
            }
            Ok(())
        })?;
        if let Some(g) = generation {
            self.generation = g;
        }
        Ok(confirmed)
    }

    /// The stored ledger cursor of `stream` (base64 of the full node's watermark cursor bytes).
    pub fn cursor(&self, stream: Stream) -> rusqlite::Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT cursor FROM cursors WHERE stream = ?")?
            .query_row([stream.as_str()], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten())
    }

    /// Insert a page of events and advance the stream's cursor in one transaction, so a crash never
    /// records a cursor past events it did not store. Re-inserting an event is a no-op.
    ///
    /// Returns how many events were new.
    pub fn ingest(&self, stream: Stream, events: &[IndexedEvent], cursor: Option<&str>) -> rusqlite::Result<usize> {
        let c = &self.conn;
        in_tx(c, || {
            let mut insert = c.prepare_cached(
                "INSERT OR IGNORE INTO events
                   (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind, gate_id, commission_mist)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            let mut bump_stream = c.prepare_cached(
                "INSERT INTO stream_stats (stream, from_checkpoint, latest_checkpoint, events) VALUES (?, ?, ?, 1)
                 ON CONFLICT (stream) DO UPDATE SET
                   from_checkpoint = CASE WHEN excluded.from_checkpoint IS NULL THEN from_checkpoint
                                          WHEN from_checkpoint IS NULL OR excluded.from_checkpoint < from_checkpoint THEN excluded.from_checkpoint
                                          ELSE from_checkpoint END,
                   latest_checkpoint = CASE WHEN excluded.latest_checkpoint IS NULL THEN latest_checkpoint
                                            WHEN latest_checkpoint IS NULL OR excluded.latest_checkpoint > latest_checkpoint THEN excluded.latest_checkpoint
                                            ELSE latest_checkpoint END,
                   events = events + 1",
            )?;
            let mut bump_gate = c.prepare_cached(
                "INSERT INTO gate_stats (gate_id, kind, n) VALUES (?, ?, 1) ON CONFLICT (gate_id, kind) DO UPDATE SET n = n + 1",
            )?;
            let mut read_commission = c.prepare_cached("SELECT total FROM gate_commission WHERE gate_id = ?")?;
            let mut write_commission = c.prepare_cached(
                "INSERT INTO gate_commission (gate_id, total) VALUES (?, ?) ON CONFLICT (gate_id) DO UPDATE SET total = excluded.total",
            )?;
            let mut added = 0;
            for e in events {
                let ckpt = e.checkpoint.map(checkpoint_i64).transpose()?;
                let changed = insert.execute(params![
                    stream.as_str(),
                    e.tx_digest,
                    e.event_index,
                    ckpt,
                    e.sender,
                    e.event_type,
                    e.bcs,
                    e.kind,
                    e.gate_id,
                    e.commission_mist,
                ])?;
                if changed == 0 {
                    continue; // already stored: counters were bumped then
                }
                added += 1;
                bump_stream.execute(params![stream.as_str(), ckpt, ckpt])?;
                if stream == Stream::AccessGate
                    && let Some(gate) = &e.gate_id
                {
                    bump_gate.execute(params![gate, e.kind])?;
                    if let Some(commission) = e.commission_mist {
                        let cur: Option<String> = read_commission.query_row([gate], |r| r.get(0)).optional()?;
                        let cur = cur.and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
                        let add = u128::try_from(commission).unwrap_or(0);
                        write_commission.execute(params![gate, (cur + add).to_string()])?;
                    }
                }
            }
            if let Some(cursor) = cursor {
                c.prepare_cached(
                    "INSERT INTO cursors (stream, cursor) VALUES (?, ?) ON CONFLICT (stream) DO UPDATE SET cursor = excluded.cursor",
                )?
                .execute(params![stream.as_str(), cursor])?;
            }
            Ok(added)
        })
    }

    /// Record `n` events skipped because they did not decode (the page still advances past them).
    pub fn note_undecodable(&self, stream: Stream, n: i64) -> rusqlite::Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO stream_stats (stream, undecodable) VALUES (?, ?)
                 ON CONFLICT (stream) DO UPDATE SET undecodable = undecodable + excluded.undecodable",
            )?
            .execute(params![stream.as_str(), n])?;
        Ok(())
    }

    /// The stored cursor points below what the full node still serves (its history was pruned while
    /// no event arrived to move the cursor). The events in between are gone for good, so restart from
    /// the node's earliest data: rows already stored are untouched (re-inserting is a no-op). Counted
    /// as a gap; `at_iso` and the reason (first 200 characters) go to `meta gap:<stream>`.
    pub fn reset_cursor_after_gap(&self, stream: Stream, reason: &str, at_iso: &str) -> rusqlite::Result<()> {
        in_tx(&self.conn, || {
            self.conn.execute("DELETE FROM cursors WHERE stream = ?", [stream.as_str()])?;
            self.conn.execute(
                "INSERT INTO stream_stats (stream, gaps) VALUES (?, 1) ON CONFLICT (stream) DO UPDATE SET gaps = gaps + 1",
                [stream.as_str()],
            )?;
            let reason: String = truncate_utf16(reason, 200);
            let value = serde_json::json!({ "at": at_iso, "reason": reason });
            set_meta(&self.conn, &format!("gap:{stream}"), &value.to_string())
        })
    }

    /// Write the stream's heartbeat (`meta poll:<stream>`).
    pub fn record_poll(&self, stream: Stream, state: &PollState) -> rusqlite::Result<()> {
        let value = serde_json::to_string(state).expect("PollState serialises");
        set_meta(&self.conn, &poll_key(stream), &value)
    }

    /// Write the ingester's liveness heartbeat (`meta ingest:alive`, epoch ms).
    pub fn touch_alive(&self, now_ms: i64) -> rusqlite::Result<()> {
        set_meta(&self.conn, ALIVE_KEY, &now_ms.to_string())
    }

    // Read paths shared with the API (the writer's tests use them too).

    pub fn page(&self, stream: Stream, q: &PageQuery) -> rusqlite::Result<Page> {
        read::page(&self.conn, stream, q)
    }
    pub fn head_id(&self, stream: Stream) -> rusqlite::Result<i64> {
        read::head_id(&self.conn, stream)
    }
    pub fn coverage(&self, stream: Stream) -> rusqlite::Result<Coverage> {
        read::coverage(&self.conn, stream)
    }
    pub fn health(&self, stream: Stream) -> rusqlite::Result<StreamHealth> {
        read::health(&self.conn, stream)
    }
    pub fn gate_stats(&self, gate_id: &str) -> rusqlite::Result<GateStats> {
        read::gate_stats(&self.conn, gate_id)
    }
    pub fn poll_state(&self, stream: Stream) -> rusqlite::Result<PollState> {
        read::poll_state(&self.conn, stream)
    }
    pub fn meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        get_meta(&self.conn, key)
    }
}

/// The JavaScript `String.prototype.slice(0, n)` of `s` (UTF-16 code units), as the TypeScript
/// indexer stored gap reasons. A surrogate pair cut in half is dropped rather than split.
fn truncate_utf16(s: &str, n: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for ch in s.chars() {
        units += ch.len_utf16();
        if units > n {
            break;
        }
        out.push(ch);
    }
    out
}

fn checkpoint_i64(c: u64) -> rusqlite::Result<i64> {
    i64::try_from(c).map_err(|_| rusqlite::Error::ToSqlConversionFailure("checkpoint exceeds i64".into()))
}

/// The meta key of a stream's heartbeat.
pub fn poll_key(stream: Stream) -> String {
    format!("poll:{stream}")
}

/// The read-only view the API opens: `SQLITE_OPEN_READ_ONLY` and `PRAGMA query_only`, so a
/// compromised API process cannot write the index even through SQL.
pub struct Reader {
    conn: Connection,
}

/// Why a [`Reader`] could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// The database (or its schema) does not exist yet: the ingester has not created it.
    NotReady(String),
    Sql(rusqlite::Error),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NotReady(m) => write!(f, "index not ready: {m}"),
            OpenError::Sql(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for OpenError {}

impl Reader {
    /// Open the database at `path` read-only. Fails with [`OpenError::NotReady`] until the ingester
    /// has created the file and its schema (the `meta` generation row).
    pub fn open(path: impl AsRef<Path>) -> Result<Reader, OpenError> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(OpenError::NotReady(format!("{} does not exist", path.display())));
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(OpenError::Sql)?;
        conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;").map_err(OpenError::Sql)?;
        let reader = Reader { conn };
        match reader.generation() {
            Ok(Some(_)) => Ok(reader),
            Ok(None) => Err(OpenError::NotReady("no generation recorded yet".into())),
            Err(rusqlite::Error::SqliteFailure(_, Some(m))) if m.contains("no such table") => {
                Err(OpenError::NotReady("schema not created yet".into()))
            }
            Err(e) => Err(OpenError::Sql(e)),
        }
    }

    /// The current generation (read per request: the ingester rotates it on a confirmed reindex).
    pub fn generation(&self) -> rusqlite::Result<Option<String>> {
        get_meta(&self.conn, "generation")
    }
    pub fn page(&self, stream: Stream, q: &PageQuery) -> rusqlite::Result<Page> {
        read::page(&self.conn, stream, q)
    }
    pub fn head_id(&self, stream: Stream) -> rusqlite::Result<i64> {
        read::head_id(&self.conn, stream)
    }
    pub fn coverage(&self, stream: Stream) -> rusqlite::Result<Coverage> {
        read::coverage(&self.conn, stream)
    }
    pub fn health(&self, stream: Stream) -> rusqlite::Result<StreamHealth> {
        read::health(&self.conn, stream)
    }
    pub fn gate_stats(&self, gate_id: &str) -> rusqlite::Result<GateStats> {
        read::gate_stats(&self.conn, gate_id)
    }
    pub fn poll_state(&self, stream: Stream) -> rusqlite::Result<PollState> {
        read::poll_state(&self.conn, stream)
    }
    pub fn meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        get_meta(&self.conn, key)
    }
    /// Begin a read transaction so several reads see one snapshot (end it with [`Reader::end`]).
    pub fn begin(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN")
    }
    /// End the read transaction started by [`Reader::begin`].
    pub fn end(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("COMMIT")
    }
    /// Whether a write through this connection is refused (it must be).
    pub fn try_write(&self) -> rusqlite::Result<usize> {
        self.conn.execute("INSERT INTO meta (key, value) VALUES ('probe', 'x')", [])
    }
}

/// Queries both connections run. None of them scans a stream: counters are maintained at ingest.
mod read {
    use super::*;

    pub fn page(conn: &Connection, stream: Stream, q: &PageQuery) -> rusqlite::Result<Page> {
        let mut sql = String::from(
            "SELECT id, event_type, sender, bcs, checkpoint, tx_digest, event_index FROM events WHERE stream = ?",
        );
        let mut args: Vec<rusqlite::types::Value> = vec![stream.as_str().to_owned().into()];
        if let Some(kinds) = q.kinds.as_ref().filter(|k| !k.is_empty()) {
            sql.push_str(&format!(" AND kind IN ({})", vec!["?"; kinds.len()].join(", ")));
            args.extend(kinds.iter().map(|k| k.clone().into()));
        }
        if let Some(g) = &q.gate_id {
            sql.push_str(" AND gate_id = ?");
            args.push(g.clone().into());
        }
        if let Some(b) = q.before_id {
            sql.push_str(" AND id < ?");
            args.push(b.into());
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        args.push(i64::from(q.limit + 1).into());
        let mut stmt = conn.prepare_cached(&sql)?;
        let mut rows: Vec<EventRow> = stmt
            .query_map(rusqlite::params_from_iter(args), |r| {
                Ok(EventRow {
                    id: r.get(0)?,
                    event_type: r.get(1)?,
                    sender: r.get(2)?,
                    bcs: r.get(3)?,
                    checkpoint: r.get::<_, Option<i64>>(4)?.map(|c| c.to_string()),
                    transaction_digest: r.get(5)?,
                    event_index: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let more = rows.len() > q.limit as usize;
        rows.truncate(q.limit as usize);
        let next = if more { rows.last().map(|r| r.id) } else { None };
        Ok(Page { rows, next })
    }

    pub fn head_id(conn: &Connection, stream: Stream) -> rusqlite::Result<i64> {
        conn.prepare_cached("SELECT MAX(id) FROM events WHERE stream = ?")?
            .query_row([stream.as_str()], |r| r.get::<_, Option<i64>>(0))
            .map(|v| v.unwrap_or(0))
    }

    pub fn coverage(conn: &Connection, stream: Stream) -> rusqlite::Result<Coverage> {
        let row = conn
            .prepare_cached("SELECT from_checkpoint, latest_checkpoint, events FROM stream_stats WHERE stream = ?")?
            .query_row([stream.as_str()], |r| {
                Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, i64>(2)?))
            })
            .optional()?;
        Ok(match row {
            Some((lo, hi, n)) => Coverage {
                from_checkpoint: lo.map(|v| v.to_string()),
                latest_checkpoint: hi.map(|v| v.to_string()),
                events: n,
            },
            None => Coverage { from_checkpoint: None, latest_checkpoint: None, events: 0 },
        })
    }

    pub fn health(conn: &Connection, stream: Stream) -> rusqlite::Result<StreamHealth> {
        let row = conn
            .prepare_cached("SELECT undecodable, gaps FROM stream_stats WHERE stream = ?")?
            .query_row([stream.as_str()], |r| Ok(StreamHealth { undecodable: r.get(0)?, gaps: r.get(1)? }))
            .optional()?;
        Ok(row.unwrap_or(StreamHealth { undecodable: 0, gaps: 0 }))
    }

    pub fn gate_stats(conn: &Connection, gate_id: &str) -> rusqlite::Result<GateStats> {
        let counts = conn
            .prepare_cached("SELECT kind, n FROM gate_stats WHERE gate_id = ?")?
            .query_map([gate_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let total: Option<String> = conn
            .prepare_cached("SELECT total FROM gate_commission WHERE gate_id = ?")?
            .query_row([gate_id], |r| r.get(0))
            .optional()?;
        Ok(GateStats { counts, commission_mist: total.and_then(|t| t.parse().ok()).unwrap_or(0) })
    }

    pub fn poll_state(conn: &Connection, stream: Stream) -> rusqlite::Result<PollState> {
        Ok(get_meta(conn, &poll_key(stream))?
            .and_then(|v| serde_json::from_str::<PollState>(&v).ok())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests;
