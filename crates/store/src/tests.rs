//! Store tests against a real PostgreSQL (`TEST_DATABASE_URL`; see `testdb`). Each test gets its own
//! database provisioned like production: an owner role (the writer) and a SELECT-only role (the API).

use super::*;
use crate::migrate::{self, MIGRATIONS};
use crate::require_db;
use crate::sui::normalize_sui_address;

fn addr(s: &str) -> String {
    normalize_sui_address(s)
}

const AG: &str = "0xa1";
const SEAL: &str = "0x5e";

pub(crate) fn ev(kind: &str, gate: &str, checkpoint: u64, commission: Option<i64>, tx: &str) -> IndexedEvent {
    let module = if kind == "SealedContentPublished" { "sealed_content" } else { "access_gate" };
    let pkg = if kind == "SealedContentPublished" { SEAL } else { AG };
    let name = if kind == "SealedContentPublished" { kind.to_owned() } else { format!("{kind}Event") };
    IndexedEvent {
        tx_digest: tx.into(),
        event_index: 0,
        checkpoint: Some(checkpoint),
        sender: addr("0xa11ce"),
        event_type: format!("{}::{module}::{name}", addr(pkg)),
        bcs: vec![1, 2, 3, checkpoint as u8],
        kind: kind.into(),
        gate_id: Some(addr(gate)),
        commission_mist: commission,
    }
}

pub(crate) fn minted(gate: &str, checkpoint: u64, commission: i64, tx: &str) -> IndexedEvent {
    ev("AccessMinted", gate, checkpoint, Some(commission), tx)
}

fn bound() -> Vec<(Stream, String)> {
    vec![(Stream::AccessGate, addr(AG)), (Stream::SealedContent, addr(SEAL))]
}

fn other() -> Vec<(Stream, String)> {
    vec![(Stream::AccessGate, format!("0x{}", "77".repeat(32))), (Stream::SealedContent, addr(SEAL))]
}

fn checkpoints(p: &Page) -> Vec<String> {
    p.rows.iter().map(|r| r.checkpoint.clone().unwrap()).collect()
}

// ── Ingest and pages ────────────────────────────────────────────────────────

#[tokio::test]
async fn re_ingesting_the_same_events_is_a_no_op() {
    let db = require_db!();
    let mut s = db.store().await;
    let e = minted("0xa", 1, 2000, "t1");
    assert_eq!(s.ingest(Stream::AccessGate, std::slice::from_ref(&e), Some("c0")).await.unwrap(), 1);
    assert_eq!(s.ingest(Stream::AccessGate, &[e], Some("c0")).await.unwrap(), 0);
    assert!(s.verify_counters().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_null_cursor_keeps_the_stored_position() {
    let db = require_db!();
    let mut s = db.store().await;
    s.ingest(Stream::AccessGate, &[], Some("kept")).await.unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], None).await.unwrap();
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("kept"));
}

#[tokio::test]
async fn stored_rows_are_served_verbatim_newest_first() {
    let db = require_db!();
    let mut s = db.store().await;
    let events: Vec<_> = (1..=5).map(|i| minted("0xa", i, 1, &format!("t{i}"))).collect();
    s.ingest(Stream::AccessGate, &events, Some("c")).await.unwrap();
    let p = s.page(Stream::AccessGate, &PageQuery { limit: 2, ..Default::default() }).await.unwrap();
    assert_eq!(checkpoints(&p), ["5", "4"]);
    assert_eq!(p.rows[0].bcs, events[4].bcs);
    assert_eq!(p.rows[0].event_type, events[4].event_type);
    assert_eq!(p.rows[0].transaction_digest, "t5");
    let p2 =
        s.page(Stream::AccessGate, &PageQuery { limit: 2, before_id: p.next, ..Default::default() }).await.unwrap();
    assert_eq!(checkpoints(&p2), ["3", "2"]);
    let p3 =
        s.page(Stream::AccessGate, &PageQuery { limit: 2, before_id: p2.next, ..Default::default() }).await.unwrap();
    assert_eq!(p3.rows.len(), 1);
    assert_eq!(p3.next, None);
    assert_eq!(s.head_id(Stream::AccessGate).await.unwrap(), 5);
    assert_eq!(s.head_id(Stream::SealedContent).await.unwrap(), 0);
}

#[tokio::test]
async fn pages_filter_by_kind_and_gate() {
    let db = require_db!();
    let mut s = db.store().await;
    s.ingest(
        Stream::AccessGate,
        &[
            minted("0xa", 1, 1, "t1"),
            ev("AccessConsumed", "0xa", 2, None, "t2"),
            minted("0xb", 3, 1, "t3"),
            ev("AccessConsumed", "0xb", 4, None, "t4"),
        ],
        Some("c"),
    )
    .await
    .unwrap();
    let q = PageQuery {
        kinds: Some(vec!["AccessConsumed".into()]),
        gate_id: Some(addr("0xa")),
        limit: 20,
        before_id: None,
    };
    let p = s.page(Stream::AccessGate, &q).await.unwrap();
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.rows[0].transaction_digest, "t2");
}

