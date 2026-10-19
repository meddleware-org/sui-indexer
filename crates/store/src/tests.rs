use super::*;
use crate::sui::normalize_sui_address;

fn addr(s: &str) -> String {
    normalize_sui_address(s)
}

const AG: &str = "0xa1";
const SEAL: &str = "0x5e";

fn ev(kind: &str, gate: &str, checkpoint: u64, commission: Option<i64>, tx: &str) -> IndexedEvent {
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

fn minted(gate: &str, checkpoint: u64, commission: i64, tx: &str) -> IndexedEvent {
    ev("AccessMinted", gate, checkpoint, Some(commission), tx)
}

/// For `index_tests`: a minted event on `gate`.
pub(crate) fn minted_for_index(gate: &str, checkpoint: u64, tx: &str) -> IndexedEvent {
    minted(gate, checkpoint, 1, tx)
}

fn bound() -> Vec<(Stream, String)> {
    vec![(Stream::AccessGate, addr(AG)), (Stream::SealedContent, addr(SEAL))]
}

fn other() -> Vec<(Stream, String)> {
    vec![(Stream::AccessGate, format!("0x{}", "77".repeat(32))), (Stream::SealedContent, addr(SEAL))]
}

fn mem() -> Store {
    Store::open(":memory:").unwrap()
}

#[test]
fn re_ingesting_the_same_events_is_a_no_op() {
    let s = mem();
    let e = minted("0xa", 1, 2000, "t1");
    assert_eq!(s.ingest(Stream::AccessGate, std::slice::from_ref(&e), Some("c0")).unwrap(), 1);
    assert_eq!(s.ingest(Stream::AccessGate, &[e], Some("c0")).unwrap(), 0);
}

#[test]
fn a_null_cursor_keeps_the_stored_position() {
    let s = mem();
    s.ingest(Stream::AccessGate, &[], Some("kept")).unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], None).unwrap();
    assert_eq!(s.cursor(Stream::AccessGate).unwrap().as_deref(), Some("kept"));
}

#[test]
fn stored_rows_are_served_verbatim_newest_first() {
    let s = mem();
    let events: Vec<_> = (1..=5).map(|i| minted("0xa", i, 1, &format!("t{i}"))).collect();
    s.ingest(Stream::AccessGate, &events, Some("c")).unwrap();
    let p = s.page(Stream::AccessGate, &PageQuery { limit: 2, ..Default::default() }).unwrap();
    assert_eq!(p.rows.iter().map(|r| r.checkpoint.clone().unwrap()).collect::<Vec<_>>(), ["5", "4"]);
    assert_eq!(p.rows[0].bcs, events[4].bcs);
    assert_eq!(p.rows[0].event_type, events[4].event_type);
    assert_eq!(p.rows[0].transaction_digest, "t5");
    let p2 = s.page(Stream::AccessGate, &PageQuery { limit: 2, before_id: p.next, ..Default::default() }).unwrap();
    assert_eq!(p2.rows.iter().map(|r| r.checkpoint.clone().unwrap()).collect::<Vec<_>>(), ["3", "2"]);
    let p3 = s.page(Stream::AccessGate, &PageQuery { limit: 2, before_id: p2.next, ..Default::default() }).unwrap();
    assert_eq!(p3.rows.len(), 1);
    assert_eq!(p3.next, None);
    assert_eq!(s.head_id(Stream::AccessGate).unwrap(), 5);
    assert_eq!(s.head_id(Stream::SealedContent).unwrap(), 0);
}

#[test]
fn pages_filter_by_kind_and_gate() {
    let s = mem();
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
    .unwrap();
    let q = PageQuery {
        kinds: Some(vec!["AccessConsumed".into()]),
        gate_id: Some(addr("0xa")),
        limit: 20,
        before_id: None,
    };
    let p = s.page(Stream::AccessGate, &q).unwrap();
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.rows[0].transaction_digest, "t2");
}

// ── bindPackages ────────────────────────────────────────────────────────────

#[test]
fn records_the_packages_of_a_fresh_index_without_clearing_anything() {
    let mut s = mem();
    let g = s.generation().to_owned();
    assert_eq!(s.bind_packages(&bound(), &[], None).unwrap(), vec![]);
    assert_eq!(s.generation(), g);
}

