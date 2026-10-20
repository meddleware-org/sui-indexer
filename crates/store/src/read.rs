//! Queries both the writer and the API's sessions run. None of them scans a stream: counters are
//! maintained at ingest. Which index serves a page is part of the cost model (the query-plan tests in
//! `index_tests.rs` pin it).

use tokio_postgres::Row;
use tokio_postgres::types::ToSql;

use crate::error::{Result, StoreError};
use crate::{Coverage, EventRow, GateStats, Page, PageQuery, PollState, Stream, StreamHealth, poll_key};

/// A way to run a query and get its rows: a plain client (the writer) or a pooled one that caches its
/// prepared statements (the API).
pub(crate) trait Query {
    fn rows(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl std::future::Future<Output = std::result::Result<Vec<Row>, tokio_postgres::Error>> + Send;
}

impl Query for tokio_postgres::Client {
    async fn rows(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> std::result::Result<Vec<Row>, tokio_postgres::Error> {
        self.query(sql, params).await
    }
}

impl Query for deadpool_postgres::ClientWrapper {
    async fn rows(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> std::result::Result<Vec<Row>, tokio_postgres::Error> {
        let statement = self.prepare_cached(sql).await?;
        self.query(&statement, params).await
    }
}

/// A bind value of the page query (kept as data so the tests can also render it as a literal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Arg {
    Text(String),
    Int(i64),
    TextArray(Vec<String>),
}

impl Arg {
    pub(crate) fn as_sql(&self) -> &(dyn ToSql + Sync) {
        match self {
            Arg::Text(v) => v,
            Arg::Int(v) => v,
            Arg::TextArray(v) => v,
        }
    }

