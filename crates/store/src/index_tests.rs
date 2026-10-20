//! The `events_kind` index (F25) on PostgreSQL: the plan of every filter shape (custom and generic
//! plans), the merged kind seeks against the plain filter, the page cost on a large stream, and a 300,000-row
//! measurement (`INDEX_MEASURE=1`).

use super::*;
use crate::read::{Arg, page_sql};
use crate::require_db;
use crate::sui::normalize_sui_address;
use crate::writer::Store;

const STREAM: Stream = Stream::AccessGate;

fn gate(i: u32) -> String {
    normalize_sui_address(&format!("0x{i:x}"))
}

/// Fill the table the way production does: `n` `access-gate` rows (mostly `AccessMinted`, 1 in 1000
/// `GateCreated`, and 5 `AccessConsumed`: the rare kind, spread over 300 gates) interleaved in ledger order
/// with `n / 999` `sealed-content` rows (a rare STREAM, 1 row in 1000). Done in SQL: the shape matters, not the BCS. Then
/// ANALYZE, so the planner sees what a long-running index would have.
async fn fill(s: &Store, n: u32) {
    let rare: Vec<String> = (1..=5).map(|k| (n * k / 6).to_string()).collect();
    let total = n + n / 999;
    s.execute_batch(&format!(
        "INSERT INTO events (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind, gate_id, commission_mist)
         SELECT CASE WHEN i % 1000 = 0 THEN 'sealed-content' ELSE 'access-gate' END, 'd' || i, 0, i, 's', 't', '\\x00'::bytea,
                CASE WHEN i % 1000 = 0 THEN 'SealedContentPublished'
                     WHEN (i - i / 1000) IN ({}) THEN 'AccessConsumed'
                     WHEN (i - i / 1000) % 1000 = 7 THEN 'GateCreated'
                     ELSE 'AccessMinted' END,
                '0x' || lpad(to_hex(i % 300), 64, '0'), 5
         FROM generate_series(1, {total}) AS i
         ORDER BY i;
         ANALYZE events",
        rare.join(", ")
    ))
    .await
    .unwrap();
}

fn q(kinds: &[&str], gate_id: Option<String>, before_id: Option<i64>) -> PageQuery {
    PageQuery {
        kinds: (!kinds.is_empty()).then(|| kinds.iter().map(|k| (*k).to_owned()).collect()),
        gate_id,
        before_id,
        limit: 20,
    }
}

/// The query with its arguments written in as literals (EXPLAIN ANALYZE cannot bind).
fn inline(stream: Stream, query: &PageQuery) -> String {
    let (mut sql, args) = page_sql(stream, query);
    for (i, a) in args.iter().enumerate().rev() {
        sql = sql.replace(&format!("${}", i + 1), &a.literal());
    }
    sql
}

