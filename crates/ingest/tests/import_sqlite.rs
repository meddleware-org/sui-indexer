//! The one-off SQLite importer (`ingest import-sqlite`), against a fixture built in the 0.0.13 format and a
//! real PostgreSQL provisioned like production. Run with `--features import-sqlite`.
#![cfg(feature = "import-sqlite")]

use std::collections::BTreeMap;
use std::path::PathBuf;

use sui_indexer_ingest::import_cmd::run_with;
use sui_indexer_store::import::{ImportOptions, import_sqlite, rusqlite};
use sui_indexer_store::testdb::TestDb;
use sui_indexer_store::{Store, StoreError, Stream, require_db};
use sui_indexer_testkit::sqlite::{GENERATION, build};

fn fixture() -> (tempfile::TempDir, PathBuf, sui_indexer_testkit::sqlite::Built) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indexer.db");
    let built = build(&path);
    (dir, path, built)
}

fn opts() -> ImportOptions {
    ImportOptions { replace_existing: false, backup_schema: "bak_1760000000001".into() }
}

#[tokio::test]
async fn imports_the_index_with_its_ids_generation_cursors_and_counters() {
    let db = require_db!();
    let (_dir, path, built) = fixture();
    let mut store = db.store().await;
    let fresh_generation = store.generation().to_owned();
    assert_ne!(fresh_generation, GENERATION);

    let report = import_sqlite(&mut store, &path, &opts()).await.unwrap();

    assert_eq!(report.generation, GENERATION);
    assert_eq!(store.generation(), GENERATION);
    assert_eq!(report.events, BTreeMap::from([("access-gate".to_owned(), 7), ("sealed-content".to_owned(), 3)]));
    assert_eq!((report.max_id, report.commission_total), (built.max_id, built.commission_total));
    assert_eq!(report.order_hash.len(), 64);
    // Ids are the SQLite ids, gaps included (cursors `<generation>.<id>` and ETags `<generation>-<head id>`
    // issued before the cutover must keep their meaning).
    let ids: Vec<i64> =
        store.query("SELECT id FROM events ORDER BY id", &[]).await.unwrap().iter().map(|r| r.get(0)).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7, 8, 9, 12]);
    assert_eq!(store.head_id(Stream::AccessGate).await.unwrap(), 12);
    // Meta: generation, bindings and the gap note come over; the writer mark is postgres; heartbeats do not.
    assert_eq!(store.meta("generation").await.unwrap().as_deref(), Some(GENERATION));
    assert_eq!(store.meta("package:access-gate").await.unwrap(), Some(sui_indexer_testkit::ag()));
    assert!(store.meta("gap:access-gate").await.unwrap().unwrap().contains("below earliest available"));
    assert_eq!(store.meta("schema_writer").await.unwrap().as_deref(), Some("postgres"));
    assert_eq!(store.meta("stats:v2").await.unwrap(), None);
    assert_eq!(store.heartbeat("ingest:alive").await.unwrap(), None);
    assert!(report.skipped_meta_keys.iter().any(|k| k == "poll:access-gate"));
    // Cursors are the same base64 strings.
    assert_eq!(store.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("Yw=="));
    assert_eq!(store.cursor(Stream::SealedContent).await.unwrap().as_deref(), Some("cw=="));
    // Counters came over exactly (and agree with a recount).
    let cov = store.coverage(Stream::AccessGate).await.unwrap();
    assert_eq!(
        (cov.events, cov.from_checkpoint.as_deref(), cov.latest_checkpoint.as_deref()),
        (7, Some("11"), Some("20"))
    );
    let h = store.health(Stream::AccessGate).await.unwrap();
    assert_eq!((h.undecodable, h.gaps), (2, 1));
    assert!(store.verify_counters().await.unwrap().is_empty());
    // The report prints what an operator compares with the pre-cutover numbers.
    let text = report.to_string();
    assert!(text.contains("generation        e32e37afbbe9") && text.contains("events access-gate"), "{text}");
}

