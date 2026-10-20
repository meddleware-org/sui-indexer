//! The read side the API uses: a small connection pool over the SELECT-only role.
//!
//! - **Read-only twice.** The database role has no write grant and `default_transaction_read_only = on`;
//!   in addition every pooled connection starts with `SET default_transaction_read_only = on`, and every
//!   request runs inside `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY`, so several reads see one
//!   snapshot (as the SQLite read transaction did).
//! - **Never migrates.** The API role cannot run DDL. [`Reader::check`] reports `NotReady` until
//!   `schema_migrations` is at [`SCHEMA_VERSION`] (or newer: migrations keep the previous queries
//!   working) and the ingester has written the generation.
//! - **Cancellation-safe.** A [`Session`] dropped without [`Session::end`] (a request timed out
//!   mid-transaction) discards its connection instead of returning it to the pool with a transaction open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use deadpool_postgres::{Hook, HookError, Manager, ManagerConfig, Object, Pool, RecyclingMethod, Runtime};
use tokio_postgres::NoTls;

use crate::db::DatabaseSettings;
use crate::error::{Result, StoreError};
use crate::migrate::SCHEMA_VERSION;
use crate::read;
use crate::{Coverage, GateStats, Page, PageQuery, PollState, Stream, StreamHealth};

/// How long a request waits for a free pooled connection before the API answers 503.
pub const POOL_WAIT: Duration = Duration::from_secs(3);

/// Why the index cannot be read right now.
#[derive(Debug)]
pub enum ReadError {
    /// The schema (or the generation) is not there yet: the ingester has not migrated and started.
    NotReady(String),
    /// No connection could be had (database down, pool exhausted, TLS failure).
    Unavailable(String),
    /// A query failed.
    Store(StoreError),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::NotReady(m) => write!(f, "index not ready: {m}"),
            ReadError::Unavailable(m) => write!(f, "database unavailable: {m}"),
            ReadError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ReadError {}

impl From<StoreError> for ReadError {
    fn from(e: StoreError) -> Self {
        ReadError::Store(e)
    }
}

impl From<tokio_postgres::Error> for ReadError {
    fn from(e: tokio_postgres::Error) -> Self {
        ReadError::Store(StoreError::Pg(e))
    }
}

/// The API's handle on the database: a lazily connecting pool.
pub struct Reader {
    pool: Pool,
    ready: AtomicBool,
}

impl Reader {
    /// Build the pool (no connection is opened yet, so the API starts and answers 503 while the database
    /// or the ingester is not up). Fails only on unusable TLS material.
    pub fn new(settings: &DatabaseSettings, pool_size: usize) -> Result<Reader> {
        let settings = settings.clone().with_application_name("sui-indexer-api");
        let mgr_cfg = ManagerConfig { recycling_method: RecyclingMethod::Fast };
        let manager = match settings.tls()? {
            Some(tls) => Manager::from_config(settings.pg_config().clone(), tls, mgr_cfg),
            None => Manager::from_config(settings.pg_config().clone(), NoTls, mgr_cfg),
        };
        let pool = Pool::builder(manager)
            .max_size(pool_size.max(1))
            .wait_timeout(Some(POOL_WAIT))
            .create_timeout(Some(crate::db::CONNECT_TIMEOUT))
            .recycle_timeout(Some(Duration::from_secs(2)))
            .runtime(Runtime::Tokio1)
            .post_create(Hook::async_fn(|client, _| {
                Box::pin(async move {
                    client
                        .batch_execute(
                            "SET default_transaction_read_only = on; SET plan_cache_mode = force_custom_plan",
                        )
                        .await
                        .map_err(HookError::Backend)
                })
            }))
            .build()
            .map_err(|e| StoreError::Config(format!("cannot build the connection pool: {e}")))?;
        Ok(Reader { pool, ready: AtomicBool::new(false) })
    }

    async fn conn(&self) -> std::result::Result<Object, ReadError> {
        self.pool.get().await.map_err(|e| ReadError::Unavailable(e.to_string()))
    }