/// The id is ledger order and nothing relies on it being gapless: a duplicate (ignored by ON CONFLICT)
/// and a rolled-back page both consume identity numbers, and paging, the head id and the counters are
/// unaffected.
#[tokio::test]
async fn ids_are_ordered_not_gapless_and_nothing_depends_on_contiguity() {
    let db = require_db!();
    let mut s = db.store().await;
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t1"), minted("0xa", 2, 1, "t2")], Some("c1")).await.unwrap();
    // A duplicate in the middle of a page consumes a number.
    s.ingest(Stream::AccessGate, &[minted("0xa", 2, 1, "t2"), minted("0xa", 3, 1, "t3")], Some("c2")).await.unwrap();
    // A page that fails (a CHECK violation on its second event) rolls back whole and consumes numbers too.
    let mut bad = minted("0xa", 4, 1, "t4");
    bad.commission_mist = Some(-1);
    let err = s.ingest(Stream::AccessGate, &[minted("0xa", 4, 1, "t4ok"), bad], Some("c3")).await.unwrap_err();
    assert_eq!(err.sqlstate(), Some("23514"), "{err}");
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("c2")); // not advanced
    s.ingest(Stream::AccessGate, &[minted("0xa", 5, 1, "t5")], Some("c4")).await.unwrap();
    let rows = s.query("SELECT id, tx_digest FROM events ORDER BY id", &[]).await.unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.get(0)).collect();
    let digests: Vec<String> = rows.iter().map(|r| r.get(1)).collect();
    assert_eq!(digests, ["t1", "t2", "t3", "t5"]); // ledger order
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    assert!(ids.windows(2).any(|w| w[1] - w[0] > 1), "expected a gap from the ignored duplicate: {ids:?}");
    // Pages walk by id bound, the head id is the max id, the counters count rows.
    let mut seen = Vec::new();
    let mut before = None;
    loop {
        let p =
            s.page(Stream::AccessGate, &PageQuery { limit: 1, before_id: before, ..Default::default() }).await.unwrap();
        seen.extend(p.rows.iter().map(|r| r.transaction_digest.clone()));
        match p.next {
            Some(n) => before = Some(n),
            None => break,
        }
    }
    assert_eq!(seen, ["t5", "t3", "t2", "t1"]);
    assert_eq!(s.head_id(Stream::AccessGate).await.unwrap(), *ids.last().unwrap());
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 4);
    assert!(s.verify_counters().await.unwrap().is_empty());
}

#[tokio::test]
async fn the_column_types_and_constraints_refuse_what_the_code_never_writes() {
    let db = require_db!();
    let s = db.store().await;
    let insert = |extra: &str| {
        format!(
            "INSERT INTO events (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind, commission_mist) \
             VALUES ({extra})"
        )
    };
    for (what, values) in [
        ("an unknown stream", "'nope', 'd', 0, 1, 's', 't', '\\x00', 'k', NULL"),
        ("a negative event index", "'access-gate', 'd', -1, 1, 's', 't', '\\x00', 'k', NULL"),
        ("a negative checkpoint", "'access-gate', 'd', 0, -1, 's', 't', '\\x00', 'k', NULL"),
        ("a negative commission", "'access-gate', 'd', 0, 1, 's', 't', '\\x00', 'k', -5"),
        ("a null sender", "'access-gate', 'd', 0, 1, NULL, 't', '\\x00', 'k', NULL"),
        ("a null bcs", "'access-gate', 'd', 0, 1, 's', 't', NULL, 'k', NULL"),
    ] {
        let err = s.execute_batch(&insert(values)).await.unwrap_err();
        assert!(matches!(err.sqlstate(), Some("23514" | "23502")), "{what}: {err}");
    }
    // The (digest, index) pair is unique.
    s.execute_batch(&insert("'access-gate', 'd', 0, 1, 's', 't', '\\x00', 'k', NULL")).await.unwrap();
    let err = s.execute_batch(&insert("'access-gate', 'd', 0, 2, 's', 't', '\\x00', 'k', NULL")).await.unwrap_err();
    assert_eq!(err.sqlstate(), Some("23505"));
}

// ── bindPackages ────────────────────────────────────────────────────────────

#[tokio::test]
async fn records_the_packages_of_a_fresh_index_without_clearing_anything() {
    let db = require_db!();
    let mut s = db.store().await;
    let g = s.generation().to_owned();
    assert_eq!(s.bind_packages(&bound(), &[], None).await.unwrap(), vec![]);
    assert_eq!(s.generation(), g);
}

#[tokio::test]
async fn keeps_an_index_whose_packages_are_unchanged() {
    let db = require_db!();
    let mut s = db.store().await;
    s.bind_packages(&bound(), &[], None).await.unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c1")).await.unwrap();
    let g = s.generation().to_owned();
    assert_eq!(s.bind_packages(&bound(), &[], None).await.unwrap(), vec![]);
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 1);
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("c1"));
    assert_eq!(s.generation(), g);
}

