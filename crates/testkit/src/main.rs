//! `seed-fixture`: provision and seed an isolated database for the CI client-contract job (the real client
//! packages against the `api` binary), then remove it.
//!
//! ```text
//! seed-fixture seed <ADMIN_URL> [poll-age-ms]   create a database with the two production roles, seed the
//!                                               fixtures and fresh heartbeats; print one JSON line
//! seed-fixture drop <ADMIN_URL> <database>      drop the database and its roles
//! seed-fixture sqlite <path>                    (with --features import-sqlite) write the 0.0.13-format
//!                                               SQLite fixture the importer's tests and the golden
//!                                               answers use
//! ```
//!
//! `<ADMIN_URL>` is a superuser URL (`TEST_DATABASE_URL`). The JSON line carries the API role's
//! `DATABASE_URL` (a secret of a throwaway database), whether `ALLOW_INSECURE_DATABASE=1` is needed, and
//! the database name for `drop`.

use sui_indexer_store::testdb::{TestDb, drop_database};
use sui_indexer_store::time::now_ms;
use sui_indexer_store::{DatabaseSettings, PollState, Stream};
use sui_indexer_testkit::seed;

fn usage() -> ! {
    eprintln!("usage: seed-fixture seed <ADMIN_URL> [poll-age-ms] | seed-fixture drop <ADMIN_URL> <database>");
    std::process::exit(2);
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(cmd), Some(admin)) = (args.next(), args.next()) else { usage() };
    #[cfg(feature = "import-sqlite")]
    if cmd == "sqlite" {
        let built = sui_indexer_testkit::sqlite::build(std::path::Path::new(&admin));
        println!("wrote {admin}: {built:?}");
        return;
    }
    let admin = DatabaseSettings::parse(&admin, true).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });
    match cmd.as_str() {
        "seed" => {
            let age: i64 = args.next().map_or(0, |a| a.parse().expect("poll-age-ms is an integer"));
            let db = TestDb::create_on_admin(admin).await;
            let mut store = db.store().await;
            store
                .bind_packages(
                    &[
                        (Stream::AccessGate, sui_indexer_testkit::ag()),
                        (Stream::SealedContent, sui_indexer_testkit::seal()),
                    ],
                    &[],
                    None,
                )
                .await
                .expect("bind the fixture packages");
            seed(&mut store).await;
            let at = now_ms() - age;
            for s in Stream::ALL {
                store
                    .record_poll(s, &PollState { last_success_at: at, last_error: None, last_attempt_at: at })
                    .await
                    .expect("heartbeat");
            }
            drop(store);
            let (name, api) = db.persist();
            let out = serde_json::json!({
                "database": name,
                "databaseUrl": api.url_unredacted(),
                "allowInsecure": !api.tls_enabled(),
            });
            println!("{out}");
        }
        "drop" => {
            let Some(name) = args.next() else { usage() };
            drop_database(&admin, &name).await;
        }
        _ => usage(),
    }
}