#[test]
fn keeps_an_index_whose_packages_are_unchanged() {
    let mut s = mem();
    s.bind_packages(&bound(), &[], None).unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c1")).unwrap();
    let g = s.generation().to_owned();
    assert_eq!(s.bind_packages(&bound(), &[], None).unwrap(), vec![]);
    assert_eq!(s.coverage(Stream::AccessGate).unwrap().events, 1);
    assert_eq!(s.cursor(Stream::AccessGate).unwrap().as_deref(), Some("c1"));
    assert_eq!(s.generation(), g);
}

#[test]
fn refuses_to_clear_history_without_confirmation_and_changes_nothing() {
    let mut s = mem();
    s.bind_packages(&bound(), &[], None).unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c1")).unwrap();
    let g = s.generation().to_owned();
    match s.bind_packages(&other(), &[], None) {
        Err(BindError::PackageChanged(e)) => assert_eq!(e.streams, vec![Stream::AccessGate]),
        r => panic!("expected PackageChanged, got {r:?}"),
    }
    let msg = s.bind_packages(&other(), &[Stream::SealedContent], None).unwrap_err().to_string();
    assert!(msg.contains("REINDEX_CONFIRM=access-gate"), "{msg}");
    assert_eq!(s.coverage(Stream::AccessGate).unwrap().events, 1);
    assert_eq!(s.cursor(Stream::AccessGate).unwrap().as_deref(), Some("c1"));
    assert_eq!(s.generation(), g);
    // The recorded binding is untouched, so the same refusal repeats until the operator decides.
    assert!(matches!(s.bind_packages(&other(), &[], None), Err(BindError::PackageChanged(_))));
}

#[test]
fn clears_only_the_confirmed_stream_and_rotates_the_generation() {
    let mut s = mem();
    s.bind_packages(&bound(), &[], None).unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 5, "t1")], Some("c1")).unwrap();
    s.ingest(Stream::SealedContent, &[ev("SealedContentPublished", "0xa", 2, None, "t2")], Some("s1")).unwrap();
    let g = s.generation().to_owned();
    assert_eq!(s.bind_packages(&other(), &[Stream::AccessGate], None).unwrap(), vec![Stream::AccessGate]);
    assert_eq!(s.coverage(Stream::AccessGate).unwrap().events, 0);
    assert_eq!(s.cursor(Stream::AccessGate).unwrap(), None);
    assert_eq!(s.gate_stats(&addr("0xa")).unwrap(), GateStats { counts: vec![], commission_mist: 0 });
    assert_eq!(s.coverage(Stream::SealedContent).unwrap().events, 1);
    assert_eq!(s.cursor(Stream::SealedContent).unwrap().as_deref(), Some("s1"));
    assert_ne!(s.generation(), g);
    assert_eq!(s.meta("generation").unwrap().as_deref(), Some(s.generation()));
    // The new binding sticks.
    assert_eq!(s.bind_packages(&other(), &[], None).unwrap(), vec![]);
}

#[test]
fn treats_rows_indexed_before_packages_were_recorded_like_a_package_change() {
    let mut s = mem();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c1")).unwrap();
    assert!(matches!(s.bind_packages(&bound(), &[], None), Err(BindError::PackageChanged(_))));
    assert_eq!(s.bind_packages(&bound(), &[Stream::AccessGate], None).unwrap(), vec![Stream::AccessGate]);
    assert_eq!(s.coverage(Stream::AccessGate).unwrap().events, 0);
}

#[test]
fn writes_a_restorable_backup_before_clearing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    let mut s = Store::open(&path).unwrap();
    s.bind_packages(&bound(), &[], None).unwrap();
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c1")).unwrap();
    let backup = dir.path().join("indexer.db.bak'quoted");
    s.bind_packages(&other(), &[Stream::AccessGate], Some(backup.to_str().unwrap())).unwrap();
    let b = Store::open(&backup).unwrap();
    assert_eq!(b.coverage(Stream::AccessGate).unwrap().events, 1);
    assert_eq!(b.cursor(Stream::AccessGate).unwrap().as_deref(), Some("c1"));
}

// ── Counters ────────────────────────────────────────────────────────────────