async fn text_rows(s: &Store, sql: &str) -> String {
    let msgs = s.client().simple_query(sql).await.unwrap();
    msgs.iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => r.get(0).map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The plan with the arguments known (what the first executions of a prepared statement get).
async fn custom_plan(s: &Store, stream: Stream, query: &PageQuery) -> String {
    text_rows(s, &format!("EXPLAIN (COSTS OFF) {}", inline(stream, query))).await
}

/// Buffers touched (hit + read, of the root node: the whole query) and rows removed by filters, from
/// `EXPLAIN (ANALYZE, BUFFERS)`.
async fn cost(s: &Store, query: &PageQuery) -> (i64, i64, String) {
    let plan =
        text_rows(s, &format!("EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, TIMING OFF) {}", inline(STREAM, query))).await;
    let numbers = |line: &str, keys: &[&str]| -> i64 {
        line.split_whitespace()
            .filter_map(|w| keys.iter().find_map(|k| w.strip_prefix(k)).and_then(|n| n.parse::<i64>().ok()))
            .sum()
    };
    let buffers = plan.lines().find(|l| l.contains("Buffers:")).map_or(0, |l| numbers(l, &["hit=", "read="]));
    let removed = plan
        .lines()
        .filter_map(|l| l.split("Rows Removed by Filter:").nth(1))
        .filter_map(|n| n.trim().parse::<i64>().ok())
        .sum();
    (buffers, removed, plan)
}

async fn run_ids(s: &Store, query: &PageQuery) -> Vec<i64> {
    let (sql, args) = page_sql(STREAM, query);
    let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = args.iter().map(Arg::as_sql).collect();
    s.query(&sql, &params).await.unwrap().iter().map(|r| r.get(0)).collect()
}

#[tokio::test]
async fn the_api_answers_without_events_kind_and_the_index_is_in_the_first_migration() {
    let db = require_db!();
    let mut s = db.store().await;
    s.ingest(STREAM, &[crate::tests::minted("0xa", 1, 1, "t1")], Some("c")).await.unwrap();
    let has = |name: &'static str| {
        let s = &s;
        async move {
            !s.query("SELECT 1 FROM pg_indexes WHERE schemaname = 'public' AND indexname = $1", &[&name])
                .await
                .unwrap()
                .is_empty()
        }
    };
    for index in ["events_stream", "events_gate", "events_kind"] {
        assert!(has(index).await, "{index} is created by migration 1");
    }
    let ddl: String =
        s.query("SELECT indexdef FROM pg_indexes WHERE indexname = 'events_kind'", &[]).await.unwrap()[0].get(0);
    assert_eq!(ddl, "CREATE INDEX events_kind ON public.events USING btree (stream, kind, id)");
    // The index serves only speed: the read-only API (which cannot create one) answers without it.
    db.admin().await.batch_execute("DROP INDEX events_kind").await.unwrap();
    let r = db.reader(1);
    let all = r.events(STREAM, &q(&["AccessMinted", "AccessConsumed"], None, None)).await.unwrap().page;
    let one = r.events(STREAM, &q(&["AccessMinted"], None, None)).await.unwrap().page;
    assert_eq!((all.rows.len(), one.rows.len()), (1, 1));
    assert!(r.try_write().await.is_err());
}