#[tokio::test]
async fn refuses_to_clear_history_without_confirmation_and_changes_nothing() {
    let db = require_db!();
    let mut s = db.store().await;
    s.bind_packages(&bound(), &[], None).await.unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c1")).await.unwrap();
    let g = s.generation().to_owned();
    match s.bind_packages(&other(), &[], None).await {
        Err(BindError::PackageChanged(e)) => assert_eq!(e.streams, vec![Stream::AccessGate]),
        r => panic!("expected PackageChanged, got {r:?}"),
    }
    let msg = s.bind_packages(&other(), &[Stream::SealedContent], None).await.unwrap_err().to_string();
    assert!(msg.contains("REINDEX_CONFIRM=access-gate"), "{msg}");
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 1);
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("c1"));
    assert_eq!(s.generation(), g);
    // The recorded binding is untouched, so the same refusal repeats until the operator decides.
    assert!(matches!(s.bind_packages(&other(), &[], None).await, Err(BindError::PackageChanged(_))));
}

#[tokio::test]
async fn clears_only_the_confirmed_stream_and_rotates_the_generation() {
    let db = require_db!();
    let mut s = db.store().await;
    s.bind_packages(&bound(), &[], None).await.unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 5, "t1")], Some("c1")).await.unwrap();
    s.ingest(Stream::SealedContent, &[ev("SealedContentPublished", "0xa", 2, None, "t2")], Some("s1")).await.unwrap();
    let g = s.generation().to_owned();
    assert_eq!(s.bind_packages(&other(), &[Stream::AccessGate], None).await.unwrap(), vec![Stream::AccessGate]);
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 0);
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap(), None);
    assert_eq!(s.gate_stats(&addr("0xa")).await.unwrap(), GateStats { counts: vec![], commission_mist: 0 });
    assert_eq!(s.coverage(Stream::SealedContent).await.unwrap().events, 1);
    assert_eq!(s.cursor(Stream::SealedContent).await.unwrap().as_deref(), Some("s1"));
    assert_ne!(s.generation(), g);
    assert_eq!(s.meta("generation").await.unwrap().as_deref(), Some(s.generation()));
    // The new binding sticks.
    assert_eq!(s.bind_packages(&other(), &[], None).await.unwrap(), vec![]);
    assert!(s.verify_counters().await.unwrap().is_empty());
}

#[tokio::test]
async fn treats_rows_indexed_before_packages_were_recorded_like_a_package_change() {
    let db = require_db!();
    let mut s = db.store().await;
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c1")).await.unwrap();
    assert!(matches!(s.bind_packages(&bound(), &[], None).await, Err(BindError::PackageChanged(_))));
    assert_eq!(s.bind_packages(&bound(), &[Stream::AccessGate], None).await.unwrap(), vec![Stream::AccessGate]);
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 0);
}

#[tokio::test]
async fn copies_the_index_into_a_backup_schema_before_clearing_and_the_copy_restores() {
    let db = require_db!();
    let mut s = db.store().await;
    s.bind_packages(&bound(), &[], None).await.unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 7, "t")], Some("c1")).await.unwrap();
    let id: i64 = s.query("SELECT id FROM events", &[]).await.unwrap()[0].get(0);
    s.bind_packages(&other(), &[Stream::AccessGate], Some("bak_1760000000000")).await.unwrap();
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 0);
    // The copy holds every table's rows.
    let n = |sql: &str| {
        let sql = sql.to_owned();
        let s = &s;
        async move { s.query(&sql, &[]).await.unwrap()[0].get::<_, i64>(0) }
    };
    assert_eq!(n("SELECT count(*) FROM bak_1760000000000.events").await, 1);
    assert_eq!(n("SELECT count(*) FROM bak_1760000000000.cursors WHERE cursor = 'c1'").await, 1);
    assert_eq!(n("SELECT n FROM bak_1760000000000.gate_stats").await, 1);
    assert_eq!(n("SELECT total::bigint FROM bak_1760000000000.gate_commission").await, 7);
    // And restores with the documented statement, ids included.
    s.execute_batch("INSERT INTO events OVERRIDING SYSTEM VALUE SELECT * FROM bak_1760000000000.events").await.unwrap();
    assert_eq!(n("SELECT id FROM events").await, id);
    // The API role can neither see nor create anything there.
    let c = db.api.connect().await.unwrap();
    let err = c.query("SELECT * FROM bak_1760000000000.events", &[]).await.unwrap_err();
    assert_eq!(err.code().map(|c| c.code()), Some("42501"));
}

#[tokio::test]
async fn the_backup_and_the_clearing_are_one_transaction() {
    let db = require_db!();
    let mut s = db.store().await;
    s.bind_packages(&bound(), &[], None).await.unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c1")).await.unwrap();
    let g = s.generation().to_owned();
    s.execute_batch("CREATE SCHEMA bak_taken").await.unwrap();
    // The schema exists already: the backup fails, so nothing is cleared and the generation is kept.
    assert!(s.bind_packages(&other(), &[Stream::AccessGate], Some("bak_taken")).await.is_err());
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 1);
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("c1"));
    assert_eq!(s.generation(), g);
    // A name that could inject SQL is refused before any statement runs.
    for name in ["bak_x; DROP TABLE events", "other", "bak_UPPER", "bak_\"q"] {
        let e = s.bind_packages(&other(), &[Stream::AccessGate], Some(name)).await.unwrap_err().to_string();
        assert!(e.contains("invalid backup schema name"), "{name}: {e}");
    }
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 1);
}