#[test]
fn counters_stay_exact_across_duplicates_and_agree_with_a_recount() {
    let s = mem();
    let rows = vec![
        minted("0xa", 7, 100, "t1"),
        minted("0xa", 3, 200, "t2"),
        ev("AccessConsumed", "0xa", 5, None, "t3"),
        minted("0xb", 9, 50, "t4"),
    ];
    s.ingest(Stream::AccessGate, &rows, Some("c")).unwrap();
    s.ingest(Stream::AccessGate, &rows, Some("c")).unwrap(); // duplicates change nothing
    assert_eq!(
        s.coverage(Stream::AccessGate).unwrap(),
        Coverage { from_checkpoint: Some("3".into()), latest_checkpoint: Some("9".into()), events: 4 }
    );
    let a = s.gate_stats(&addr("0xa")).unwrap();
    assert_eq!((a.count("AccessMinted"), a.count("AccessConsumed"), a.count("AccessBurned")), (2, 1, 0));
    assert_eq!(a.commission_mist, 300);
    assert_eq!(s.gate_stats(&addr("0xb")).unwrap().commission_mist, 50);
    let (n, lo, hi): (i64, i64, i64) = s
        .connection()
        .query_row(
            "SELECT COUNT(*), MIN(checkpoint), MAX(checkpoint) FROM events WHERE stream = 'access-gate'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((n, lo, hi), (4, 3, 9));
}

#[test]
fn commission_totals_are_exact_beyond_64_bits() {
    let s = mem();
    let big = 1i64 << 60;
    let rows: Vec<_> = (0..20).map(|i| minted("0xa", i, big, &format!("t{i}"))).collect();
    s.ingest(Stream::AccessGate, &rows, Some("c")).unwrap();
    assert_eq!(s.gate_stats(&addr("0xa")).unwrap().commission_mist, 20u128 << 60);
}

#[test]
fn counters_are_rebuilt_from_existing_rows_for_a_database_that_predates_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    {
        let first = Store::open(&path).unwrap();
        first
            .ingest(Stream::AccessGate, &[minted("0xa", 1, 100, "t1"), minted("0xa", 2, 250, "t2")], Some("c"))
            .unwrap();
        first
            .connection()
            .execute_batch("DELETE FROM stream_stats; DELETE FROM gate_stats; DELETE FROM gate_commission; DELETE FROM meta WHERE key = 'stats:v2'")
            .unwrap();
    }
    let again = Store::open(&path).unwrap();
    assert_eq!(
        again.coverage(Stream::AccessGate).unwrap(),
        Coverage { from_checkpoint: Some("1".into()), latest_checkpoint: Some("2".into()), events: 2 }
    );
    let g = again.gate_stats(&addr("0xa")).unwrap();
    assert_eq!((g.count("AccessMinted"), g.commission_mist), (2, 350));
}

#[test]
fn coverage_over_50k_rows_is_a_constant_time_read() {
    let s = mem();
    {
        let c = s.connection();
        c.execute_batch("BEGIN").unwrap();
        let mut ins = c
            .prepare("INSERT INTO events (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind) VALUES ('access-gate', ?, 0, ?, 's', 't', x'00', 'AccessMinted')")
            .unwrap();
        for i in 0..50_000 {
            ins.execute(params![format!("d{i}"), i]).unwrap();
        }
        c.execute_batch("COMMIT").unwrap();
    }
    let t = std::time::Instant::now();
    for _ in 0..200 {
        s.coverage(Stream::AccessGate).unwrap();
    }
    assert!(t.elapsed().as_millis() < 200, "200 reads took {:?}: a scan would take seconds", t.elapsed());
}

#[test]
fn undecodable_and_gap_counters() {
    let s = mem();
    s.note_undecodable(Stream::AccessGate, 2).unwrap();
    s.note_undecodable(Stream::AccessGate, 1).unwrap();
    s.ingest(Stream::AccessGate, &[], Some("stale")).unwrap();
    s.reset_cursor_after_gap(Stream::AccessGate, &"x".repeat(300), "2026-10-19T00:00:00.000Z").unwrap();
    assert_eq!(s.health(Stream::AccessGate).unwrap(), StreamHealth { undecodable: 3, gaps: 1 });
    assert_eq!(s.cursor(Stream::AccessGate).unwrap(), None);
    let gap: serde_json::Value = serde_json::from_str(&s.meta("gap:access-gate").unwrap().unwrap()).unwrap();
    assert_eq!(gap["reason"].as_str().unwrap().len(), 200);
    assert_eq!(gap["at"], "2026-10-19T00:00:00.000Z");
    assert_eq!(s.health(Stream::SealedContent).unwrap(), StreamHealth { undecodable: 0, gaps: 0 });
}