#[tokio::test]
async fn every_filter_shape_uses_the_intended_index() {
    let db = require_db!();
    let s = db.store().await;
    fill(&s, 30_000).await;
    let g = Some(gate(7));
    let shapes: Vec<(&str, PageQuery)> = vec![
        ("stream", q(&[], None, None)),
        ("stream before", q(&[], None, Some(100))),
        ("one kind", q(&["AccessConsumed"], None, None)),
        ("one kind before", q(&["AccessConsumed"], None, Some(100))),
        ("two kinds", q(&["AccessConsumed", "GateCreated"], None, None)),
        ("two kinds before", q(&["AccessMinted", "AccessConsumed"], None, Some(100))),
        ("gate", q(&[], g.clone(), None)),
        ("gate before", q(&[], g.clone(), Some(100))),
        ("gate and a kind", q(&["AccessConsumed"], g.clone(), None)),
        ("gate and two kinds before", q(&["AccessMinted", "AccessConsumed"], g.clone(), Some(100))),
    ];
    for (name, query) in &shapes {
        {
            let (mode, p) = ("custom", custom_plan(&s, STREAM, query).await);
            // Whatever the shape: an index, never a walk of the table, and no sort of the table.
            assert!(!p.contains("Seq Scan") && !p.contains("Bitmap Heap Scan"), "{name}/{mode}:\n{p}");
            match *name {
                "stream" | "stream before" => {
                    // The stream that makes up most of the table may be read by the primary key (every
                    // row matches, so the first 21 do); never by a kind or gate index, never sorted.
                    assert!(
                        !p.contains("events_kind") && !p.contains("events_gate") && !p.contains("Sort"),
                        "{name}/{mode}:\n{p}"
                    );
                    assert!(p.contains("events_stream") || p.contains("events_pkey"), "{name}/{mode}:\n{p}");
                }
                "one kind" | "one kind before" => {
                    assert!(p.contains("Index Scan Backward using events_kind"), "{name}/{mode}:\n{p}");
                    assert!(!p.contains("Sort"), "{name}/{mode}:\n{p}");
                }
                "two kinds" | "two kinds before" => {
                    // One bounded seek per kind, merged: a top-N over the arms' rows only.
                    assert!(
                        p.contains("Append") && !p.contains("events_stream") && !p.contains("events_gate"),
                        "{name}/{mode}:\n{p}"
                    );
                    let arms_on_kind = p.matches("Index Scan Backward using events_kind").count();
                    if *name == "two kinds" {
                        assert_eq!(arms_on_kind, 2, "{name}/{mode}:\n{p}");
                    } else {
                        // With an upper bound as small as 100 the planner may read a common kind's arm off
                        // the primary key (a range of 100 ids): still bounded, which is the point.
                        let pk_arms = p.matches("Index Scan Backward using events_pkey").count();
                        assert_eq!(arms_on_kind + pk_arms, 2, "{name}/{mode}:\n{p}");
                        assert!(pk_arms == 0 || p.contains("Index Cond: (id < 100)"), "{name}/{mode}:\n{p}");
                    }
                }
                _ => {
                    // A gate, alone or with kinds: the gate index; kinds only filter its rows.
                    assert!(p.contains("events_gate") && !p.contains("events_kind"), "{name}/{mode}:\n{p}");
                    assert!(!p.contains("Sort"), "{name}/{mode}:\n{p}");
                }
            }
        }
    }
    // A rare STREAM (1 row in 1000) is read through (stream, id): walking the primary key backward would
    // filter past thousands of other rows. The planner knows from the statistics, with the values given
    // (the API's pooled connections force custom plans for exactly this reason).
    let sealed = PageQuery { limit: 20, ..Default::default() };
    let p = custom_plan(&s, Stream::SealedContent, &sealed).await;
    assert!(p.contains("events_stream") && !p.contains("events_pkey") && !p.contains("Seq Scan"), "{p}");
    let p = custom_plan(&s, Stream::SealedContent, &PageQuery { before_id: Some(500), ..sealed.clone() }).await;
    assert!(p.contains("events_stream") && !p.contains("events_pkey"), "{p}");
}

#[tokio::test]
async fn merged_kind_seeks_return_what_the_filter_returns() {
    let db = require_db!();
    let s = db.store().await;
    fill(&s, 4_000).await;
    let every = ["AccessMinted", "AccessConsumed", "GateCreated"];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |m: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % m
    };
    for _ in 0..200 {
        let n = 1 + next(3) as usize;
        let mut kinds: Vec<&str> = (0..n).map(|_| every[next(3) as usize]).collect(); // may repeat
        kinds.sort_unstable();
        let before = (next(2) == 0).then(|| next(4200) as i64);
        let gate_id = (next(4) == 0).then(|| gate(next(300) as u32));
        let limit = 1 + next(30) as u32;
        let mut query = q(&kinds, gate_id.clone(), before);
        query.limit = limit;
        // The reference: the plain filter, ordered by id.
        let mut want = String::from("SELECT id FROM events WHERE stream = 'access-gate'");
        want.push_str(&format!(
            " AND kind IN ({})",
            kinds.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",")
        ));
        if let Some(g) = &gate_id {
            want.push_str(&format!(" AND gate_id = '{g}'"));
        }
        if let Some(b) = before {
            want.push_str(&format!(" AND id < {b}"));
        }
        want.push_str(&format!(" ORDER BY id DESC LIMIT {}", limit + 1));
        let want: Vec<i64> = s.query(&want, &[]).await.unwrap().iter().map(|r| r.get(0)).collect();
        let got = run_ids(&s, &query).await;
        assert_eq!(got, want, "kinds {kinds:?} gate {gate_id:?} before {before:?} limit {limit}");
        let page = s.page(STREAM, &query).await.unwrap();
        assert_eq!(page.rows.len(), want.len().min(limit as usize));
        assert_eq!(page.next.is_some(), want.len() > limit as usize);
    }
}

