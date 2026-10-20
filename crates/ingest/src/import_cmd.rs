//! `ingest import-sqlite <path> [--replace-existing]`: the one-off import of a 0.0.13 SQLite index.
//! **Temporary** (the `import-sqlite` feature exists in the 0.0.14 image only).
//!
//! Runs as the database owner (`DATABASE_URL` of the ingest role), takes the writer lock like the ingester
//! does, applies the migrations, imports in one transaction and verifies before it commits. Exit code 0 only
//! when the import committed and verified; the report goes to stdout.

use std::path::Path;
use std::process::ExitCode;

use sui_indexer_store::config::database;
use sui_indexer_store::import::{ImportOptions, import_sqlite};
use sui_indexer_store::time::now_ms;
use sui_indexer_store::{ConnectOptions, Store};

use crate::stdout_logger;

/// The command with the process environment.
pub async fn run(args: &[String]) -> ExitCode {
    run_with(args, &|k| std::env::var(k).ok()).await
}

/// The command with an explicit environment (`DATABASE_URL`, `ALLOW_INSECURE_DATABASE`).
pub async fn run_with(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> ExitCode {
    let log = stdout_logger();
    let mut path = None;
    let mut replace = false;
    for a in args {
        match a.as_str() {
            "--replace-existing" => replace = true,
            s if s.starts_with("--") => {
                log(&format!("unknown option {s} (usage: ingest import-sqlite <path> [--replace-existing])"));
                return ExitCode::from(2);
            }
            s if path.is_none() => path = Some(s.to_owned()),
            _ => {
                log("usage: ingest import-sqlite <path> [--replace-existing]");
                return ExitCode::from(2);
            }
        }
    }
    let Some(path) = path else {
        log("usage: ingest import-sqlite <path> [--replace-existing]");
        return ExitCode::from(2);
    };
    let settings = match database(env) {
        Ok(s) => s,
        Err(e) => {
            log(&format!("refusing to start: {e}"));
            return ExitCode::FAILURE;
        }
    };
    let mut store = match Store::connect_with(
        &settings,
        ConnectOptions { application_name: "sui-indexer-import", ..Default::default() },
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            log(&format!("cannot start the importer: {e}"));
            return ExitCode::FAILURE;
        }
    };
    log(&format!(
        "importing {path} into database {} (generation of the empty target: {})",
        settings.dbname(),
        store.generation()
    ));
    let opts = ImportOptions { replace_existing: replace, backup_schema: format!("bak_{}", now_ms()) };
    match import_sqlite(&mut store, Path::new(&path), &opts).await {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            log(&format!("import failed, nothing was committed: {e}"));
            ExitCode::FAILURE
        }
    }
}