#[test]
fn heartbeats_round_trip() {
    let s = mem();
    assert_eq!(s.poll_state(Stream::AccessGate).unwrap(), PollState::default());
    let st = PollState { last_success_at: 5, last_error: Some("boom".into()), last_attempt_at: 9 };
    s.record_poll(Stream::AccessGate, &st).unwrap();
    assert_eq!(s.poll_state(Stream::AccessGate).unwrap(), st);
    assert_eq!(
        s.meta("poll:access-gate").unwrap().unwrap(),
        r#"{"lastSuccessAt":5,"lastError":"boom","lastAttemptAt":9}"#
    );
    s.touch_alive(42).unwrap();
    assert_eq!(s.meta(ALIVE_KEY).unwrap().as_deref(), Some("42"));
}

// ── Cutover and the read-only view ──────────────────────────────────────────

#[test]
fn backs_up_an_earlier_writers_database_once_before_marking_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    // A database the TypeScript indexer wrote: same schema, no `schema_writer`.
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.execute_batch("INSERT INTO meta VALUES ('generation', 'abcdefabcdef'), ('stats:v2', '1'); INSERT INTO cursors VALUES ('access-gate', 'AQID');").unwrap();
    }
    let backup = dir.path().join("indexer.db.bak-pre-rust-1");
    let s = Store::open(&path).unwrap();
    assert_eq!(s.generation(), "abcdefabcdef");
    assert!(s.mark_rust_writer(backup.to_str().unwrap()).unwrap());
    assert!(backup.exists());
    assert_eq!(Store::open(&backup).unwrap().cursor(Stream::AccessGate).unwrap().as_deref(), Some("AQID"));
    assert_eq!(s.meta(SCHEMA_WRITER_KEY).unwrap().as_deref(), Some("rust"));
    // Marked: never again.
    let second = dir.path().join("indexer.db.bak-pre-rust-2");
    assert!(!s.mark_rust_writer(second.to_str().unwrap()).unwrap());
    assert!(!second.exists());
}

#[test]
fn a_new_database_is_marked_without_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path().join("indexer.db")).unwrap();
    let backup = dir.path().join("b");
    assert!(!s.mark_rust_writer(backup.to_str().unwrap()).unwrap());
    assert!(!backup.exists());
    assert_eq!(s.meta(SCHEMA_WRITER_KEY).unwrap().as_deref(), Some("rust"));
}

#[test]
fn the_reader_is_not_ready_until_the_writer_creates_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    assert!(matches!(Reader::open(&path), Err(OpenError::NotReady(_))));
    // An empty file (another process mid-creation) is not ready either.
    std::fs::write(&path, b"").unwrap();
    assert!(matches!(Reader::open(&path), Err(OpenError::NotReady(_))));
    std::fs::remove_file(&path).unwrap();
    let s = Store::open(&path).unwrap();
    let r = Reader::open(&path).unwrap();
    assert_eq!(r.generation().unwrap().as_deref(), Some(s.generation()));
}

#[test]
fn the_reader_cannot_write_and_sees_new_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    let s = Store::open(&path).unwrap();
    let r = Reader::open(&path).unwrap();
    let err = r.try_write().unwrap_err().to_string();
    assert!(err.contains("readonly") || err.contains("read-only") || err.contains("query_only"), "{err}");
    assert_eq!(r.coverage(Stream::AccessGate).unwrap().events, 0);
    s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c")).unwrap();
    assert_eq!(r.coverage(Stream::AccessGate).unwrap().events, 1);
    s.record_poll(Stream::AccessGate, &PollState { last_success_at: 7, last_error: None, last_attempt_at: 7 }).unwrap();
    assert_eq!(r.poll_state(Stream::AccessGate).unwrap().last_success_at, 7);
}

#[test]
fn the_wal_files_outlive_the_writer_so_a_read_only_mount_can_open_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    {
        let s = Store::open(&path).unwrap();
        s.ingest(Stream::AccessGate, &[minted("0xa", 1, 1, "t")], Some("c")).unwrap();
    }
    assert!(dir.path().join("indexer.db-wal").exists());
    assert!(dir.path().join("indexer.db-shm").exists());
    let r = Reader::open(&path).unwrap();
    assert_eq!(r.coverage(Stream::AccessGate).unwrap().events, 1);
}