#[tokio::test]
async fn the_ingester_continues_the_imported_index_with_the_next_id_and_serves_old_cursors() {
    let db = require_db!();
    let (_dir, path, _) = fixture();
    {
        let mut store = db.store().await;
        import_sqlite(&mut store, &path, &opts()).await.unwrap();
    }
    // The ingester starts (migrations are already applied, the generation is the imported one) ...
    let mut store = db.store().await;
    assert_eq!(store.generation(), GENERATION);
    let mut bound =
        vec![(Stream::AccessGate, sui_indexer_testkit::ag()), (Stream::SealedContent, sui_indexer_testkit::seal())];
    // ... the recorded package bindings match: no REINDEX_CONFIRM is needed ...
    assert_eq!(store.bind_packages(&bound, &[], None).await.unwrap(), vec![]);
    // ... and the next event gets an id above everything imported.
    let new = sui_indexer_testkit::minted(&sui_indexer_testkit::gate_a(), 30, 9);
    store.ingest(Stream::AccessGate, &[new.indexed()], Some("bmV3")).await.unwrap();
    let id: i64 = store.query("SELECT max(id) FROM events", &[]).await.unwrap()[0].get(0);
    assert!(id > 12, "the identity sequence continues after the imported ids, got {id}");
    // A page bounded by a pre-cutover cursor id returns exactly the rows the SQLite index returned.
    let page = store
        .page(Stream::AccessGate, &sui_indexer_store::PageQuery { before_id: Some(5), limit: 20, ..Default::default() })
        .await
        .unwrap();
    assert_eq!(page.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [4, 2, 1]);
    // A different package binding is still refused, as for any index.
    bound[0].1 = sui_indexer_testkit::addr("0x77");
    assert!(store.bind_packages(&bound, &[], None).await.is_err());
}

#[tokio::test]
async fn refuses_a_target_that_already_holds_data_and_changes_nothing() {
    let db = require_db!();
    let (_dir, path, _) = fixture();
    let mut store = db.store().await;
    store
        .ingest(
            Stream::AccessGate,
            &[sui_indexer_testkit::minted(&sui_indexer_testkit::gate_a(), 1, 1).indexed()],
            Some("x"),
        )
        .await
        .unwrap();
    let generation = store.generation().to_owned();
    let err = import_sqlite(&mut store, &path, &opts()).await.unwrap_err().to_string();
    assert!(err.contains("already holds index data") && err.contains("--replace-existing"), "{err}");
    assert_eq!(store.generation(), generation);
    assert_eq!(store.coverage(Stream::AccessGate).await.unwrap().events, 1);
    assert_eq!(store.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("x"));
}