    /// Whether the schema is there at the version this build needs and the ingester has recorded a
    /// generation. Always asks the database (`/readyz`).
    pub async fn check(&self) -> std::result::Result<(), ReadError> {
        let conn = self.conn().await?;
        let rows = match conn
            .query(
                "SELECT (SELECT max(version) FROM schema_migrations), (SELECT value FROM meta WHERE key = 'generation')",
                &[],
            )
            .await
        {
            Ok(rows) => rows,
            Err(e) if e.code().is_some_and(|c| c.code() == "42P01") => {
                self.ready.store(false, Ordering::Relaxed);
                return Err(ReadError::NotReady("schema not created yet".into()));
            }
            Err(e) => return Err(e.into()),
        };
        let version: Option<i32> = rows[0].get(0);
        let generation: Option<String> = rows[0].get(1);
        let verdict = match (version, generation) {
            (Some(v), Some(_)) if v >= SCHEMA_VERSION => Ok(()),
            (Some(v), _) if v < SCHEMA_VERSION => {
                Err(ReadError::NotReady(format!("schema version {v}, this build needs {SCHEMA_VERSION}")))
            }
            (None, _) => Err(ReadError::NotReady("no migration applied yet".into())),
            (_, None) => Err(ReadError::NotReady("no generation recorded yet".into())),
            (Some(_), Some(_)) => Ok(()),
        };
        self.ready.store(verdict.is_ok(), Ordering::Relaxed);
        verdict
    }

    /// Whether the last check found the index ready.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// Begin a read-only snapshot on a pooled connection. Checks readiness first until it has once
    /// succeeded (a ready index stays ready: a migration only adds).
    pub async fn session(&self) -> std::result::Result<Session, ReadError> {
        if !self.is_ready() {
            self.check().await?;
        }
        let conn = self.conn().await?;
        conn.batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").await?;
        Ok(Session { conn: Some(conn) })
    }

    /// Connections open now and in use (monitoring and tests).
    pub fn pool_size(&self) -> (usize, usize) {
        let s = self.pool.status();
        (s.size, s.size.saturating_sub(s.available))
    }
}

/// One read-only snapshot of the index. End it with [`Session::end`].
pub struct Session {
    conn: Option<Object>,
}

impl Session {
    fn c(&self) -> &Object {
        self.conn.as_ref().expect("the session is open")
    }

    /// The current generation (read per request: the ingester rotates it on a confirmed reindex).
    pub async fn generation(&self) -> Result<Option<String>> {
        read::meta(&**self.c(), "generation").await
    }
    pub async fn page(&self, stream: Stream, q: &PageQuery) -> Result<Page> {
        read::page(&**self.c(), stream, q).await
    }
    pub async fn head_id(&self, stream: Stream) -> Result<i64> {
        read::head_id(&**self.c(), stream).await
    }
    pub async fn coverage(&self, stream: Stream) -> Result<Coverage> {
        read::coverage(&**self.c(), stream).await
    }
    pub async fn health(&self, stream: Stream) -> Result<StreamHealth> {
        read::health(&**self.c(), stream).await
    }
    pub async fn gate_stats(&self, gate_id: &str) -> Result<GateStats> {
        read::gate_stats(&**self.c(), gate_id).await
    }
    pub async fn poll_state(&self, stream: Stream) -> Result<PollState> {
        read::poll_state(&**self.c(), stream).await
    }
    pub async fn meta(&self, key: &str) -> Result<Option<String>> {
        read::meta(&**self.c(), key).await
    }

    /// Attempt a write through this connection (it must fail: tests).
    pub async fn try_write(&self) -> Result<u64> {
        Ok(self.c().execute("INSERT INTO meta (key, value) VALUES ('probe', 'x')", &[]).await?)
    }

    /// Run `sql` and return the first column of the first row as text (tests).
    #[cfg(any(test, feature = "testing"))]
    pub async fn query_for_test(&self, sql: &str) -> String {
        let rows = self.c().simple_query(sql).await.expect("query");
        rows.iter()
            .find_map(|m| match m {
                tokio_postgres::SimpleQueryMessage::Row(r) => r.get(0).map(str::to_owned),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// End the snapshot and return the connection to the pool. If the connection cannot be reset it is
    /// discarded instead.
    pub async fn end(mut self) {
        if let Some(conn) = self.conn.take()
            && conn.batch_execute("COMMIT").await.is_err()
        {
            drop(Object::take(conn));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Reached without `end`: the request was cancelled (timeout, client gone) or failed with the
        // transaction open. The connection must not go back to the pool in that state.
        if let Some(conn) = self.conn.take() {
            drop(Object::take(conn));
        }
    }
}