/// One statement serves every kind (`kind = $3`). Its GENERIC plan (parameters unknown) walks the primary key
/// backward with a filter (`EXPLAIN (GENERIC_PLAN)` shows it), which is right for a common kind and a walk of
/// the table for a rare one. PostgreSQL's `plan_cache_mode = auto` adopts a generic plan after five executions
/// when it estimates it no worse than the custom ones; on the data of these tests it does not, but that is a
/// cost estimate on today's statistics, not a guarantee. The API's pooled connections therefore force custom
/// plans (the values known, the statistics used): after any number of executions with a common kind, a rare
/// kind still uses its index. (Protocol-level prepared statements are not listed in `pg_prepared_statements`,
/// so the same statement is prepared with SQL `PREPARE` on a connection set up like the pool's.)
#[tokio::test]
async fn pooled_connections_use_custom_plans_so_a_rare_kind_keeps_its_index() {
    let db = require_db!();
    let s = db.store().await;
    fill(&s, 50_000).await;
    let (sql, args) = page_sql(STREAM, &q(&["AccessConsumed"], None, None));
    assert_eq!(args.len(), 3);
    let c = db.owner.connect().await.unwrap();
    c.batch_execute(crate::reader::SESSION_SETUP).await.unwrap();
    c.batch_execute(&format!("PREPARE p(text, bigint, text) AS {sql}")).await.unwrap();
    for _ in 0..12 {
        c.batch_execute("EXECUTE p('access-gate', 21, 'AccessMinted')").await.unwrap();
    }
    let r = c
        .query_one("SELECT generic_plans, custom_plans FROM pg_prepared_statements WHERE name = 'p'", &[])
        .await
        .unwrap();
    assert_eq!((r.get::<_, i64>(0), r.get::<_, i64>(1)), (0, 12));
    let plan = c
        .simple_query(
            "EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, TIMING OFF) EXECUTE p('access-gate', 21, 'AccessConsumed')",
        )
        .await
        .unwrap()
        .iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => r.get(0).map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plan.contains("events_kind") && !plan.contains("Rows Removed by Filter"), "{plan}");
    // The generic plan, for the record: the hazard the setting removes.
    let generic = text_rows(&s, &format!("EXPLAIN (COSTS OFF, GENERIC_PLAN) {sql}")).await;
    assert!(generic.contains("events_pkey") || generic.contains("events_kind"), "{generic}");
    // And the pool really applies that setup to its connections.
    let r = db.reader(1);
    for _ in 0..8 {
        assert!(!r.events(STREAM, &q(&["AccessConsumed"], None, None)).await.unwrap().page.rows.is_empty());
    }
    assert_eq!(r.query_for_test("SHOW plan_cache_mode").await, "force_custom_plan");
}

/// F25: a rare kind costs its own rows, not the stream's.
#[tokio::test]
async fn a_rare_kind_page_over_300k_rows_touches_a_handful_of_buffers() {
    let db = require_db!();
    let s = db.store().await;
    fill(&s, 300_000).await;
    for kinds in [&["AccessConsumed"][..], &["AccessConsumed", "GateCreated"][..]] {
        for before in [None, Some(150_000)] {
            let (buffers, removed, plan) = cost(&s, &q(kinds, None, before)).await;
            assert!(
                removed == 0 && buffers < 200,
                "{kinds:?} before {before:?}: {buffers} buffers, {removed} rows filtered:\n{plan}"
            );
            assert!(!run_ids(&s, &q(kinds, None, before)).await.is_empty());
        }
    }
    // The head page and a gate page are as cheap.
    for query in [q(&[], None, None), q(&[], Some(gate(7)), None)] {
        let (buffers, _, plan) = cost(&s, &query).await;
        assert!(buffers < 200, "{buffers} buffers:\n{plan}");
    }
    // So is the head page of the RARE stream: one id range, not a walk past the other stream's rows.
    let rare = PageQuery { limit: 20, ..Default::default() };
    let plan = text_rows(
        &s,
        &format!("EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, TIMING OFF) {}", inline(Stream::SealedContent, &rare)),
    )
    .await;
    assert!(plan.contains("events_stream") && !plan.contains("Rows Removed by Filter"), "{plan}");
    // Without the index the same single-kind page walks the stream: the test sees the difference.
    s.execute_batch("DROP INDEX events_kind").await.unwrap();
    let (buffers, _, plan) = cost(&s, &q(&["AccessConsumed"], None, None)).await;
    assert!(
        buffers > 2_000 || plan.contains("Rows Removed by Filter"),
        "without events_kind the page took only {buffers} buffers:\n{plan}"
    );
}

