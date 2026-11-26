//! The writer: owned by the ingester, the only process that writes the index. Every write path of the
//! indexer is here.
//!
//! **Single writer, enforced.** [`Store::connect`] takes `pg_try_advisory_lock(WRITER_LOCK_KEY)` on its one
//! connection and holds it for the life of the process; a second writer fails with
//! [`StoreError::WriterLockHeld`]. That is what keeps `events.id` in ledger order: ids come from one
//! identity sequence and one connection commits them one transaction at a time, so a reader never sees a
//! higher id without every lower id of the stream that will ever exist. The lock lives on the session: if
//! the connection drops, the lock is gone, so the ingester treats a closed connection as fatal
//! ([`Store::is_closed`]) and exits for Kubernetes to restart it.

use std::time::Duration;

use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Statement};

use crate::db::DatabaseSettings;
use crate::error::{Result, StoreError};
use crate::migrate;
use crate::read;
use crate::{
    ALIVE_KEY, BindError, Coverage, GateStats, IndexedEvent, PackageChangedError, Page, PageQuery, PollState,
    SCHEMA_WRITER_KEY, Stream, StreamHealth, poll_key, truncate_utf16,
};

/// The advisory-lock key of the writer (per database: advisory locks are scoped to the database).
pub const WRITER_LOCK_KEY: i64 = 0x5355_4958_5752_0001;

/// The tables a backup schema copies (everything except `schema_migrations`).
pub const INDEX_TABLES: [&str; 7] =
    ["meta", "cursors", "events", "stream_stats", "gate_stats", "gate_commission", "heartbeats"];

/// How [`Store::connect_with`] behaves.
#[derive(Clone, Debug)]
pub struct ConnectOptions {
    /// How long to keep retrying the writer lock before failing with [`StoreError::WriterLockHeld`]
    /// (a restarted process meets its predecessor's session still closing).
    pub lock_wait: Duration,
    /// `application_name` in `pg_stat_activity`.
    pub application_name: &'static str,
}

impl Default for ConnectOptions {
    /// The default options: wait five seconds for the writer lock and show `sui-indexer-ingest` as the application name.
    fn default() -> Self {
        ConnectOptions { lock_wait: Duration::from_secs(5), application_name: "sui-indexer-ingest" }
    }
}

/// Six random bytes in hex: a new database generation.
pub(crate) fn new_generation() -> String {
    let mut b = [0u8; 6];
    getrandom::fill(&mut b).expect("the OS random source is available");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The prepared statements the writer reuses for every event, so each insert is one round trip per statement with no re-planning.
struct Statements {
    /// Insert one event row (ignoring a duplicate transaction digest and event index).
    insert_event: Statement,
    /// Bump the stream's counters and checkpoint range.
    bump_stream: Statement,
    /// Bump the per-gate count for the event kind.
    bump_gate: Statement,
    /// Add the minted commission to the gate's total.
    bump_commission: Statement,
    /// Store the stream's poll cursor.
    upsert_cursor: Statement,
    /// Store a heartbeat value (poll state or liveness).
    upsert_heartbeat: Statement,
    /// Store a `meta` value.
    upsert_meta: Statement,
    /// Count an undecodable event on the stream.
    note_undecodable: Statement,
}

/// The writer.
pub struct Store {
    /// The one connection to the database, as the owner role.
    client: Client,
    /// The current index generation.
    generation: String,
    /// The prepared statements.
    stmts: Statements,
}

/// A checkpoint as the `i64` the database stores; larger values are rejected rather than wrapped.
fn checkpoint_i64(c: u64) -> Result<i64> {
    i64::try_from(c).map_err(|_| StoreError::Data("checkpoint exceeds i64".into()))
}

/// A backup schema name: `bak_` and lower-case alphanumerics, so it is safe as an identifier.
fn backup_schema(label: &str) -> Result<&str> {
    if label.starts_with("bak_")
        && label.len() <= 40
        && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        Ok(label)
    } else {
        Err(StoreError::Data(format!("invalid backup schema name {label:?} (want bak_<lowercase alphanumerics>)")))
    }
}

