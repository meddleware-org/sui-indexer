//! The read side the API uses: a small connection pool over the SELECT-only role.
//!
//! - **One statement per request, one snapshot.** Every request is answered by a single SQL statement (the
//!   events page and the facts its response carries are one `SELECT`; see `read::events_sql`), so what a
//!   response says is consistent without a transaction, costs one round trip, and can never leave a pooled
//!   connection inside an open transaction (a cancelled request leaves nothing behind).
//! - **Read-only twice.** The database role has no write grant and `default_transaction_read_only = on`;
//!   in addition every pooled connection starts with `SET default_transaction_read_only = on`.
//! - **Never migrates.** The API role cannot run DDL. [`Reader::check`] reports `NotReady` until
//!   `schema_migrations` is at [`SCHEMA_VERSION`] (or newer: migrations keep the previous queries
//!   working) and the ingester has written the generation.
//! - **Custom plans.** Pooled connections force `plan_cache_mode = force_custom_plan`: a generic plan of the
//!   one-kind page walks the primary key, right for a common kind and a table walk for a rare one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use deadpool_postgres::{Hook, HookError, Manager, ManagerConfig, Object, Pool, RecyclingMethod, Runtime};
use tokio_postgres::NoTls;

use crate::db::DatabaseSettings;
use crate::error::{Result, StoreError};
use crate::migrate::SCHEMA_VERSION;
use crate::read::{self, CoverageSnapshot, EventsSnapshot, GateSnapshot};
use crate::{PageQuery, PollState, Stream};

/// What every pooled connection runs when it is created.
pub(crate) const SESSION_SETUP: &str =
    "SET default_transaction_read_only = on; SET plan_cache_mode = force_custom_plan";

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

/// An error and its causes, `a: b: c` (the pool's own message hides why a connection failed).
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
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
                Box::pin(async move { client.batch_execute(SESSION_SETUP).await.map_err(HookError::Backend) })
            }))
            .build()
            .map_err(|e| StoreError::Config(format!("cannot build the connection pool: {e}")))?;
        Ok(Reader { pool, ready: AtomicBool::new(false) })
    }

    /// A pooled connection, once the index is known to be ready (the schema is checked until it has been
    /// found ready once: a ready index stays ready, a migration only adds).
    async fn conn(&self) -> std::result::Result<Object, ReadError> {
        self.ensure_ready().await?;
        self.pool.get().await.map_err(|e| ReadError::Unavailable(chain(&e)))
    }

    /// Whether the schema is there at the version this build needs and the ingester has recorded a
    /// generation. Always asks the database (`/readyz`).
    pub async fn check(&self) -> std::result::Result<(), ReadError> {
        let conn = self.pool.get().await.map_err(|e| ReadError::Unavailable(chain(&e)))?;
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
            Err(e) => return Err(StoreError::Pg(e).into()),
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

    /// Fail with `NotReady`/`Unavailable` unless the index can be read (no query once it has been found ready).
    pub async fn ensure_ready(&self) -> std::result::Result<(), ReadError> {
        if self.is_ready() { Ok(()) } else { self.check().await }
    }

    /// The current generation (the ingester rotates it on a confirmed reindex).
    pub async fn generation(&self) -> std::result::Result<Option<String>, ReadError> {
        let conn = self.conn().await?;
        Ok(read::meta(&**conn, "generation").await?)
    }

    /// One page of `stream` with the facts the response carries (the newest id for the ETag, the stream's
    /// counters, its poll heartbeat, the generation), all from one statement.
    pub async fn events(&self, stream: Stream, q: &PageQuery) -> std::result::Result<EventsSnapshot, ReadError> {
        let conn = self.conn().await?;
        Ok(read::events(&**conn, stream, q).await?)
    }

    /// The generation and both streams' counters, from one statement.
    pub async fn coverage(&self) -> std::result::Result<CoverageSnapshot, ReadError> {
        let conn = self.conn().await?;
        Ok(read::coverage_snapshot(&**conn).await?)
    }

    /// One gate's counters and commission, and where coverage starts, from one statement.
    pub async fn gate(&self, gate_id: &str) -> std::result::Result<GateSnapshot, ReadError> {
        let conn = self.conn().await?;
        Ok(read::gate_snapshot(&**conn, gate_id).await?)
    }

    /// Both streams' poll heartbeats (`/healthz`), from one statement.
    pub async fn polls(&self) -> std::result::Result<[PollState; 2], ReadError> {
        let conn = self.conn().await?;
        Ok(read::poll_states(&**conn).await?)
    }

    /// Attempt a write through a pooled connection (it must fail: tests).
    #[cfg(any(test, feature = "testing"))]
    pub async fn try_write(&self) -> Result<u64> {
        let conn = self.conn().await.map_err(|e| StoreError::Data(e.to_string()))?;
        Ok(conn.execute("INSERT INTO meta (key, value) VALUES ('probe', 'x')", &[]).await?)
    }

    /// Run `sql` on a pooled connection and return the first column of the first row as text (tests).
    #[cfg(any(test, feature = "testing"))]
    pub async fn query_for_test(&self, sql: &str) -> String {
        let conn = self.pool.get().await.expect("a pooled connection");
        let rows = conn.simple_query(sql).await.expect("query");
        rows.iter()
            .find_map(|m| match m {
                tokio_postgres::SimpleQueryMessage::Row(r) => r.get(0).map(str::to_owned),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Connections open now and in use (monitoring and tests).
    pub fn pool_size(&self) -> (usize, usize) {
        let s = self.pool.status();
        (s.size, s.size.saturating_sub(s.available))
    }
}