#[tokio::test]
async fn replace_existing_copies_the_target_to_a_backup_schema_first() {
    let db = require_db!();
    let (_dir, path, _) = fixture();
    let mut store = db.store().await;
    store
        .ingest(
            Stream::AccessGate,
            &[sui_indexer_testkit::minted(&sui_indexer_testkit::gate_a(), 1, 1).indexed()],
            Some("x"),
        )
        .await
        .unwrap();
    let report = import_sqlite(&mut store, &path, &ImportOptions { replace_existing: true, ..opts() }).await.unwrap();
    assert_eq!(report.backup.as_deref(), Some("bak_1760000000001"));
    assert_eq!(store.coverage(Stream::AccessGate).await.unwrap().events, 7);
    assert_eq!(store.query("SELECT count(*) FROM bak_1760000000001.events", &[]).await.unwrap()[0].get::<_, i64>(0), 1);
    assert_eq!(
        store.query("SELECT cursor FROM bak_1760000000001.cursors", &[]).await.unwrap()[0].get::<_, String>(0),
        "x"
    );
    assert!(store.verify_counters().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_verification_failure_rolls_everything_back() {
    let db = require_db!();
    let (_dir, path, _) = fixture();
    // Damage the SQLite counters: the copied counters would not match a recount of the copied rows.
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("UPDATE stream_stats SET events = events + 1 WHERE stream = 'access-gate'").unwrap();
    drop(c);
    let mut store = db.store().await;
    let generation = store.generation().to_owned();
    let err = import_sqlite(&mut store, &path, &opts()).await.unwrap_err().to_string();
    assert!(err.contains("did not verify and was rolled back") && err.contains("stream_stats"), "{err}");
    assert_eq!(store.generation(), generation);
    for t in ["events", "cursors", "stream_stats", "gate_stats", "gate_commission"] {
        let n: i64 = store.query(&format!("SELECT count(*) FROM {t}"), &[]).await.unwrap()[0].get(0);
        assert_eq!(n, 0, "{t} must be empty after the rollback");
    }
    assert_eq!(store.meta("generation").await.unwrap().as_deref(), Some(generation.as_str()));
    // A damaged commission total is caught the same way.
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("UPDATE stream_stats SET events = events - 1 WHERE stream = 'access-gate'; UPDATE gate_commission SET total = '1'").unwrap();
    drop(c);
    let err = import_sqlite(&mut store, &path, &opts()).await.unwrap_err().to_string();
    assert!(err.contains("gate_commission"), "{err}");
}

#[tokio::test]
async fn refuses_files_that_are_not_an_index_of_this_service() {
    let db = require_db!();
    let dir = tempfile::tempdir().unwrap();
    let mut store = db.store().await;
    let err = import_sqlite(&mut store, &dir.path().join("missing.db"), &opts()).await.unwrap_err().to_string();
    assert!(err.contains("is not a file"), "{err}");
    let junk = dir.path().join("junk.db");
    std::fs::write(&junk, b"not a database at all, just text").unwrap();
    assert!(import_sqlite(&mut store, &junk, &opts()).await.is_err());
    let other = dir.path().join("other.db");
    rusqlite::Connection::open(&other).unwrap().execute_batch("CREATE TABLE meta (key TEXT, value TEXT)").unwrap();
    assert!(import_sqlite(&mut store, &other, &opts()).await.is_err());
    // An index with an unexpected generation is refused.
    let (_d, path, _) = fixture();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("UPDATE meta SET value = 'XYZ' WHERE key = 'generation'")
        .unwrap();
    let err = import_sqlite(&mut store, &path, &opts()).await.unwrap_err().to_string();
    assert!(err.contains("unexpected generation"), "{err}");
}

#[tokio::test]
async fn reads_the_sqlite_file_without_changing_it() {
    let db = require_db!();
    let (_dir, path, _) = fixture();
    let before = std::fs::read(&path).unwrap();
    let mut store = db.store().await;
    import_sqlite(&mut store, &path, &opts()).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before, "the importer must open the file read-only");
}

#[tokio::test]
async fn cannot_run_while_an_ingester_holds_the_writer_lock() {
    let db = require_db!();
    let _running = db.store().await;
    let err = db.store_nowait().await.err().expect("refused");
    assert!(matches!(err, StoreError::WriterLockHeld), "{err}");
}

#[tokio::test]
async fn the_command_runs_end_to_end_from_the_environment() {
    let db: TestDb = require_db!();
    let (_dir, path, _) = fixture();
    let env = db.env(&db.owner, &[]);
    let code = run_with(&["import-sqlite-ignored".to_owned()], &env).await;
    // (an argument that is a path to nothing fails; the real call follows)
    assert_ne!(format!("{code:?}"), format!("{:?}", std::process::ExitCode::SUCCESS));
    let code = run_with(&[path.display().to_string()], &env).await;
    assert_eq!(format!("{code:?}"), format!("{:?}", std::process::ExitCode::SUCCESS));
    let store = Store::connect(&db.owner).await.unwrap();
    assert_eq!(store.generation(), GENERATION);
    // A second run refuses (the target now holds data) and fails.
    drop(store);
    let code = run_with(&[path.display().to_string()], &env).await;
    assert_ne!(format!("{code:?}"), format!("{:?}", std::process::ExitCode::SUCCESS));
    // Usage errors.
    for args in [vec![], vec!["--nope".to_owned(), path.display().to_string()], vec!["a".to_owned(), "b".to_owned()]] {
        assert_ne!(format!("{:?}", run_with(&args, &env).await), format!("{:?}", std::process::ExitCode::SUCCESS));
    }
}