/// Recorded in the audit (F25). Run: `INDEX_MEASURE=1 cargo test -p sui-indexer-store --release -- --nocapture measurement`.
#[tokio::test]
async fn measurement_on_300k_rows() {
    if std::env::var("INDEX_MEASURE").ok().as_deref() != Some("1") {
        return;
    }
    let db = require_db!();
    let s = db.store().await;
    fill(&s, 300_000).await;
    let r = db.reader(8);
    for (label, query) in [
        ("head page", q(&[], None, None)),
        ("one rare kind", q(&["AccessConsumed"], None, None)),
        ("two kinds, one rare", q(&["AccessConsumed", "GateCreated"], None, None)),
        ("gate", q(&[], Some(gate(7)), None)),
    ] {
        let t = std::time::Instant::now();
        let mut n = 0u32;
        while t.elapsed().as_millis() < 2000 {
            assert!(!r.events(STREAM, &query).await.unwrap().page.rows.is_empty());
            n += 1;
        }
        println!("{label}: {:.0} pages/s (one connection, sequential)", f64::from(n) / t.elapsed().as_secs_f64());
    }
}

/// The statement an events request runs (facts + page) uses the same indexes as the page alone, and never
/// walks the table, for every filter shape.
#[tokio::test]
async fn the_events_statement_keeps_the_intended_index_for_every_shape() {
    let db = require_db!();
    let s = db.store().await;
    fill(&s, 30_000).await;
    let g = Some(gate(7));
    for (name, query, index) in [
        ("one kind", q(&["AccessConsumed"], None, None), "events_kind"),
        ("two kinds", q(&["AccessConsumed", "GateCreated"], None, Some(100)), "events_kind"),
        ("gate", q(&[], g.clone(), None), "events_gate"),
        ("gate and kinds", q(&["AccessMinted", "AccessConsumed"], g, Some(100)), "events_gate"),
    ] {
        let (mut sql, args) = crate::read::events_sql(STREAM, &query);
        for (i, a) in args.iter().enumerate().rev() {
            sql = sql.replace(&format!("${}", i + 1), &a.literal());
        }
        let plan = text_rows(&s, &format!("EXPLAIN (COSTS OFF) {sql}")).await;
        assert!(plan.contains(index) && !plan.contains("Seq Scan on events"), "{name}:\n{plan}");
        // And the cost on 30,000 rows is a handful of buffers, the head's `max(id)` included.
        let analyzed = text_rows(&s, &format!("EXPLAIN (ANALYZE, BUFFERS, COSTS OFF, TIMING OFF) {sql}")).await;
        let buffers: i64 = analyzed.lines().find(|l| l.contains("Buffers:")).map_or(0, |l| {
            l.split_whitespace()
                .filter_map(|w| {
                    w.strip_prefix("hit=").or_else(|| w.strip_prefix("read=")).and_then(|n| n.parse::<i64>().ok())
                })
                .sum()
        });
        assert!(buffers < 300, "{name}: {buffers} buffers\n{analyzed}");
    }
}