/// Copy every index table into a new schema `schema`, inside `tx` (so the copy and what follows are one
/// atomic step). The copies keep the rows and column types, not constraints or indexes: restore with
/// `INSERT INTO <table> OVERRIDING SYSTEM VALUE SELECT * FROM <schema>.<table>`.
pub(crate) async fn backup_into(tx: &tokio_postgres::Transaction<'_>, schema: &str) -> Result<()> {
    let schema = backup_schema(schema)?;
    tx.batch_execute(&format!("CREATE SCHEMA {schema}")).await?;
    for t in INDEX_TABLES {
        tx.batch_execute(&format!("CREATE TABLE {schema}.{t} AS SELECT * FROM public.{t}")).await?;
    }
    Ok(())
}

impl Store {
    /// Connect as the database owner, take the writer lock, apply pending migrations and read or create
    /// the generation.
    pub async fn connect(settings: &DatabaseSettings) -> Result<Store> {
        Self::connect_with(settings, ConnectOptions::default()).await
    }

    /// Connect with explicit options: how long to wait for the writer lock and which application name to show in `pg_stat_activity`.
    pub async fn connect_with(settings: &DatabaseSettings, opts: ConnectOptions) -> Result<Store> {
        let mut client = settings.clone().with_application_name(opts.application_name).connect().await?;
        // The old writer may still be shutting down during a rollout, so the new one waits a little for its lock.
        let deadline = tokio::time::Instant::now() + opts.lock_wait;
        loop {
            // A session-level advisory lock makes this the only writer: two ingesters would race on the cursors and counters.
            let got: bool = client.query_one("SELECT pg_try_advisory_lock($1)", &[&WRITER_LOCK_KEY]).await?.get(0);
            if got {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                // Give up with a clear error instead of waiting for ever.
                return Err(StoreError::WriterLockHeld);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        // Apply pending migrations under the lock, so two processes never migrate at once.
        migrate::run(&mut client).await?;
        // A new database gets a new random generation; an existing one keeps its own.
        client
            .execute(
                "INSERT INTO meta (key, value) VALUES ('generation', $1) ON CONFLICT (key) DO NOTHING",
                &[&new_generation()],
            )
            .await?;
        client
            .execute(
                "INSERT INTO meta (key, value) VALUES ($1, 'postgres') ON CONFLICT (key) DO NOTHING",
                &[&SCHEMA_WRITER_KEY],
            )
            .await?;
        // Read back whichever generation is stored (ours if we just created it).
        let generation: String = client.query_one("SELECT value FROM meta WHERE key = 'generation'", &[]).await?.get(0);
        // Prepare every statement once on this connection; ingesting an event then reuses the plans.
        let stmts = Statements {
            // Idempotent insert: replaying a page after a crash adds no duplicate (the unique key is the digest and event index).
            insert_event: client
                .prepare(
                    "INSERT INTO events
                       (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind, gate_id, commission_mist)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                     ON CONFLICT (tx_digest, event_index) DO NOTHING",
                )
                .await?,
            // Counters are maintained as events arrive, so coverage never needs a scan; the checkpoint range only widens.
            bump_stream: client
                .prepare(
                    "INSERT INTO stream_stats (stream, from_checkpoint, latest_checkpoint, events) VALUES ($1, $2, $2, 1)
                     ON CONFLICT (stream) DO UPDATE SET
                       from_checkpoint = LEAST(stream_stats.from_checkpoint, EXCLUDED.from_checkpoint),
                       latest_checkpoint = GREATEST(stream_stats.latest_checkpoint, EXCLUDED.latest_checkpoint),
                       events = stream_stats.events + 1",
                )
                .await?,
            // The per-gate count of each event kind.
            bump_gate: client
                .prepare(
                    "INSERT INTO gate_stats (gate_id, kind, n) VALUES ($1, $2, 1)
                     ON CONFLICT (gate_id, kind) DO UPDATE SET n = gate_stats.n + 1",
                )
                .await?,
            // The running commission total per gate, in `numeric` so a sum of u64 values cannot overflow.
            bump_commission: client
                .prepare(
                    "INSERT INTO gate_commission (gate_id, total) VALUES ($1, $2::bigint::numeric)
                     ON CONFLICT (gate_id) DO UPDATE SET total = gate_commission.total + EXCLUDED.total",
                )
                .await?,
            // The stream's position: where the next poll starts.
            upsert_cursor: client
                .prepare(
                    "INSERT INTO cursors (stream, cursor) VALUES ($1, $2)
                     ON CONFLICT (stream) DO UPDATE SET cursor = EXCLUDED.cursor",
                )
                .await?,
            // Poll state and liveness are stored as heartbeats the API and the health check read.
            upsert_heartbeat: client
                .prepare("INSERT INTO heartbeats (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
                .await?,
            // Values such as the bound packages and the generation.
            upsert_meta: client
                .prepare("INSERT INTO meta (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
                .await?,
            // An event that could not be decoded is counted, not dropped silently.
            note_undecodable: client
                .prepare(
                    "INSERT INTO stream_stats (stream, undecodable) VALUES ($1, $2)
                     ON CONFLICT (stream) DO UPDATE SET undecodable = stream_stats.undecodable + EXCLUDED.undecodable",
                )
                .await?,
        };
        Ok(Store { client, generation, stmts })
    }

    /// The underlying connection (tests and tooling use it directly).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Whether the connection (and with it the writer lock) is gone.
    pub fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    /// Random per index build; API cursors carry it, and a cursor from another generation is refused.
    pub fn generation(&self) -> &str {
        &self.generation
    }

    /// Bind each stream to the package (original id) it indexes. A stream recorded under a different
    /// package — or holding rows from before packages were recorded — would have to be cleared to
    /// re-read its new package. That discards history the full node may already have pruned, so it is
    /// refused with [`PackageChangedError`] unless the stream is listed in `confirm`; confirming
    /// first copies the whole index into the schema `backup` (`bak_<epoch ms>`, `CREATE TABLE … AS`, in
    /// the same transaction as the clearing: both happen or neither does), then clears the stream's
    /// rows, cursor and counters and rotates the generation (so cursors served from the old index are
    /// refused). Call once at startup, before ingesting.
    ///
    /// Returns the streams that were cleared.
    pub async fn bind_packages(
        &mut self,
        packages: &[(Stream, String)],
        confirm: &[Stream],
        backup: Option<&str>,
    ) -> std::result::Result<Vec<Stream>, BindError> {
        match self.bind_packages_inner(packages, confirm, backup).await {
            Ok(Ok(cleared)) => Ok(cleared),
            Ok(Err(refused)) => Err(BindError::PackageChanged(refused)),
            Err(e) => Err(BindError::Store(e)),
        }
    }

    /// Compare the stored packages with the configured ones and bind them; a changed package is refused unless the stream is confirmed for re-indexing, in which case its history is deleted (after the optional backup) and the generation changes.
    async fn bind_packages_inner(
        &mut self,
        packages: &[(Stream, String)],
        confirm: &[Stream],
        backup: Option<&str>,
    ) -> Result<std::result::Result<Vec<Stream>, PackageChangedError>> {
        // The streams whose recorded package differs from the configured one (or that hold rows from before packages were recorded).
        let mut pending = Vec::new();
        for (stream, pkg) in packages {
            let recorded = self.meta(&format!("package:{stream}")).await?;
            if recorded.as_deref() == Some(pkg.as_str()) {
                continue;
            }
            let rows: i64 = self
                .client
                .query_one("SELECT count(*) FROM events WHERE stream = $1", &[&stream.as_str()])
                .await?
                .get(0);
            if recorded.is_some() || rows > 0 || self.cursor(*stream).await?.is_some() {
                pending.push(*stream);
            }
        }
        // Only streams the operator listed may be cleared.
        let confirmed: Vec<Stream> = pending.iter().copied().filter(|s| confirm.contains(s)).collect();
        if pending.len() != confirmed.len() {
            let refused = pending.into_iter().filter(|s| !confirmed.contains(s)).collect();
            return Ok(Err(PackageChangedError { streams: refused }));
        }
        let mut generation = None;
        {
            let tx = self.client.transaction().await?;
            if !confirmed.is_empty()
                && let Some(schema) = backup
            {
                backup_into(&tx, schema).await?;
            }
            for stream in &confirmed {
                let s = stream.as_str();
                tx.execute("DELETE FROM events WHERE stream = $1", &[&s]).await?;
                tx.execute("DELETE FROM cursors WHERE stream = $1", &[&s]).await?;
                tx.execute("DELETE FROM stream_stats WHERE stream = $1", &[&s]).await?;
                if *stream == Stream::AccessGate {
                    tx.batch_execute("DELETE FROM gate_stats; DELETE FROM gate_commission").await?;
                }
            }
            for (stream, pkg) in packages {
                tx.execute(&self.stmts.upsert_meta, &[&format!("package:{stream}"), pkg]).await?;
            }
            if !confirmed.is_empty() {
                let g = new_generation();
                tx.execute(&self.stmts.upsert_meta, &[&"generation", &g]).await?;
                generation = Some(g);
            }
            tx.commit().await?;
        }
        if let Some(g) = generation {
            self.generation = g;
        }
        Ok(Ok(confirmed))
    }

    /// The stored ledger cursor of `stream` (base64 of the full node's watermark cursor bytes).
    pub async fn cursor(&self, stream: Stream) -> Result<Option<String>> {
        let rows = self.client.query("SELECT cursor FROM cursors WHERE stream = $1", &[&stream.as_str()]).await?;
        Ok(rows.first().and_then(|r| r.get::<_, Option<String>>(0)))
    }

    /// Insert a page of events and advance the stream's cursor in one transaction, so a crash never
    /// records a cursor past events it did not store. Re-inserting an event is a no-op (it may consume
    /// an identity number: ids are ordered, not gapless).
    ///
    /// Returns how many events were new.
    pub async fn ingest(&mut self, stream: Stream, events: &[IndexedEvent], cursor: Option<&str>) -> Result<usize> {
        let tx = self.client.transaction().await?;
        let st = &self.stmts;
        let stream_name = stream.as_str();
        let mut added = 0;
        for e in events {
            let ckpt = e.checkpoint.map(checkpoint_i64).transpose()?;
            let index = i64::from(e.event_index);
            let changed = tx
                .execute(
                    &st.insert_event,
                    &[
                        &stream_name,
                        &e.tx_digest,
                        &index,
                        &ckpt,
                        &e.sender,
                        &e.event_type,
                        &e.bcs,
                        &e.kind,
                        &e.gate_id,
                        &e.commission_mist,
                    ],
                )
                .await?;
            if changed == 0 {
                continue; // already stored: counters were bumped then
            }
            added += 1;
            tx.execute(&st.bump_stream, &[&stream_name, &ckpt]).await?;
            if stream == Stream::AccessGate
                && let Some(gate) = &e.gate_id
            {
                tx.execute(&st.bump_gate, &[gate, &e.kind]).await?;
                if let Some(commission) = e.commission_mist {
                    tx.execute(&st.bump_commission, &[gate, &commission]).await?;
                }
            }
        }
        if let Some(cursor) = cursor {
            tx.execute(&st.upsert_cursor, &[&stream_name, &cursor]).await?;
        }
        tx.commit().await?;
        Ok(added)
    }

    /// Record `n` events skipped because they did not decode (the page still advances past them).
    pub async fn note_undecodable(&self, stream: Stream, n: i64) -> Result<()> {
        self.client.execute(&self.stmts.note_undecodable, &[&stream.as_str(), &n]).await?;
        Ok(())
    }

    /// The highest checkpoint of an event stored for `stream`, if any: where a refused cursor is
    /// resumed from (re-reading that checkpoint is harmless, inserting an event twice is a no-op).
    pub async fn last_checkpoint(&self, stream: Stream) -> Result<Option<u64>> {
        let row =
            self.client.query_one("SELECT max(checkpoint) FROM events WHERE stream = $1", &[&stream.as_str()]).await?;
        Ok(row.get::<_, Option<i64>>(0).and_then(|c| u64::try_from(c).ok()))
    }

    /// The node refused the stored cursor although the data after it is still served (its cursor
    /// format changed): forget the cursor so the next poll resumes from the last stored checkpoint.
    /// Not a gap, so it is not counted as one; `at_iso` and the reason go to `meta resume:<stream>`.
    pub async fn drop_cursor_to_resume(&mut self, stream: Stream, reason: &str, at_iso: &str) -> Result<()> {
        let reason: String = truncate_utf16(reason, 200);
        let value = serde_json::json!({ "at": at_iso, "reason": reason }).to_string();
        let tx = self.client.transaction().await?;
        tx.execute("DELETE FROM cursors WHERE stream = $1", &[&stream.as_str()]).await?;
        tx.execute(&self.stmts.upsert_meta, &[&format!("resume:{stream}"), &value]).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The stored cursor points below what the full node still serves (its history was pruned while
    /// no event arrived to move the cursor). The events in between are gone for good, so restart from
    /// the node's earliest data: rows already stored are untouched (re-inserting is a no-op). Counted
    /// as a gap; `at_iso` and the reason (first 200 UTF-16 units) go to `meta gap:<stream>`.
    pub async fn reset_cursor_after_gap(&mut self, stream: Stream, reason: &str, at_iso: &str) -> Result<()> {
        let reason: String = truncate_utf16(reason, 200);
        let value = serde_json::json!({ "at": at_iso, "reason": reason }).to_string();
        let tx = self.client.transaction().await?;
        tx.execute("DELETE FROM cursors WHERE stream = $1", &[&stream.as_str()]).await?;
        tx.execute(
            "INSERT INTO stream_stats (stream, gaps) VALUES ($1, 1)
             ON CONFLICT (stream) DO UPDATE SET gaps = stream_stats.gaps + 1",
            &[&stream.as_str()],
        )
        .await?;
        tx.execute(&self.stmts.upsert_meta, &[&format!("gap:{stream}"), &value]).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Write the stream's heartbeat (`heartbeats poll:<stream>`).
    pub async fn record_poll(&self, stream: Stream, state: &PollState) -> Result<()> {
        let value = serde_json::to_string(state).expect("PollState serialises");
        self.client.execute(&self.stmts.upsert_heartbeat, &[&poll_key(stream), &value]).await?;
        Ok(())
    }

    /// Write the ingester's liveness heartbeat (`heartbeats ingest:alive`, epoch ms).
    pub async fn touch_alive(&self, now_ms: i64) -> Result<()> {
        self.client.execute(&self.stmts.upsert_heartbeat, &[&ALIVE_KEY, &now_ms.to_string()]).await?;
        Ok(())
    }

    // Read paths shared with the API (the writer's tests use them too).

    /// One page of a stream (see the shared read path).
    pub async fn page(&self, stream: Stream, q: &PageQuery) -> Result<Page> {
        read::page(&self.client, stream, q).await
    }
    /// The newest row id of a stream (0 when empty).
    pub async fn head_id(&self, stream: Stream) -> Result<i64> {
        read::head_id(&self.client, stream).await
    }
    /// A stream's coverage counters.
    pub async fn coverage(&self, stream: Stream) -> Result<Coverage> {
        read::coverage(&self.client, stream).await
    }
    /// A stream's undecodable and gap counters.
    pub async fn health(&self, stream: Stream) -> Result<StreamHealth> {
        read::health(&self.client, stream).await
    }
    /// One gate's counts and commission.
    pub async fn gate_stats(&self, gate_id: &str) -> Result<GateStats> {
        read::gate_stats(&self.client, gate_id).await
    }
    /// A stream's persisted poll state.
    pub async fn poll_state(&self, stream: Stream) -> Result<PollState> {
        read::poll_state(&self.client, stream).await
    }
    /// A value from the `meta` table.
    pub async fn meta(&self, key: &str) -> Result<Option<String>> {
        read::meta(&self.client, key).await
    }
    /// A value from the `heartbeats` table.
    pub async fn heartbeat(&self, key: &str) -> Result<Option<String>> {
        read::heartbeat(&self.client, key).await
    }

    /// Recount the counters from the rows and compare them with the maintained ones; the differences
    /// found (empty when they agree). The ingest transaction keeps them exact; this is the check the
    /// tests use.
    pub async fn verify_counters(&self) -> Result<Vec<String>> {
        verify_counters(&self.client).await
    }

    /// Run an arbitrary parameterless statement (tests).
    pub async fn execute_batch(&self, sql: &str) -> Result<()> {
        self.client.batch_execute(sql).await?;
        Ok(())
    }

    /// Run a query returning rows (tests).
    pub async fn query(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Vec<tokio_postgres::Row>> {
        Ok(self.client.query(sql, params).await?)
    }
}

/// See [`Store::verify_counters`].
pub(crate) async fn verify_counters(c: &impl tokio_postgres::GenericClient) -> Result<Vec<String>> {
    let mut diffs = Vec::new();
    // stream_stats: events, from/latest checkpoint.
    let truth = c
        .query(
            "SELECT stream, min(checkpoint), max(checkpoint), count(*) FROM events GROUP BY stream ORDER BY stream",
            &[],
        )
        .await?;
    let kept = c
        .query("SELECT stream, from_checkpoint, latest_checkpoint, events FROM stream_stats ORDER BY stream", &[])
        .await?;
    let mut kept_map: std::collections::BTreeMap<String, (Option<i64>, Option<i64>, i64)> =
        kept.iter().map(|r| (r.get(0), (r.get(1), r.get(2), r.get(3)))).collect();
    for r in &truth {
        let s: String = r.get(0);
        let want = (r.get::<_, Option<i64>>(1), r.get::<_, Option<i64>>(2), r.get::<_, i64>(3));
        match kept_map.remove(&s) {
            Some(got) if got == want => {}
            got => diffs.push(format!("stream_stats[{s}]: kept {got:?}, recount {want:?}")),
        }
    }
    for (s, got) in kept_map {
        if got.2 != 0 {
            diffs.push(format!("stream_stats[{s}]: kept {got:?}, but the stream has no rows"));
        }
    }
    // gate_stats and gate_commission (access-gate stream only).
    let truth: Vec<(String, String, i64)> = c
        .query(
            "SELECT gate_id, kind, count(*) FROM events WHERE stream = 'access-gate' AND gate_id IS NOT NULL
             GROUP BY gate_id, kind ORDER BY gate_id, kind",
            &[],
        )
        .await?
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();
    let kept: Vec<(String, String, i64)> = c
        .query("SELECT gate_id, kind, n FROM gate_stats ORDER BY gate_id, kind", &[])
        .await?
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();
    if truth != kept {
        diffs.push(format!("gate_stats: kept {} rows, recount {} rows (or values differ)", kept.len(), truth.len()));
    }
    let truth: Vec<(String, String)> = c
        .query(
            "SELECT gate_id, sum(commission_mist)::numeric(39,0)::text FROM events
             WHERE stream = 'access-gate' AND gate_id IS NOT NULL AND commission_mist IS NOT NULL
             GROUP BY gate_id ORDER BY gate_id",
            &[],
        )
        .await?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    let kept: Vec<(String, String)> = c
        .query("SELECT gate_id, total::text FROM gate_commission ORDER BY gate_id", &[])
        .await?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    if truth != kept {
        diffs.push(format!(
            "gate_commission: kept {} rows, recount {} rows (or totals differ)",
            kept.len(),
            truth.len()
        ));
    }
    Ok(diffs)
}

/// The ingester's liveness heartbeat (`heartbeats ingest:alive`, epoch ms) over a fresh short-lived
/// connection: the exec liveness probe's read. `None` when no heartbeat was recorded yet.
pub async fn read_alive(settings: &DatabaseSettings) -> Result<Option<i64>> {
    let client = settings.clone().with_application_name("sui-indexer-healthcheck").connect().await?;
    let rows = client.query("SELECT value FROM heartbeats WHERE key = $1", &[&ALIVE_KEY]).await?;
    Ok(rows.first().and_then(|r| r.get::<_, String>(0).parse().ok()))
}
