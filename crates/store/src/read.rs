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
    /// Run a query and return its rows.
    fn rows(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl std::future::Future<Output = std::result::Result<Vec<Row>, tokio_postgres::Error>> + Send;
}

impl Query for tokio_postgres::Client {
    /// Run on a plain client (the writer's single connection); the statement is prepared each time.
    async fn rows(
        &self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> std::result::Result<Vec<Row>, tokio_postgres::Error> {
        self.query(sql, params).await
    }
}

impl Query for deadpool_postgres::ClientWrapper {
    /// Run on a pooled client, preparing the statement once and caching it for the API's many identical requests.
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
    /// A text bind value.
    Text(String),
    /// An integer bind value.
    Int(i64),
    /// A text-array bind value (the `kinds` filter).
    TextArray(Vec<String>),
}

impl Arg {
    /// The value as a bind parameter.
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

/// The columns of an event row, in the order the row mapping reads them.
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
    // Ask for one row more than the page, to learn whether an older page exists without a count.
    let take = i64::from(q.limit) + 1;
    // Parameter `$1` is always the stream; the rest are numbered as they are pushed.
    let mut args = vec![Arg::Text(stream.as_str().to_owned())];
    // The optional `before` bound is a bind parameter, so ids are never spliced into the SQL.
    let before = q.before_id.map(|b| {
        args.push(Arg::Int(b));
        args.len()
    });
    args.push(Arg::Int(take));
    let take_n = args.len();
    // `id < $n` makes the page start after the cursor (ids are the ledger order).
    let before_clause = before.map(|n| format!(" AND id < ${n}")).unwrap_or_default();

    if let (Some(kinds), None) = (kinds, &q.gate_id)
        && kinds.len() > 1
    {
        // One bounded, ordered seek per kind, merged: cost O(kinds x limit), independent of history.
        // Each arm is its own bounded seek on the per-kind index, newest first.
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
    // The general shape: stream, optional gate, optional kind, optional cursor, newest first.
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
            // One kind and no gate: a plain equality the per-kind index serves.
            args.push(Arg::Text(kinds[0].clone()));
            sql.push_str(&format!(" AND kind = ${}", args.len()));
        }
    }
    // The cursor condition goes after the filters, so it applies to every shape above.
    sql.push_str(&before_clause);
    sql.push_str(&format!(" ORDER BY id DESC LIMIT ${take_n}"));
    (sql, args)
}

/// The statement of one events request: the head's facts and the page, read by ONE statement, so they are
/// one snapshot (and one round trip). `m` is a single row (generation, newest id of the stream, the stream's
/// counters, its poll heartbeat); the page is `LEFT JOIN LATERAL`-ed onto it, so an empty page still returns
/// the facts. Parameters: those of [`page_sql`], then the heartbeat key.
pub(crate) fn events_sql(stream: Stream, q: &PageQuery) -> (String, Vec<Arg>) {
    // Start from the page statement and its parameters.
    let (page, mut args) = page_sql(stream, q);
    // The last parameter is the stream's heartbeat key.
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
    /// The index generation (changes when the index is rebuilt); a cursor is only valid within one.
    pub generation: Option<String>,
    /// The newest row id of the stream (0 when empty): the head page's ETag.
    pub head_id: i64,
    /// The stream's coverage counters.
    pub coverage: Coverage,
    /// The ingester's poll state for the stream.
    pub poll: PollState,
    /// The page of rows.
    pub page: Page,
}

/// Read the generation, head id, coverage, poll state and one page of a stream in a single statement, so they describe one consistent moment.
pub(crate) async fn events(c: &impl Query, stream: Stream, q: &PageQuery) -> Result<EventsSnapshot> {
    // The whole answer comes from one statement (see `events_sql`).
    let (sql, args) = events_sql(stream, q);
    // Borrow each argument as a bind parameter.
    let params: Vec<&(dyn ToSql + Sync)> = args.iter().map(Arg::as_sql).collect();
    // One statement answers the page and the facts about the index (see `events_sql`).
    let rows = c.rows(&sql, &params).await?;
    // The head row is always returned, even for an empty page; none means the statement itself is wrong.
    let first = rows.first().ok_or_else(|| StoreError::Data("the events statement returned no row".into()))?;
    // A stream with no statistics row yet has no coverage.
    let coverage = match first.get::<_, Option<i64>>(4) {
        Some(n) => Coverage {
            from_checkpoint: first.get::<_, Option<i64>>(2).map(|v| v.to_string()),
            latest_checkpoint: first.get::<_, Option<i64>>(3).map(|v| v.to_string()),
            events: n,
        },
        None => Coverage { from_checkpoint: None, latest_checkpoint: None, events: 0 },
    };
    let poll =
        // The poll heartbeat is JSON; an unreadable or missing one counts as never polled.
        first.get::<_, Option<String>>(5).and_then(|v| serde_json::from_str::<PollState>(&v).ok()).unwrap_or_default();
    // Rows with no id are the lateral join's placeholder for an empty page; keep only real rows.
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
    // Having more rows than the limit proves an older page exists.
    let more = page_rows.len() > q.limit as usize;
    // Drop the look-ahead row from the page.
    page_rows.truncate(q.limit as usize);
    // The cursor is the id of the last row served: the next page asks for ids below it.
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
    /// The index generation.
    pub generation: Option<String>,
    /// Coverage and health of both streams, in the order of `Stream::ALL`.
    pub streams: [(Stream, Coverage, StreamHealth); 2],
}