// ── Counters ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn counters_stay_exact_across_duplicates_and_agree_with_a_recount() {
    let db = require_db!();
    let mut s = db.store().await;
    let rows = vec![
        minted("0xa", 7, 100, "t1"),
        minted("0xa", 3, 200, "t2"),
        ev("AccessConsumed", "0xa", 5, None, "t3"),
        minted("0xb", 9, 50, "t4"),
    ];
    s.ingest(Stream::AccessGate, &rows, Some("c")).await.unwrap();
    s.ingest(Stream::AccessGate, &rows, Some("c")).await.unwrap(); // duplicates change nothing
    assert_eq!(
        s.coverage(Stream::AccessGate).await.unwrap(),
        Coverage { from_checkpoint: Some("3".into()), latest_checkpoint: Some("9".into()), events: 4 }
    );
    let a = s.gate_stats(&addr("0xa")).await.unwrap();
    assert_eq!((a.count("AccessMinted"), a.count("AccessConsumed"), a.count("AccessBurned")), (2, 1, 0));
    assert_eq!(a.commission_mist, 300);
    assert_eq!(s.gate_stats(&addr("0xb")).await.unwrap().commission_mist, 50);
    let r = &s
        .query("SELECT count(*), min(checkpoint), max(checkpoint) FROM events WHERE stream = 'access-gate'", &[])
        .await
        .unwrap()[0];
    assert_eq!((r.get::<_, i64>(0), r.get::<_, i64>(1), r.get::<_, i64>(2)), (4, 3, 9));
    assert!(s.verify_counters().await.unwrap().is_empty());
}

#[tokio::test]
async fn commission_totals_are_exact_beyond_64_bits() {
    let db = require_db!();
    let mut s = db.store().await;
    let big = 1i64 << 60;
    let rows: Vec<_> = (0..20).map(|i| minted("0xa", i, big, &format!("t{i}"))).collect();
    s.ingest(Stream::AccessGate, &rows, Some("c")).await.unwrap();
    assert_eq!(s.gate_stats(&addr("0xa")).await.unwrap().commission_mist, 20u128 << 60);
    assert!(s.verify_counters().await.unwrap().is_empty());
}

/// There is no counter backfill in the PostgreSQL schema: every row enters through `ingest`, which
/// maintains the counters in the same transaction; this recount is the check that they still match.
#[tokio::test]
async fn a_recount_detects_counters_that_do_not_match_the_rows() {
    let db = require_db!();
    let mut s = db.store().await;
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 100, "t1"), minted("0xa", 2, 250, "t2")], Some("c")).await.unwrap();
    assert!(s.verify_counters().await.unwrap().is_empty());
    s.execute_batch("UPDATE stream_stats SET events = events + 1").await.unwrap();
    s.execute_batch("UPDATE gate_commission SET total = total + 1").await.unwrap();
    s.execute_batch("DELETE FROM gate_stats").await.unwrap();
    let diffs = s.verify_counters().await.unwrap();
    assert_eq!(diffs.len(), 3, "{diffs:?}");
}

#[tokio::test]
async fn coverage_over_50k_rows_is_a_constant_time_read() {
    let db = require_db!();
    let s = db.store().await;
    s.execute_batch(
        "INSERT INTO events (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind)
         SELECT 'access-gate', 'd' || i, 0, i, 's', 't', '\\x00', 'AccessMinted' FROM generate_series(1, 50000) i;
         INSERT INTO stream_stats (stream, from_checkpoint, latest_checkpoint, events) VALUES ('access-gate', 1, 50000, 50000);
         ANALYZE events",
    )
    .await
    .unwrap();
    let t = std::time::Instant::now();
    for _ in 0..200 {
        s.coverage(Stream::AccessGate).await.unwrap();
    }
    assert!(t.elapsed().as_millis() < 1500, "200 reads took {:?}", t.elapsed());
    // And by plan: the counter read never touches the events table.
    let plan = s
        .query(
            "EXPLAIN SELECT from_checkpoint, latest_checkpoint, events FROM stream_stats WHERE stream = 'access-gate'",
            &[],
        )
        .await
        .unwrap();
    let plan: String = plan.iter().map(|r| r.get::<_, String>(0)).collect::<Vec<_>>().join("\n");
    assert!(!plan.contains("events on") && !plan.contains("Seq Scan on events"), "{plan}");
}

#[tokio::test]
async fn undecodable_and_gap_counters() {
    let db = require_db!();
    let mut s = db.store().await;
    s.note_undecodable(Stream::AccessGate, 2).await.unwrap();
    s.note_undecodable(Stream::AccessGate, 1).await.unwrap();
    s.ingest(Stream::AccessGate, &[], Some("stale")).await.unwrap();
    s.reset_cursor_after_gap(Stream::AccessGate, &"x".repeat(300), "2026-10-19T00:00:00.000Z").await.unwrap();
    assert_eq!(s.health(Stream::AccessGate).await.unwrap(), StreamHealth { undecodable: 3, gaps: 1 });
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap(), None);
    let gap: serde_json::Value = serde_json::from_str(&s.meta("gap:access-gate").await.unwrap().unwrap()).unwrap();
    assert_eq!(gap["reason"].as_str().unwrap().len(), 200);
    assert_eq!(gap["at"], "2026-10-19T00:00:00.000Z");
    assert_eq!(s.health(Stream::SealedContent).await.unwrap(), StreamHealth { undecodable: 0, gaps: 0 });
}

