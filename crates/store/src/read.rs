//! Queries both the writer and the API's sessions run. None of them scans a stream: counters are
//! maintained at ingest. Which index serves a page is part of the cost model (the query-plan tests in
//! `index_tests.rs` pin it).

use tokio_postgres::Row;
use tokio_postgres::types::ToSql;

use crate::error::Result;
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
