//! The `events_kind` index (F25): migration of an older database, the plan of every filter shape,
//! the page cost on a large stream, and the 300,000-row measurement (`--ignored`).

use super::*;
use crate::sui::normalize_sui_address;
use rusqlite::StatementStatus;

const STREAM: Stream = Stream::AccessGate;

fn gate(i: u32) -> String {
    normalize_sui_address(&format!("0x{i:x}"))
}

/// Fill a stream with `n` rows: mostly `AccessMinted`, 1 in 1000 `GateCreated`, and 5 `AccessConsumed`
/// (the rare kind), spread over 300 gates. Done in SQL: the shape matters, not the BCS.
fn fill(conn: &Connection, n: u32) {
    let rare: Vec<String> = (1..=5).map(|k| (n * k / 6).to_string()).collect();
    conn.execute_batch(&format!(
        "WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM s WHERE i < {n})
         INSERT INTO events (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind, gate_id, commission_mist)
         SELECT 'access-gate', 'd' || i, 0, i, 's', 't', x'00',
                CASE WHEN i IN ({}) THEN 'AccessConsumed' WHEN i % 1000 = 7 THEN 'GateCreated' ELSE 'AccessMinted' END,
                '0x' || printf('%064x', i % 300), 5
         FROM s;",
        rare.join(", ")
    ))
    .unwrap();
}

fn has_index(conn: &Connection, name: &str) -> bool {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?", [name], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap()
        == 1
}

fn q(kinds: &[&str], gate_id: Option<String>, before_id: Option<i64>) -> PageQuery {
    PageQuery {
        kinds: (!kinds.is_empty()).then(|| kinds.iter().map(|k| (*k).to_owned()).collect()),
        gate_id,
        before_id,
        limit: 20,
    }
}

/// `EXPLAIN QUERY PLAN` detail lines of the SQL a page query runs.
fn plan(conn: &Connection, query: &PageQuery) -> String {
    let (sql, args) = page_sql(STREAM, query);
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let rows: Vec<String> = stmt
        .query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    rows.join("\n")
}

/// The ids a page query returns and the VM steps SQLite took to produce them.
fn run(conn: &Connection, query: &PageQuery) -> (Vec<i64>, i32) {
    let (sql, args) = page_sql(STREAM, query);
    let mut stmt = conn.prepare(&sql).unwrap();
    let ids: Vec<i64> = stmt
        .query_map(rusqlite::params_from_iter(args), |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    (ids, stmt.get_status(StatementStatus::VmStep))
}

#[test]
fn the_ingester_adds_the_index_to_an_older_database_and_it_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    {
        // A 0.0.12 (or TypeScript) database: the same tables, no `events_kind`.
        let s = Store::open(&path).unwrap();
        s.ingest(STREAM, &[crate::tests::minted_for_index("0xa", 1, "t1")], Some("c")).unwrap();
        s.connection().execute_batch("DROP INDEX events_kind").unwrap();
        assert!(!has_index(s.connection(), "events_kind"));
    }
    // The API's read-only view opens it and answers without the index.
    let r = Reader::open(&path).unwrap();
    let all = r.page(STREAM, &q(&["AccessMinted", "AccessConsumed"], None, None)).unwrap();
    let one = r.page(STREAM, &q(&["AccessMinted"], None, None)).unwrap();
    assert_eq!((all.rows.len(), one.rows.len()), (1, 1));
    assert!(r.try_write().is_err());
    drop(r);
    // The ingester's start creates it, once.
    let s = Store::open(&path).unwrap();
    assert!(has_index(s.connection(), "events_kind"));
    let ddl = |s: &Store| -> String {
        s.connection().query_row("SELECT sql FROM sqlite_master WHERE name = 'events_kind'", [], |r| r.get(0)).unwrap()
    };
    assert_eq!(ddl(&s), "CREATE INDEX events_kind ON events (stream, kind, id)");
    drop(s);
    let again = Store::open(&path).unwrap();
    assert!(has_index(again.connection(), "events_kind"));
    assert_eq!(again.coverage(STREAM).unwrap().events, 1);
    // A fresh database has it from the start, and the reader never creates it.
    let fresh = Store::open(dir.path().join("fresh.db")).unwrap();
    assert!(has_index(fresh.connection(), "events_kind"));
    let r = Reader::open(&path).unwrap();
    assert!(r.try_write().is_err());
}

#[test]
fn every_filter_shape_uses_the_intended_index() {
    let s = Store::open(":memory:").unwrap();
    let c = s.connection();
    fill(c, 3000);
    let g = Some(gate(7));
    // No filter, or only a cursor: the stream index.
    for query in [q(&[], None, None), q(&[], None, Some(100))] {
        let p = plan(c, &query);
        assert!(p.contains("events_stream") && !p.contains("events_kind") && !p.contains("events_gate"), "{p}");
    }
    // One kind: the kind index, with or without a cursor, and no sort.
    for query in [q(&["AccessConsumed"], None, None), q(&["AccessConsumed"], None, Some(100))] {
        let p = plan(c, &query);
        assert!(p.contains("events_kind (stream=? AND kind=?") && !p.contains("TEMP B-TREE"), "{p}");
    }
    // Several kinds: one seek on the kind index per kind, merged; never a walk of the stream.
    for query in
        [q(&["AccessConsumed", "GateCreated"], None, None), q(&["AccessMinted", "AccessConsumed"], None, Some(100))]
    {
        let p = plan(c, &query);
        assert!(p.contains("MERGE (UNION ALL)"), "{p}");
        assert_eq!(p.matches("events_kind (stream=? AND kind=?").count(), 2, "{p}");
        assert!(!p.contains("events_stream") && !p.contains("events_gate") && !p.contains("SCAN events"), "{p}");
    }
    // A gate, alone or with kinds: the gate index (bounded by the gate's rows), kinds only filter.
    for query in [
        q(&[], g.clone(), None),
        q(&["AccessConsumed"], g.clone(), None),
        q(&["AccessMinted", "AccessConsumed"], g.clone(), Some(100)),
    ] {
        let p = plan(c, &query);
        assert!(p.contains("events_gate (stream=? AND gate_id=?") && !p.contains("events_kind"), "{p}");
    }
}