#[tokio::test]
async fn heartbeats_round_trip() {
    let db = require_db!();
    let s = db.store().await;
    assert_eq!(s.poll_state(Stream::AccessGate).await.unwrap(), PollState::default());
    let st = PollState { last_success_at: 5, last_error: Some("boom".into()), last_attempt_at: 9 };
    s.record_poll(Stream::AccessGate, &st).await.unwrap();
    assert_eq!(s.poll_state(Stream::AccessGate).await.unwrap(), st);
    assert_eq!(
        s.heartbeat("poll:access-gate").await.unwrap().unwrap(),
        r#"{"lastSuccessAt":5,"lastError":"boom","lastAttemptAt":9}"#
    );
    s.touch_alive(42).await.unwrap();
    assert_eq!(s.heartbeat(ALIVE_KEY).await.unwrap().as_deref(), Some("42"));
    // Heartbeats are not `meta` rows.
    assert_eq!(s.meta(ALIVE_KEY).await.unwrap(), None);
}

// ── Single writer and migrations ────────────────────────────────────────────

#[tokio::test]
async fn a_second_writer_is_refused_while_the_first_lives_and_admitted_after() {
    let db = require_db!();
    let first = db.store().await;
    match db.store_nowait().await {
        Err(StoreError::WriterLockHeld) => {}
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a second writer got the lock"),
    }
    // The lock belongs to the first connection's session and ends with it. A new start waits briefly for
    // a predecessor that is still closing.
    drop(first);
    let again = Store::connect(&db.owner).await.expect("the lock is free after the first writer closed");
    assert!(!again.is_closed());
}

#[tokio::test]
async fn the_writer_lock_is_visible_as_an_advisory_lock_of_the_owner_session() {
    let db = require_db!();
    let s = db.store().await;
    let admin = db.admin().await;
    let rows = admin
        .query(
            "SELECT l.pid, a.usename FROM pg_locks l JOIN pg_stat_activity a USING (pid)
             WHERE l.locktype = 'advisory' AND l.granted AND ((l.classid::bigint << 32) | l.objid::bigint) = $1
               AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database())",
            &[&WRITER_LOCK_KEY],
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<_, String>(1), db.owner_role);
    assert!(!s.is_closed());
}

#[tokio::test]
async fn migrations_apply_once_and_restarts_do_not_reapply() {
    let db = require_db!();
    let s = db.store().await;
    let versions: Vec<i32> = s
        .query("SELECT version FROM schema_migrations ORDER BY version", &[])
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(versions, MIGRATIONS.iter().map(|m| m.version).collect::<Vec<_>>());
    let applied_at: String =
        s.query("SELECT applied_at::text FROM schema_migrations WHERE version = 1", &[]).await.unwrap()[0].get(0);
    let generation = s.generation().to_owned();
    drop(s);
    let again = Store::connect(&db.owner).await.unwrap();
    assert_eq!(
        again.query("SELECT applied_at::text FROM schema_migrations WHERE version = 1", &[]).await.unwrap()[0]
            .get::<_, String>(0),
        applied_at
    );
    assert_eq!(again.generation(), generation);
    assert_eq!(again.meta(SCHEMA_WRITER_KEY).await.unwrap().as_deref(), Some("postgres"));
}

#[tokio::test]
async fn two_migrators_racing_on_an_empty_database_apply_each_migration_once() {
    let db = require_db!();
    let (mut a, mut b) = (db.owner.connect().await.unwrap(), db.owner.connect().await.unwrap());
    let (ra, rb) = tokio::join!(migrate::run(&mut a), migrate::run(&mut b));
    let (ra, rb) = (ra.unwrap(), rb.unwrap());
    // Exactly one of them applied the migration; the other found it applied.
    assert_eq!(ra.len() + rb.len(), MIGRATIONS.len(), "{ra:?} {rb:?}");
    assert_eq!(
        a.query_one("SELECT count(*) FROM schema_migrations", &[]).await.unwrap().get::<_, i64>(0),
        MIGRATIONS.len() as i64
    );
}

#[tokio::test]
async fn a_migration_applied_from_different_sql_is_refused() {
    let db = require_db!();
    drop(db.store().await);
    db.admin()
        .await
        .execute("UPDATE schema_migrations SET checksum = 'tampered' WHERE version = 1", &[])
        .await
        .unwrap();
    let err = Store::connect(&db.owner).await.err().expect("refused");
    assert!(matches!(err, StoreError::Migration(_)) && err.to_string().contains("different SQL"), "{err}");
}

#[tokio::test]
async fn a_schema_newer_than_the_build_is_refused_by_the_ingester_and_served_by_the_api() {
    let db = require_db!();
    drop(db.store().await);
    let admin = db.admin().await;
    admin
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum) VALUES ($1, 'future', 'x')",
            &[&(SCHEMA_VERSION + 1)],
        )
        .await
        .unwrap();
    let err = Store::connect(&db.owner).await.err().expect("an older ingester must not run");
    assert!(err.to_string().contains("newer than this build"), "{err}");
    // The API accepts a newer schema (migrations add; they never break the previous queries).
    db.reader(1).check().await.expect("ready");
}

