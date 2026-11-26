//! PostgreSQL storage shared by the two sui-indexer processes. This crate holds **all** the SQL.
//!
//! - [`Store`] is the **writer**, owned by the ingester (the only process that writes): one connection
//!   holding the single-writer advisory lock, the owner role, migrations at startup.
//! - [`Reader`] is the API's pool over the **SELECT-only** role; it never migrates.
//!
//! The logical schema (`migrations/0001_init.sql`), meta keys, generation semantics and counters are
//! documented in the README.
//!
//! Rows get ascending ids in ingest order (an identity column written by one connection), and ingest
//! reads the ledger in ascending order, so `id` is ledger order: a page bounded by `before_id` never
//! changes once served. Ids are ordered, **not gapless** (a duplicate ignored by `ON CONFLICT` or a
//! rolled-back page consumes numbers); nothing here reads an id as a count. `generation` is random per
//! database, so cursors from a rebuilt database are refused.

/// Configuration from the environment.
pub mod config;
/// The database settings and connection (TLS verified).
pub mod db;
/// The store error type.
pub mod error;
/// The schema migrations.
pub mod migrate;
/// The shared read queries.
mod read;
/// The API's pooled reader.
pub mod reader;
/// Sui addresses and struct tags.
pub mod sui;
/// Clock helpers.
pub mod time;
/// The writer.
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
    /// The `access_gate` module's events: gates, access NFTs, commission and platform changes.
    AccessGate,
    /// The `SealedContentPublished` events of the sealed-storage package.
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
    /// The stream's kebab-case name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One indexed event: the full node's entry verbatim, plus the columns queries filter on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedEvent {
    /// The digest of the transaction that emitted the event.
    pub tx_digest: String,
    /// The event's position within its transaction (with the digest, the event's unique key).
    pub event_index: u32,
    /// The checkpoint the transaction was in, when the node reported one.
    pub checkpoint: Option<u64>,
    /// The Sui address that emitted the event.
    pub sender: String,
    /// The fully qualified Move event type.
    pub event_type: String,
    /// The event's BCS bytes, stored unchanged.
    pub bcs: Vec<u8>,
    /// Event kind (`AccessMinted`, …, or `SealedContentPublished`).
    pub kind: String,
    /// The gate the event concerns, when its kind has one.
    pub gate_id: Option<String>,
    /// Commission of an `AccessMinted` event (fits in 63 bits: at most 10% of a u64 price).
    pub commission_mist: Option<i64>,
}

/// A stored row as served: the event entry the clients decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    /// The row id, increasing in ledger order; it is the paging cursor and the ETag basis.
    pub id: i64,
    /// The fully qualified Move event type.
    pub event_type: String,
    /// The Sui address that emitted the event.
    pub sender: String,
    /// The event's BCS bytes.
    pub bcs: Vec<u8>,
    /// The checkpoint as a decimal string (it can exceed what JSON numbers hold exactly), when known.
    pub checkpoint: Option<String>,
    /// The digest of the transaction that emitted the event.
    pub transaction_digest: String,
    /// The event's position within its transaction.
    pub event_index: i64,
}

/// A page request: newest first, optionally filtered.
#[derive(Clone, Debug, Default)]
pub struct PageQuery {
    /// Only these event kinds; `None` for all.
    pub kinds: Option<Vec<String>>,
    /// Only events of this gate; `None` for all.
    pub gate_id: Option<String>,
    /// Only rows with a smaller id (older).
    pub before_id: Option<i64>,
    /// The page size.
    pub limit: u32,
}

/// A page of rows and the id to pass as `before_id` for older rows (`None` at the end).
#[derive(Clone, Debug)]
pub struct Page {
    /// The rows, newest first.
    pub rows: Vec<EventRow>,
    /// The id to pass as `before_id` for the next older page, or `None` at the end.
    pub next: Option<i64>,
}

/// Oldest and newest indexed checkpoints and the event count of a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    /// The oldest checkpoint indexed.
    pub from_checkpoint: Option<String>,
    /// The newest checkpoint indexed.
    pub latest_checkpoint: Option<String>,
    /// How many events are stored.
    pub events: i64,
}

/// Events skipped as undecodable, and cursor resets after pruning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamHealth {
    /// How many events were skipped because they could not be decoded.
    pub undecodable: i64,
    /// How many times a cursor was reset after the node pruned history, leaving a gap.
    pub gaps: i64,
}

/// Per-gate counts by kind and the commission its mints paid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateStats {
    /// The count of each event kind for the gate.
    pub counts: Vec<(String, i64)>,
    /// The commission its mints paid, summed in MIST (u128: a sum of u64 values can exceed u64).
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
    /// The streams whose bound package differs from the configured one.
    pub streams: Vec<Stream>,
}

impl fmt::Display for PackageChangedError {
    /// Name the streams whose package changed and say how to confirm re-indexing.
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
    /// The configured packages differ from the ones this database was indexed with; re-indexing needs the operator's confirmation.
    PackageChanged(PackageChangedError),
    /// The store failed while binding.
    Store(StoreError),
}

impl fmt::Display for BindError {
    /// Show the underlying error.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::PackageChanged(e) => e.fmt(f),
            BindError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for BindError {}

impl From<StoreError> for BindError {
    /// Wrap a store error.
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

/// Query-plan tests (they need a database).
#[cfg(test)]
mod index_tests;
/// A throwaway database provisioned like production, for tests.
#[cfg(any(test, feature = "testing"))]
pub mod testdb;
/// Store tests against a real database.
#[cfg(test)]
mod tests;