/// Read both streams' counters and the generation in one statement.
pub(crate) async fn coverage_snapshot(c: &impl Query) -> Result<CoverageSnapshot> {
    // A single-row anchor with LEFT JOINs, so the answer exists even before any stream has data.
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
    // Pick each stream's row out of the one result set, with zeros for a stream that has none.
    let (ag, sc) = (one(Stream::AccessGate), one(Stream::SealedContent));
    Ok(CoverageSnapshot {
        generation,
        streams: [(Stream::AccessGate, ag.0, ag.1), (Stream::SealedContent, sc.0, sc.1)],
    })
}

/// One gate's counters, its commission and where coverage starts, from one statement.
#[derive(Clone, Debug)]
pub struct GateSnapshot {
    /// The counts by kind and the commission total.
    pub stats: GateStats,
    /// The first checkpoint of the access-gate stream, so a client knows how far back the counts reach.
    pub from_checkpoint: Option<String>,
}

/// Read one gate's counters, commission and the start of coverage in one statement.
pub(crate) async fn gate_snapshot(c: &impl Query, gate_id: &str) -> Result<GateSnapshot> {
    // Per-kind counts and the commission total are maintained at ingest, so this never scans events.
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
    // The anchor row has no kind for a gate never seen: no counts, not a zero row.
    let counts = rows.iter().filter_map(|r| r.get::<_, Option<String>>(0).map(|k| (k, r.get::<_, i64>(1)))).collect();
    // The commission total arrives as text because it can exceed 64 bits; it is parsed as u128.
    let total = rows.first().and_then(|r| r.get::<_, Option<String>>(2));
    Ok(GateSnapshot {
        stats: GateStats { counts, commission_mist: total.and_then(|t| t.parse().ok()).unwrap_or(0) },
        from_checkpoint: rows.first().and_then(|r| r.get::<_, Option<i64>>(3)).map(|v| v.to_string()),
    })
}

/// Both streams' poll heartbeats, from one statement.
pub(crate) async fn poll_states(c: &impl Query) -> Result<[PollState; 2]> {
    let rows = c
        // Both streams' heartbeats in one round trip.
        .rows("SELECT key, value FROM heartbeats WHERE key IN ('poll:access-gate', 'poll:sealed-content')", &[])
        .await?;
    let one = |stream: Stream| {
        // The heartbeat key of a stream, so each result row is matched to its stream.
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

/// Read one stream's coverage counters (checkpoints are decimal strings).
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

/// Read one stream's undecodable and gap counters.
pub(crate) async fn health(c: &impl Query, stream: Stream) -> Result<StreamHealth> {
    let rows = c.rows("SELECT undecodable, gaps FROM stream_stats WHERE stream = $1", &[&stream.as_str()]).await?;
    Ok(rows
        .first()
        .map(|r| StreamHealth { undecodable: r.get(0), gaps: r.get(1) })
        .unwrap_or(StreamHealth { undecodable: 0, gaps: 0 }))
}

/// Read one gate's counts by kind and its commission total (zero for a gate never seen).
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

/// Read a value from the `meta` table (the bound packages and the index generation).
pub(crate) async fn meta(c: &impl Query, key: &str) -> Result<Option<String>> {
    Ok(c.rows("SELECT value FROM meta WHERE key = $1", &[&key]).await?.first().map(|r| r.get(0)))
}

/// Read a value from the `heartbeats` table (poll state and liveness).
pub(crate) async fn heartbeat(c: &impl Query, key: &str) -> Result<Option<String>> {
    Ok(c.rows("SELECT value FROM heartbeats WHERE key = $1", &[&key]).await?.first().map(|r| r.get(0)))
}

/// Read a stream's persisted poll state, or the default when none was stored or it cannot be read.
pub(crate) async fn poll_state(c: &impl Query, stream: Stream) -> Result<PollState> {
    Ok(heartbeat(c, &poll_key(stream))
        .await?
        .and_then(|v| serde_json::from_str::<PollState>(&v).ok())
        .unwrap_or_default())
}