#[test]
fn merged_kind_seeks_return_what_the_filter_returns() {
    let s = Store::open(":memory:").unwrap();
    let c = s.connection();
    fill(c, 4000);
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
        let want: Vec<i64> =
            c.prepare(&want).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        let (got, _) = run(c, &query);
        assert_eq!(got, want, "kinds {kinds:?} gate {gate_id:?} before {before:?} limit {limit}");
        let page = s.page(STREAM, &query).unwrap();
        assert_eq!(page.rows.len(), want.len().min(limit as usize));
        assert_eq!(page.next.is_some(), want.len() > limit as usize);
    }
}

/// F25: a rare kind costs its own rows, not the stream's.
#[test]
fn a_rare_kind_page_over_300k_rows_costs_a_handful_of_steps() {
    let s = Store::open(":memory:").unwrap();
    let c = s.connection();
    fill(c, 300_000);
    for kinds in [&["AccessConsumed"][..], &["AccessConsumed", "GateCreated"][..]] {
        let (ids, steps) = run(c, &q(kinds, None, None));
        assert!(!ids.is_empty());
        assert!(steps < 5_000, "{kinds:?} took {steps} VM steps");
        // A cursor into the older half is as cheap.
        let (_, steps) = run(c, &q(kinds, None, Some(150_000)));
        assert!(steps < 5_000, "{kinds:?} before 150000 took {steps} VM steps");
    }
    // Without the index the same single-kind page walks the stream: the test sees the difference.
    c.execute_batch("DROP INDEX events_kind").unwrap();
    let (_, steps) = run(c, &q(&["AccessConsumed"], None, None));
    assert!(steps > 100_000, "without events_kind the page took only {steps} VM steps");
}

/// Recorded in the audit (F25). Run: `cargo test -p sui-indexer-store --release -- --ignored --nocapture f25`.
#[test]
#[ignore = "measurement, not a gate"]
fn f25_measurement_on_300k_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    let s = Store::open(&path).unwrap();
    fill(s.connection(), 300_000);
    s.connection().execute_batch("DROP INDEX events_kind").unwrap();
    drop(s);

    // The pre-0.0.13 query on the pre-0.0.13 database.
    let old = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let rate = |f: &dyn Fn() -> usize| -> f64 {
        let t = std::time::Instant::now();
        let mut n = 0;
        while t.elapsed().as_millis() < 2000 {
            assert!(f() > 0);
            n += 1;
        }
        f64::from(n) / t.elapsed().as_secs_f64()
    };
    let old_sql = "SELECT id, event_type, sender, bcs, checkpoint, tx_digest, event_index FROM events WHERE stream = ? AND kind IN (?) ORDER BY id DESC LIMIT 21";
    let before = rate(&|| {
        old.prepare_cached(old_sql)
            .unwrap()
            .query_map(["access-gate", "AccessConsumed"], |r| r.get::<_, i64>(0))
            .unwrap()
            .count()
    });
    drop(old);

    // The ingester starts: how long it takes to build the index, and what readers see meanwhile.
    let reader = Reader::open(&path).unwrap();
    let writer = std::thread::spawn({
        let path = path.clone();
        move || {
            let t = std::time::Instant::now();
            let s = Store::open(&path).unwrap();
            (t.elapsed(), s)
        }
    });
    let mut worst = std::time::Duration::ZERO;
    let mut reads = 0;
    while !writer.is_finished() {
        let t = std::time::Instant::now();
        let p = reader.page(STREAM, &PageQuery { kinds: None, gate_id: None, before_id: None, limit: 20 }).unwrap();
        worst = worst.max(t.elapsed());
        assert_eq!(p.rows.len(), 20);
        reads += 1;
    }
    let (built, s) = writer.join().unwrap();
    println!("index build in Store::open: {built:?}; {reads} head pages read meanwhile, slowest {worst:?}");
    assert!(has_index(s.connection(), "events_kind"));

    for (label, kinds) in
        [("one rare kind", &["AccessConsumed"][..]), ("two kinds, one rare", &["AccessConsumed", "GateCreated"][..])]
    {
        let query = q(kinds, None, None);
        let after = rate(&|| reader.page(STREAM, &query).unwrap().rows.len());
        println!("{label}: after {after:.0} pages/s");
    }
    println!("one rare kind, 0.0.12 query and database: {before:.0} pages/s");
    let query = q(&["AccessConsumed"], None, None);
    let after = rate(&|| reader.page(STREAM, &query).unwrap().rows.len());
    println!("one rare kind: before {before:.0} pages/s, after {after:.0} pages/s ({:.0}x)", after / before);
}