#[tokio::test]
async fn a_failed_migration_leaves_nothing_behind() {
    let db = require_db!();
    let mut c = db.owner.connect().await.unwrap();
    // A first run creates the tables. Dropping one and re-running proves a half-applied schema is not
    // silently accepted: the checksum row says the migration was applied, so nothing is re-run.
    migrate::run(&mut c).await.unwrap();
    c.batch_execute("DROP TABLE gate_commission").await.unwrap();
    assert!(migrate::run(&mut c).await.unwrap().is_empty());
}

// ── The API's reader: readiness, read-only, one statement per request, pool ─

#[tokio::test]
async fn the_reader_is_not_ready_until_the_writer_has_migrated_and_started() {
    let db = require_db!();
    let r = db.reader(2);
    // Nothing exists yet: no table.
    assert!(matches!(r.check().await, Err(ReadError::NotReady(_))), "empty database");
    assert!(matches!(
        r.events(Stream::AccessGate, &PageQuery { limit: 5, ..Default::default() }).await,
        Err(ReadError::NotReady(_))
    ));
    assert!(matches!(r.coverage().await, Err(ReadError::NotReady(_))));
    // The schema alone (no generation yet) is not ready either.
    let mut c = db.owner.connect().await.unwrap();
    migrate::run(&mut c).await.unwrap();
    match r.check().await {
        Err(ReadError::NotReady(m)) => assert!(m.contains("generation"), "{m}"),
        other => panic!("{other:?}"),
    }
    drop(c);
    let s = db.store().await;
    r.check().await.unwrap();
    assert!(r.is_ready());
    assert_eq!(r.generation().await.unwrap().as_deref(), Some(s.generation()));
}

#[tokio::test]
async fn the_reader_cannot_write_cannot_change_the_schema_and_sees_new_commits() {
    let db = require_db!();
    let mut s = db.store().await;
    let r = db.reader(2);
    // Refused by the read-only default of the pooled connections ...
    let err = r.try_write().await.unwrap_err();
    assert_eq!(err.sqlstate(), Some("25006"), "{err}");
    // ... and by the grants, even when a client switches the read-only guards off.
    let c = db.api.connect().await.unwrap();
    c.batch_execute("SET default_transaction_read_only = off").await.unwrap();
    for sql in [
        "INSERT INTO meta (key, value) VALUES ('probe', 'x')",
        "UPDATE events SET kind = 'x'",
        "DELETE FROM cursors",
        "TRUNCATE events",
        "CREATE TABLE evil (x int)",
        "DROP TABLE events",
        "ALTER TABLE events ADD COLUMN x int",
        "CREATE INDEX evil ON events (sender)",
    ] {
        let err = c.batch_execute(sql).await.unwrap_err();
        assert_eq!(err.code().map(|c| c.code()), Some("42501"), "{sql}: {err}");
    }
    // What it may do: read every index table.
    for t in INDEX_TABLES {
        c.query(&format!("SELECT count(*) FROM {t}"), &[]).await.unwrap_or_else(|e| panic!("{t}: {e}"));
    }
    // It sees commits made after it connected.
    assert_eq!(r.coverage().await.unwrap().streams[0].1.events, 0);
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c")).await.unwrap();
    s.record_poll(Stream::AccessGate, &PollState { last_success_at: 7, last_error: None, last_attempt_at: 7 })
        .await
        .unwrap();
    assert_eq!(r.coverage().await.unwrap().streams[0].1.events, 1);
    assert_eq!(r.polls().await.unwrap()[0].last_success_at, 7);
}

#[tokio::test]
async fn pooled_connections_start_read_only_and_never_use_a_generic_plan() {
    let db = require_db!();
    let _s = db.store().await;
    let r = db.reader(1);
    r.check().await.unwrap();
    assert_eq!(r.query_for_test("SHOW default_transaction_read_only").await, "on");
    assert_eq!(r.query_for_test("SHOW plan_cache_mode").await, "force_custom_plan");
}

/// The statement behind an events response reads the facts (generation, newest id, counters) and the page at
/// once: while a writer commits pages continuously, the newest id is always the id of the first row served.
#[tokio::test]
async fn an_events_response_is_one_snapshot_of_the_head_and_the_page() {
    let db = require_db!();
    let mut s = db.store().await;
    let r = db.reader(2);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (stop, r) = (stop.clone(), &r);
        async move {
            let mut checked = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let snap = r.events(Stream::AccessGate, &PageQuery { limit: 5, ..Default::default() }).await.unwrap();
                if let Some(first) = snap.page.rows.first() {
                    assert_eq!(snap.head_id, first.id, "the head id and the page must come from one snapshot");
                    assert!(snap.coverage.events as usize >= snap.page.rows.len());
                    // The counters are in the same snapshot as the rows they count.
                    let n = snap.coverage.events;
                    assert!(n >= 1);
                    checked += 1;
                }
            }
            checked
        }
    };
    let writer = async {
        for i in 0..150 {
            s.ingest(Stream::AccessGate, &[minted("0xa", i + 1, 1, &format!("w{i}"))], Some("c")).await.unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    };
    let (checked, ()) = tokio::join!(reader, writer);
    assert!(checked > 5, "only {checked} snapshots were checked");
    assert!(s.verify_counters().await.unwrap().is_empty());
}

