//! The poll loop: each stream is read forward from its stored cursor (ascending, so row ids follow
//! ledger order) and every recognised event is stored with the cursor, atomically per page.
//!
//! - The streams are polled independently: one failing never stops the other.
//! - An event that cannot be stored is counted and skipped (the page advances past it).
//! - A cursor that fell below the node's retention restarts from the node's earliest data instead
//!   of retrying forever (a counted gap).
//! - After each stream poll the heartbeat `meta poll:<stream>` is written for the API's `/healthz`,
//!   and `meta ingest:alive` is refreshed around every request for the exec liveness probe.

use std::time::Duration;

use sui_indexer_store::time::{iso, now_ms};
use sui_indexer_store::{IndexedEvent, PollState, Store, Stream};

use crate::decode::{Packages, stream_filter, to_indexed};
use crate::source::{EventSource, ListRequest, PAGE_SIZE};

/// A log sink (one line per call).
pub type Logger = Box<dyn Fn(&str)>;

/// Tuning of the poll loop.
#[derive(Clone, Debug)]
pub struct IngestOptions {
    pub packages: Packages,
    /// Pages read per stream per poll, so one busy stream cannot starve the other (default 20).
    pub max_pages_per_poll: u32,
    /// Per-request timeout for the full node, so a hung call fails the poll instead of stalling it
    /// (default 30 s).
    pub request_timeout: Duration,
}

impl IngestOptions {
    pub fn new(packages: Packages) -> IngestOptions {
        IngestOptions { packages, max_pages_per_poll: 20, request_timeout: Duration::from_secs(30) }
    }
}

/// What the full node says when a cursor falls below the data it still serves (history pruned).
pub fn is_pruned(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("below earliest available") || m.contains("earliest available") || m.contains("has been pruned")
}

pub struct Ingestor<S> {
    store: Store,
    source: S,
    opts: IngestOptions,
    log: Logger,
    state: [PollState; 2],
}

fn idx(stream: Stream) -> usize {
    match stream {
        Stream::AccessGate => 0,
        Stream::SealedContent => 1,
    }
}

impl<S: EventSource> Ingestor<S> {
    pub fn new(store: Store, source: S, opts: IngestOptions, log: Logger) -> Ingestor<S> {
        // Continue from the persisted heartbeats, so a restart does not forget the last success.
        let state = Stream::ALL.map(|s| store.poll_state(s).unwrap_or_default());
        Ingestor { store, source, opts, log, state }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn source(&self) -> &S {
        &self.source
    }

    /// Per-stream poll state (also persisted as the stream's heartbeat).
    pub fn health(&self, stream: Stream) -> &PollState {
        &self.state[idx(stream)]
    }

    /// Epoch ms of the last poll in which EVERY stream succeeded (0 until then).
    pub fn last_success_at(&self) -> i64 {
        self.state.iter().map(|s| s.last_success_at).min().unwrap_or(0)
    }

    fn alive(&self) {
        if let Err(e) = self.store.touch_alive(now_ms()) {
            (self.log)(&format!("liveness heartbeat failed: {e}"));
        }
    }

    /// Read one stream up to the page budget. Returns the number of new events.
    pub async fn poll_stream(&self, stream: Stream) -> Result<usize, String> {
        let event_type = stream_filter(stream, &self.opts.packages);
        let mut added = 0;
        let mut cursor = self.store.cursor(stream).map_err(|e| e.to_string())?;
        for _ in 0..self.opts.max_pages_per_poll {
            let req = ListRequest { event_type: event_type.clone(), after: cursor.clone(), limit: PAGE_SIZE };
            let res = tokio::time::timeout(self.opts.request_timeout, self.source.list_events(req)).await;
            self.alive();
            let res = res.map_err(|_| {
                format!("the full node request timed out after {} ms", self.opts.request_timeout.as_millis())
            })??;
            let mut events: Vec<IndexedEvent> = Vec::with_capacity(res.events.len());
            let (mut skipped, mut undecodable) = (0, 0);
            for entry in &res.events {
                match to_indexed(stream, entry, &self.opts.packages) {
                    Ok(Some(e)) => events.push(e),
                    Ok(None) => skipped += 1,
                    Err(e) => {
                        // A recognised event the ingester cannot store (a node rendering without the
                        // field, a missing position): do not let it stop the stream. Count it, log it
                        // loudly, and move past it.
                        undecodable += 1;
                        (self.log)(&format!(
                            "{stream}: UNDECODABLE event {}#{}: {e}",
                            entry.transaction_digest.as_deref().unwrap_or("?"),
                            entry.event_index.map_or("?".to_owned(), |i| i.to_string())
                        ));
                    }
                }
            }
            if skipped > 0 {
                (self.log)(&format!("{stream}: skipped {skipped} unrecognised event(s)"));
            }
            // A missing end cursor (nothing further) keeps the stored position.
            added += self.store.ingest(stream, &events, res.end_cursor.as_deref()).map_err(|e| e.to_string())?;
            if undecodable > 0 {
                self.store.note_undecodable(stream, undecodable).map_err(|e| e.to_string())?;
            }
            if let Some(c) = res.end_cursor {
                cursor = Some(c);
            }
            if !res.has_next_page {
                break;
            }
        }
        Ok(added)
    }

    /// Poll one stream, recovering from a pruned cursor, and record its health.
    pub async fn poll_one(&mut self, stream: Stream) {
        self.alive();
        let outcome = match self.poll_stream(stream).await {
            Ok(n) => Ok(n),
            Err(message) => {
                let has_cursor = matches!(self.store.cursor(stream), Ok(Some(_)));
                if !is_pruned(&message) || !has_cursor {
                    Err(message)
                } else {
                    // The events between the stored cursor and the node's retention are gone for good.
                    (self.log)(&format!(
                        "{stream}: cursor is below the node's retention ({message}); restarting from its earliest data"
                    ));
                    match self.store.reset_cursor_after_gap(stream, &message, &iso(now_ms())) {
                        Ok(()) => self.poll_stream(stream).await,
                        Err(e) => Err(e.to_string()),
                    }
                }
            }
        };
        let now = now_ms();
        let state = &mut self.state[idx(stream)];
        match outcome {
            Ok(added) => {
                if added > 0 {
                    (self.log)(&format!("{stream}: +{added}"));
                }
                *state = PollState { last_success_at: now, last_error: None, last_attempt_at: now };
            }
            Err(message) => {
                (self.log)(&format!("{stream}: poll failed: {message}"));
                state.last_error = Some(message);
                state.last_attempt_at = now;
            }
        }
        let state = state.clone();
        if let Err(e) = self.store.record_poll(stream, &state) {
            (self.log)(&format!("{stream}: heartbeat write failed: {e}"));
        }
    }

    /// Poll both streams once, independently.
    pub async fn poll(&mut self) {
        for stream in Stream::ALL {
            self.poll_one(stream).await;
        }
    }

    /// Release the store (the database closes when it drops).
    pub fn into_store(self) -> Store {
        self.store
    }
}