    /// The value as a SQL literal (for `EXPLAIN ANALYZE` in the tests, which cannot bind).
    #[cfg(test)]
    pub(crate) fn literal(&self) -> String {
        let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
        match self {
            Arg::Text(v) => q(v),
            Arg::Int(v) => v.to_string(),
            Arg::TextArray(v) => format!("ARRAY[{}]::text[]", v.iter().map(|s| q(s)).collect::<Vec<_>>().join(", ")),
        }
    }
}

const PAGE_COLUMNS: &str = "id, event_type, sender, bcs, checkpoint, tx_digest, event_index";

/// The SQL and arguments of [`page`] (also what the query-plan tests explain). Parameters are numbered:
/// `$1` stream, then (when present) `$2` before, `$3` take, the rest are filters.
pub(crate) fn page_sql(stream: Stream, q: &PageQuery) -> (String, Vec<Arg>) {
    // Distinct kinds: a repeated one would return its rows twice from the merged seeks.
    let mut distinct: Vec<&String> = Vec::new();
    for k in q.kinds.iter().flatten() {
        if !distinct.contains(&k) {
            distinct.push(k);
        }
    }
    let kinds = Some(distinct.as_slice()).filter(|k| !k.is_empty());
    let take = i64::from(q.limit) + 1;
    let mut args = vec![Arg::Text(stream.as_str().to_owned())];
    let before = q.before_id.map(|b| {
        args.push(Arg::Int(b));
        args.len()
    });
    args.push(Arg::Int(take));
    let take_n = args.len();
    let before_clause = before.map(|n| format!(" AND id < ${n}")).unwrap_or_default();

    if let (Some(kinds), None) = (kinds, &q.gate_id)
        && kinds.len() > 1
    {
        // One bounded, ordered seek per kind, merged: cost O(kinds x limit), independent of history.
        let mut arms = Vec::with_capacity(kinds.len());
        for k in kinds {
            args.push(Arg::Text((*k).clone()));
            arms.push(format!(
                "(SELECT {PAGE_COLUMNS} FROM events WHERE stream = $1 AND kind = ${}{before_clause} ORDER BY id DESC LIMIT ${take_n})",
                args.len()
            ));
        }
        return (
            format!("SELECT * FROM ({}) AS seeks ORDER BY id DESC LIMIT ${take_n}", arms.join(" UNION ALL ")),
            args,
        );
    }
    let mut sql = format!("SELECT {PAGE_COLUMNS} FROM events WHERE stream = $1");
    if let Some(g) = &q.gate_id {
        args.push(Arg::Text(g.clone()));
        sql.push_str(&format!(" AND gate_id = ${}", args.len()));
    }
    if let Some(kinds) = kinds {
        if q.gate_id.is_some() {
            // With a gate the kind is a plain row filter: `kind || ''` is not an indexed expression, so
            // the planner (even on a generic plan) keeps to `events_gate` instead of walking a common
            // kind on `events_kind` to find one gate's rows.
            args.push(Arg::TextArray(kinds.iter().map(|k| (*k).clone()).collect()));
            sql.push_str(&format!(" AND (kind || '') = ANY(${})", args.len()));
        } else {
            args.push(Arg::Text(kinds[0].clone()));
            sql.push_str(&format!(" AND kind = ${}", args.len()));
        }
    }
    sql.push_str(&before_clause);
    sql.push_str(&format!(" ORDER BY id DESC LIMIT ${take_n}"));
    (sql, args)
}

/// The statement of one events request: the head's facts and the page, read by ONE statement, so they are
/// one snapshot (and one round trip). `m` is a single row (generation, newest id of the stream, the stream's
/// counters, its poll heartbeat); the page is `LEFT JOIN LATERAL`-ed onto it, so an empty page still returns
/// the facts. Parameters: those of [`page_sql`], then the heartbeat key.
pub(crate) fn events_sql(stream: Stream, q: &PageQuery) -> (String, Vec<Arg>) {
    let (page, mut args) = page_sql(stream, q);
    args.push(Arg::Text(poll_key(stream)));
    let key = args.len();
    let sql = format!(
        "SELECT m.generation, m.head_id, m.from_checkpoint, m.latest_checkpoint, m.events, m.poll,
                p.id, p.event_type, p.sender, p.bcs, p.checkpoint, p.tx_digest, p.event_index
         FROM (SELECT (SELECT value FROM meta WHERE key = 'generation') AS generation,
                      (SELECT max(id) FROM events WHERE stream = $1) AS head_id,
                      s.from_checkpoint, s.latest_checkpoint, s.events,
                      (SELECT value FROM heartbeats WHERE key = ${key}) AS poll
               FROM (SELECT 1) AS one LEFT JOIN stream_stats s ON s.stream = $1) AS m
         LEFT JOIN LATERAL ({page}) AS p ON true
         ORDER BY p.id DESC NULLS LAST"
    );
    (sql, args)
}

/// Everything an events response needs, from one snapshot.
#[derive(Clone, Debug)]
pub struct EventsSnapshot {
    pub generation: Option<String>,
    /// The newest row id of the stream (0 when empty): the head page's ETag.
    pub head_id: i64,
    pub coverage: Coverage,
    pub poll: PollState,
    pub page: Page,
}

pub(crate) async fn events(c: &impl Query, stream: Stream, q: &PageQuery) -> Result<EventsSnapshot> {
    let (sql, args) = events_sql(stream, q);
    let params: Vec<&(dyn ToSql + Sync)> = args.iter().map(Arg::as_sql).collect();
    let rows = c.rows(&sql, &params).await?;
    let first = rows.first().ok_or_else(|| StoreError::Data("the events statement returned no row".into()))?;
    let coverage = match first.get::<_, Option<i64>>(4) {
        Some(n) => Coverage {
            from_checkpoint: first.get::<_, Option<i64>>(2).map(|v| v.to_string()),
            latest_checkpoint: first.get::<_, Option<i64>>(3).map(|v| v.to_string()),
            events: n,
        },
        None => Coverage { from_checkpoint: None, latest_checkpoint: None, events: 0 },
    };
    let poll =
        first.get::<_, Option<String>>(5).and_then(|v| serde_json::from_str::<PollState>(&v).ok()).unwrap_or_default();
    let mut page_rows: Vec<EventRow> = rows
        .iter()
        .filter_map(|r| {
            r.get::<_, Option<i64>>(6).map(|id| EventRow {
                id,
                event_type: r.get(7),
                sender: r.get(8),
                bcs: r.get(9),
                checkpoint: r.get::<_, Option<i64>>(10).map(|c| c.to_string()),
                transaction_digest: r.get(11),
                event_index: r.get(12),
            })
        })
        .collect();
    let more = page_rows.len() > q.limit as usize;
    page_rows.truncate(q.limit as usize);
    let next = if more { page_rows.last().map(|r| r.id) } else { None };
    Ok(EventsSnapshot {
        generation: first.get(0),
        head_id: first.get::<_, Option<i64>>(1).unwrap_or(0),
        coverage,
        poll,
        page: Page { rows: page_rows, next },
    })
}

/// Both streams' counters and the generation, from one statement.
#[derive(Clone, Debug)]
pub struct CoverageSnapshot {
    pub generation: Option<String>,
    pub streams: [(Stream, Coverage, StreamHealth); 2],
}

pub(crate) async fn coverage_snapshot(c: &impl Query) -> Result<CoverageSnapshot> {
    let rows = c
        .rows(
            "SELECT g.value, s.stream, s.from_checkpoint, s.latest_checkpoint, s.events, s.undecodable, s.gaps
             FROM (SELECT 1) AS one LEFT JOIN meta g ON g.key = 'generation' LEFT JOIN stream_stats s ON true",
            &[],
        )
        .await?;
    let generation = rows.first().and_then(|r| r.get::<_, Option<String>>(0));
    let one = |stream: Stream| {
        rows.iter()
            .find(|r| r.get::<_, Option<String>>(1).as_deref() == Some(stream.as_str()))
            .map(|r| {
                (
                    Coverage {
                        from_checkpoint: r.get::<_, Option<i64>>(2).map(|v| v.to_string()),
                        latest_checkpoint: r.get::<_, Option<i64>>(3).map(|v| v.to_string()),
                        events: r.get::<_, Option<i64>>(4).unwrap_or(0),
                    },
                    StreamHealth {
                        undecodable: r.get::<_, Option<i64>>(5).unwrap_or(0),
                        gaps: r.get::<_, Option<i64>>(6).unwrap_or(0),
                    },
                )
            })
            .unwrap_or((
                Coverage { from_checkpoint: None, latest_checkpoint: None, events: 0 },
                StreamHealth { undecodable: 0, gaps: 0 },
            ))
    };
    let (ag, sc) = (one(Stream::AccessGate), one(Stream::SealedContent));
    Ok(CoverageSnapshot {
        generation,
        streams: [(Stream::AccessGate, ag.0, ag.1), (Stream::SealedContent, sc.0, sc.1)],
    })
}

/// One gate's counters, its commission and where coverage starts, from one statement.
#[derive(Clone, Debug)]
pub struct GateSnapshot {
    pub stats: GateStats,
    pub from_checkpoint: Option<String>,
}

pub(crate) async fn gate_snapshot(c: &impl Query, gate_id: &str) -> Result<GateSnapshot> {
    let rows = c
        .rows(
            "SELECT s.kind, s.n, c.total::text, (SELECT from_checkpoint FROM stream_stats WHERE stream = 'access-gate')
             FROM (SELECT 1) AS one
             LEFT JOIN gate_stats s ON s.gate_id = $1
             LEFT JOIN gate_commission c ON c.gate_id = $1
             ORDER BY s.kind",
            &[&gate_id],
        )
        .await?;
    let counts = rows.iter().filter_map(|r| r.get::<_, Option<String>>(0).map(|k| (k, r.get::<_, i64>(1)))).collect();
    let total = rows.first().and_then(|r| r.get::<_, Option<String>>(2));
    Ok(GateSnapshot {
        stats: GateStats { counts, commission_mist: total.and_then(|t| t.parse().ok()).unwrap_or(0) },
        from_checkpoint: rows.first().and_then(|r| r.get::<_, Option<i64>>(3)).map(|v| v.to_string()),
    })
}

/// Both streams' poll heartbeats, from one statement.
pub(crate) async fn poll_states(c: &impl Query) -> Result<[PollState; 2]> {
    let rows = c
        .rows("SELECT key, value FROM heartbeats WHERE key IN ('poll:access-gate', 'poll:sealed-content')", &[])
        .await?;
    let one = |stream: Stream| {
        let key = poll_key(stream);
        rows.iter()
            .find(|r| r.get::<_, String>(0) == key)
            .and_then(|r| serde_json::from_str::<PollState>(&r.get::<_, String>(1)).ok())
            .unwrap_or_default()
    };
    Ok([one(Stream::AccessGate), one(Stream::SealedContent)])
}

/// One page, newest first:
/// - no filter, or only `before`: `events_stream`;
/// - `gate` (with or without `kinds`): `events_gate`; `kinds` is then only a row filter;
/// - one kind: `events_kind`;
/// - several kinds: one `events_kind` seek per kind (at most `limit + 1` rows each), merged by id.
pub(crate) async fn page(c: &impl Query, stream: Stream, q: &PageQuery) -> Result<Page> {
    let (sql, args) = page_sql(stream, q);
    let params: Vec<&(dyn ToSql + Sync)> = args.iter().map(Arg::as_sql).collect();
    let mut rows: Vec<EventRow> = c
        .rows(&sql, &params)
        .await?
        .iter()
        .map(|r| EventRow {
            id: r.get(0),
            event_type: r.get(1),
            sender: r.get(2),
            bcs: r.get(3),
            checkpoint: r.get::<_, Option<i64>>(4).map(|c| c.to_string()),
            transaction_digest: r.get(5),
            event_index: r.get(6),
        })
        .collect();
    let more = rows.len() > q.limit as usize;
    rows.truncate(q.limit as usize);
    let next = if more { rows.last().map(|r| r.id) } else { None };
    Ok(Page { rows, next })
}

/// The newest row id of `stream` (0 when empty): the head page's ETag. Ids are ordered, not contiguous.
pub(crate) async fn head_id(c: &impl Query, stream: Stream) -> Result<i64> {
    let rows = c.rows("SELECT max(id) FROM events WHERE stream = $1", &[&stream.as_str()]).await?;
    Ok(rows.first().and_then(|r| r.get::<_, Option<i64>>(0)).unwrap_or(0))
}

pub(crate) async fn coverage(c: &impl Query, stream: Stream) -> Result<Coverage> {
    let rows = c
        .rows(
            "SELECT from_checkpoint, latest_checkpoint, events FROM stream_stats WHERE stream = $1",
            &[&stream.as_str()],
        )
        .await?;
    Ok(match rows.first() {
        Some(r) => Coverage {
            from_checkpoint: r.get::<_, Option<i64>>(0).map(|v| v.to_string()),
            latest_checkpoint: r.get::<_, Option<i64>>(1).map(|v| v.to_string()),
            events: r.get(2),
        },
        None => Coverage { from_checkpoint: None, latest_checkpoint: None, events: 0 },
    })
}

pub(crate) async fn health(c: &impl Query, stream: Stream) -> Result<StreamHealth> {
    let rows = c.rows("SELECT undecodable, gaps FROM stream_stats WHERE stream = $1", &[&stream.as_str()]).await?;
    Ok(rows
        .first()
        .map(|r| StreamHealth { undecodable: r.get(0), gaps: r.get(1) })
        .unwrap_or(StreamHealth { undecodable: 0, gaps: 0 }))
}

pub(crate) async fn gate_stats(c: &impl Query, gate_id: &str) -> Result<GateStats> {
    let counts = c
        .rows("SELECT kind, n FROM gate_stats WHERE gate_id = $1 ORDER BY kind", &[&gate_id])
        .await?
        .iter()
        .map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1)))
        .collect();
    let total = c.rows("SELECT total::text FROM gate_commission WHERE gate_id = $1", &[&gate_id]).await?;
    Ok(GateStats {
        counts,
        commission_mist: total.first().and_then(|r| r.get::<_, String>(0).parse().ok()).unwrap_or(0),
    })
}

pub(crate) async fn meta(c: &impl Query, key: &str) -> Result<Option<String>> {
    Ok(c.rows("SELECT value FROM meta WHERE key = $1", &[&key]).await?.first().map(|r| r.get(0)))
}

pub(crate) async fn heartbeat(c: &impl Query, key: &str) -> Result<Option<String>> {
    Ok(c.rows("SELECT value FROM heartbeats WHERE key = $1", &[&key]).await?.first().map(|r| r.get(0)))
}

pub(crate) async fn poll_state(c: &impl Query, stream: Stream) -> Result<PollState> {
    Ok(heartbeat(c, &poll_key(stream))
        .await?
        .and_then(|v| serde_json::from_str::<PollState>(&v).ok())
        .unwrap_or_default())
}