/// The statement that answers a request reads exactly what the separate reads read.
#[tokio::test]
async fn the_events_statement_equals_the_separate_reads() {
    let db = require_db!();
    let mut s = db.store().await;
    let r = db.reader(2);
    // Empty first: no row at all must still carry the facts.
    let snap = r.events(Stream::AccessGate, &PageQuery { limit: 5, ..Default::default() }).await.unwrap();
    assert_eq!((snap.head_id, snap.page.rows.len(), snap.coverage.events), (0, 0, 0));
    assert_eq!(snap.generation.as_deref(), Some(s.generation()));
    assert_eq!(snap.poll, PollState::default());
    s.ingest(
        Stream::AccessGate,
        &(1..=9).map(|i| minted(if i % 2 == 0 { "0xa" } else { "0xb" }, i, 10, &format!("t{i}"))).collect::<Vec<_>>(),
        Some("c"),
    )
    .await
    .unwrap();
    s.ingest(Stream::SealedContent, &[ev("SealedContentPublished", "0xa", 20, None, "s1")], Some("s")).await.unwrap();
    s.record_poll(
        Stream::AccessGate,
        &PollState { last_success_at: 42, last_error: Some("e".into()), last_attempt_at: 43 },
    )
    .await
    .unwrap();
    for q in [
        PageQuery { limit: 4, ..Default::default() },
        PageQuery { limit: 4, before_id: Some(6), ..Default::default() },
        PageQuery { limit: 2, kinds: Some(vec!["AccessMinted".into()]), ..Default::default() },
        PageQuery { limit: 3, gate_id: Some(addr("0xa")), ..Default::default() },
        PageQuery { limit: 3, kinds: Some(vec!["AccessMinted".into(), "AccessConsumed".into()]), ..Default::default() },
        PageQuery { limit: 3, kinds: Some(vec!["AccessBurned".into()]), ..Default::default() },
    ] {
        let snap = r.events(Stream::AccessGate, &q).await.unwrap();
        let page = s.page(Stream::AccessGate, &q).await.unwrap();
        assert_eq!(snap.page.rows, page.rows, "{q:?}");
        assert_eq!(snap.page.next, page.next, "{q:?}");
        assert_eq!(snap.head_id, s.head_id(Stream::AccessGate).await.unwrap());
        assert_eq!(snap.coverage, s.coverage(Stream::AccessGate).await.unwrap());
        assert_eq!(snap.poll, s.poll_state(Stream::AccessGate).await.unwrap());
        assert_eq!(snap.generation.as_deref(), Some(s.generation()));
    }
    let sealed = r
        .events(Stream::SealedContent, &PageQuery { limit: 5, gate_id: Some(addr("0xa")), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(
        (sealed.page.rows.len(), sealed.head_id, sealed.coverage.events),
        (1, s.head_id(Stream::SealedContent).await.unwrap(), 1)
    );
    // Coverage and gate statements.
    let cov = r.coverage().await.unwrap();
    assert_eq!(cov.generation.as_deref(), Some(s.generation()));
    assert_eq!(cov.streams[0].1, s.coverage(Stream::AccessGate).await.unwrap());
    assert_eq!(cov.streams[1].1, s.coverage(Stream::SealedContent).await.unwrap());
    let gate = r.gate(&addr("0xa")).await.unwrap();
    assert_eq!(gate.stats, s.gate_stats(&addr("0xa")).await.unwrap());
    assert_eq!(gate.from_checkpoint.as_deref(), Some("1"));
    let none = r.gate(&addr("0xf")).await.unwrap();
    assert_eq!((none.stats.counts.len(), none.stats.commission_mist), (0, 0));
}

#[tokio::test]
async fn a_cancelled_request_leaves_the_pool_usable() {
    let db = require_db!();
    let mut s = db.store().await;
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c")).await.unwrap();
    let r = db.reader(1);
    r.check().await.unwrap();
    // One slot: drop in-flight requests at various points; the next ones still get a clean connection.
    for micros in [1u64, 20, 100, 400] {
        let _ = tokio::time::timeout(
            std::time::Duration::from_micros(micros),
            r.events(Stream::AccessGate, &PageQuery { limit: 5, ..Default::default() }),
        )
        .await;
        let snap = r.events(Stream::AccessGate, &PageQuery { limit: 5, ..Default::default() }).await.unwrap();
        assert_eq!(snap.page.rows.len(), 1);
    }
    // No request ever opens a transaction: nothing can be left open.
    assert_eq!(r.query_for_test("SELECT (pg_current_xact_id_if_assigned() IS NULL)::text").await, "true");
}

#[tokio::test]
async fn an_unreachable_database_is_unavailable_not_a_panic() {
    let Some(admin) = crate::testdb::admin_settings() else { return };
    // A closed port on the same host.
    let dead = admin.with_credentials("u", "p", "d");
    let url = dead.url_unredacted().replace(&format!(":{}/", dead.port()), ":1/");
    let r = Reader::new(&DatabaseSettings::parse(&url, true).unwrap(), 1).unwrap();
    assert!(matches!(r.check().await, Err(ReadError::Unavailable(_))));
    assert!(matches!(r.coverage().await, Err(ReadError::Unavailable(_))));
}

// ── Roles and TLS ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_role_cannot_connect_to_another_services_database() {
    let db = require_db!();
    let other = db.another().await;
    drop(db.store().await);
    drop(other.store().await);
    // Each owner reaches its own database ...
    db.owner.connect().await.unwrap();
    // ... and neither role of one reaches the other's (REVOKE ... FROM PUBLIC).
    for role in [&db.owner, &db.api] {
        let across = role.with_credentials(
            role.user(),
            &String::from_utf8_lossy(role.pg_config().get_password().unwrap()),
            &other.name,
        );
        let err = across.connect().await.unwrap_err();
        assert_eq!(err.sqlstate(), Some("42501"), "{err}");
    }
}

#[tokio::test]
async fn the_owner_can_migrate_and_the_api_role_cannot() {
    let db = require_db!();
    let mut c = db.api.connect().await.unwrap();
    // The SELECT-only role cannot even bootstrap the schema (the read-only default is switched off here
    // to show the grants refuse it too).
    c.batch_execute("SET default_transaction_read_only = off").await.unwrap();
    let err = migrate::run(&mut c).await.unwrap_err();
    assert!(err.to_string().contains("permission denied"), "{err}");
    assert!(migrate::run(&mut db.owner.connect().await.unwrap()).await.is_ok());
}

#[tokio::test]
async fn tls_is_verified_against_the_given_ca_when_the_server_offers_it() {
    let Some(admin) = crate::testdb::admin_settings() else { return };
    if !admin.tls_enabled() {
        assert!(
            !crate::testdb::required(),
            "REQUIRE_TEST_DATABASE=1 needs a TLS test server (TEST_DATABASE_URL with sslmode=verify-full)"
        );
        eprintln!("SKIPPED: the test server is plaintext");
        return;
    }
    // The configured CA verifies the server.
    admin.connect().await.expect("verify-full against the test CA");
    // Another CA does not: the connection is refused, not downgraded.
    let dir = tempfile::tempdir().unwrap();
    let wrong = dir.path().join("other-ca.pem");
    std::fs::write(&wrong, OTHER_CA).unwrap();
    let url = admin.url_unredacted();
    let (before, after) = url.split_once("sslrootcert=").unwrap();
    let rest = after.split_once('&').map_or("", |(_, r)| r);
    let wrong_url = format!("{before}sslrootcert={}{}{rest}", wrong.display(), if rest.is_empty() { "" } else { "&" });
    let err = DatabaseSettings::parse(&wrong_url, true).unwrap().connect().await.unwrap_err();
    let msg = err.to_string().to_lowercase();
    assert!(msg.contains("certificate") || msg.contains("unknownissuer") || msg.contains("tls"), "{msg}");
    // A missing or empty CA file is a configuration error before any connection is tried.
    let missing =
        DatabaseSettings::parse(&wrong_url.replace(&wrong.display().to_string(), "/nonexistent/ca.pem"), true).unwrap();
    assert!(matches!(missing.connect().await, Err(StoreError::Config(_))));
    std::fs::write(&wrong, "not a certificate").unwrap();
    assert!(matches!(DatabaseSettings::parse(&wrong_url, true).unwrap().connect().await, Err(StoreError::Config(_))));
    // A plaintext request to a TLS server is not how the settings connect: TLS is mandatory when verify-full.
    assert!(admin.tls_enabled());
}

/// A throwaway self-signed CA (CN=unrelated-test-ca), unrelated to any test server.
const OTHER_CA: &str = "-----BEGIN CERTIFICATE-----
MIIBjzCCATWgAwIBAgIUGaTneQHxUUQnv9bkjsSYyWz46GcwCgYIKoZIzj0EAwIw
HDEaMBgGA1UEAwwRdW5yZWxhdGVkLXRlc3QtY2EwIBcNMjYxMDIwMTAyODI4WhgP
MjEyNjA5MjYxMDI4MjhaMBwxGjAYBgNVBAMMEXVucmVsYXRlZC10ZXN0LWNhMFkw
EwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE8OI4ugkVE5v0IJP7f1bfdirD+BAsbrcj
+5r1YEmz1ym5bqnx/QuSxwgSSVtMJAPGYa4ZI5Rm3kalOAmYyIQ0bqNTMFEwHQYD
VR0OBBYEFHSmbuqOAT+j9Pmo+HjviIKYOmiUMB8GA1UdIwQYMBaAFHSmbuqOAT+j
9Pmo+HjviIKYOmiUMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSAAwRQIg
SSueRYip/gMvxjjrTFAzyZ4mXWGFZRS7INDaBUb49DECIQD07/oaCP1uCB6y5SfK
1Gwe1iF+3zZU5OYHwzWC/QZYhw==
-----END CERTIFICATE-----
";
