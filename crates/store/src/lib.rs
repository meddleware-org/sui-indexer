//! PostgreSQL storage shared by the two sui-indexer processes. This crate holds **all** the SQL.
//!
//! - [`Store`] is the **writer**, owned by the ingester (the only process that writes): one connection
//!   holding the single-writer advisory lock, the owner role, migrations at startup.
//! - [`Reader`] is the API's pool over the **SELECT-only** role; it never migrates.
//!
//! The logical schema, meta keys, generation semantics and counters are those of the SQLite store of
//! 0.0.4-0.0.13 (`migrations/0001_init.sql` maps it to PostgreSQL column types and constraints).
//!
//! Rows get ascending ids in ingest order (an identity column written by one connection), and ingest
//! reads the ledger in ascending order, so `id` is ledger order: a page bounded by `before_id` never
//! changes once served. Ids are ordered, **not gapless** (a duplicate ignored by `ON CONFLICT` or a
//! rolled-back page consumes numbers); nothing here reads an id as a count. `generation` is random per
//! database, so cursors from a rebuilt database are refused.

pub mod config;
pub mod db;
pub mod error;
#[cfg(feature = "import-sqlite")]
pub mod import;
pub mod migrate;
mod read;
pub mod reader;
pub mod sui;
pub mod time;
pub mod writer;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use db::DatabaseSettings;
pub use error::{Result, StoreError};
pub use migrate::SCHEMA_VERSION;
pub use read::{CoverageSnapshot, EventsSnapshot, GateSnapshot};
pub use reader::{ReadError, Reader};
pub use writer::{ConnectOptions, INDEX_TABLES, Store, WRITER_LOCK_KEY, read_alive};

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

/// The ingester's per-stream heartbeat (`heartbeats poll:<stream>`), read by the API's `/healthz` and head
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

/// `heartbeats` key of the ingester's liveness heartbeat (epoch ms, written at least once per request).
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
             Set REINDEX_CONFIRM={} to back up the index (a bak_<time> schema) and re-index from the start.",
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
    Store(StoreError),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::PackageChanged(e) => e.fmt(f),
            BindError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for BindError {}

impl From<StoreError> for BindError {
    fn from(e: StoreError) -> Self {
        BindError::Store(e)
    }
}

/// The JavaScript `String.prototype.slice(0, n)` of `s` (UTF-16 code units), as the TypeScript
/// indexer stored gap reasons. A surrogate pair cut in half is dropped rather than split.
pub(crate) fn truncate_utf16(s: &str, n: usize) -> String {
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

/// The `heartbeats` key of a stream's poll state.
pub fn poll_key(stream: Stream) -> String {
    format!("poll:{stream}")
}

#[cfg(test)]
mod index_tests;
#[cfg(any(test, feature = "testing"))]
pub mod testdb;
#[cfg(test)]
mod tests;
