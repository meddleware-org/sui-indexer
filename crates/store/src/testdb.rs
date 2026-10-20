//! The test harness: an isolated database per test on a real PostgreSQL, provisioned the way the
//! platform's `provision.sh` provisions a service (an owner role with DDL and write, a SELECT-only role,
//! `REVOKE ... FROM PUBLIC`, per-role timeouts), so a test runs with the same two privilege levels as
//! production.
//!
//! - `TEST_DATABASE_URL` is a superuser URL (`postgres://postgres:pw@127.0.0.1:55432/postgres?sslmode=verify-full&sslrootcert=ca.crt`).
//!   Unset, a test prints `SKIPPED` and returns; **CI sets `REQUIRE_TEST_DATABASE=1`**, which turns a
//!   missing or unreachable server into a failure, so no test can be skipped silently there.
//! - Each [`TestDb`] creates `si_t_<random>` with roles `si_t_<random>_own` and `_api` and drops all three
//!   when it goes out of scope (also when the test panics).

use std::time::Duration;

use tokio_postgres::Client;

use crate::db::DatabaseSettings;
use crate::reader::Reader;
use crate::writer::{ConnectOptions, Store};

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("the OS random source is available");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Whether a missing test database is a failure (CI) rather than a skip.
pub fn required() -> bool {
    std::env::var("REQUIRE_TEST_DATABASE").is_ok_and(|v| v == "1")
}

/// The admin settings from `TEST_DATABASE_URL`, or `None` (after saying so) when unset and not required.
pub fn admin_settings() -> Option<DatabaseSettings> {
    match std::env::var("TEST_DATABASE_URL") {
        Ok(url) if !url.is_empty() => {
            Some(DatabaseSettings::parse(&url, true).expect("TEST_DATABASE_URL is a valid PostgreSQL URL"))
        }
        _ => {
            assert!(!required(), "REQUIRE_TEST_DATABASE=1 but TEST_DATABASE_URL is not set: tests would be skipped");
            eprintln!(
                "SKIPPED: TEST_DATABASE_URL is not set (these tests need a PostgreSQL; see README → Development)"
            );
            None
        }
    }
}

/// An isolated, provisioned database.
pub struct TestDb {
    admin: DatabaseSettings,
    pub name: String,
    pub owner_role: String,
    pub api_role: String,
    /// The owner role (what the ingester uses).
    pub owner: DatabaseSettings,
    /// The SELECT-only role (what the API uses).
    pub api: DatabaseSettings,
}

/// Run `f` with an admin client on database `db`.
async fn admin_client(admin: &DatabaseSettings, db: &str) -> Client {
    let s = admin.with_credentials(admin.user(), &admin_password(admin), db);
    s.connect().await.expect("connect as the test admin")
}

fn admin_password(admin: &DatabaseSettings) -> String {
    // The settings keep the password inside the URL only; read it back from the parsed config.
    String::from_utf8_lossy(admin.pg_config().get_password().unwrap_or_default()).into_owned()
}

impl TestDb {
    /// Create a fresh database and its two roles on the server `admin` (a superuser).
    pub async fn create_on_admin(admin: DatabaseSettings) -> TestDb {
        Self::create_on(admin).await
    }

    /// Keep the database after this value goes away (the contract job's seeder): returns its name and the
    /// API role's settings. Remove it later with [`drop_database`].
    pub fn persist(self) -> (String, DatabaseSettings) {
        let this = std::mem::ManuallyDrop::new(self);
        (this.name.clone(), this.api.clone())
    }

    /// Create a fresh database and its two roles; `None` when no test database is configured.
    pub async fn create() -> Option<TestDb> {
        let admin = admin_settings()?;
        Some(Self::create_on(admin).await)
    }

    async fn create_on(admin: DatabaseSettings) -> TestDb {
        let tag = random_hex(6);
        let name = format!("si_t_{tag}");
        let (own, api) = (format!("{name}_own"), format!("{name}_api"));
        let (own_pw, api_pw) = (random_hex(16), random_hex(16));
        let c = admin_client(&admin, "postgres").await;
        c.batch_execute(&format!(
            "CREATE ROLE {own} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS CONNECTION LIMIT 8 PASSWORD '{own_pw}';
             ALTER ROLE {own} SET statement_timeout = '60s';
             ALTER ROLE {own} SET idle_in_transaction_session_timeout = '60s';
             CREATE ROLE {api} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS CONNECTION LIMIT 20 PASSWORD '{api_pw}';
             ALTER ROLE {api} SET statement_timeout = '5s';
             ALTER ROLE {api} SET idle_in_transaction_session_timeout = '5s';
             ALTER ROLE {api} SET default_transaction_read_only = on;"
        ))
        .await
        .expect("create the test roles");
        // CREATE DATABASE cannot share a multi-statement (implicit transaction) batch.
        c.batch_execute(&format!("CREATE DATABASE {name} OWNER {own}")).await.expect("create the test database");
        c.batch_execute(&format!(
            "REVOKE ALL ON DATABASE {name} FROM PUBLIC;
             GRANT CONNECT, TEMPORARY, CREATE ON DATABASE {name} TO {own};
             GRANT CONNECT ON DATABASE {name} TO {api};"
        ))
        .await
        .expect("grant the test database privileges");
        let d = admin_client(&admin, &name).await;
        d.batch_execute(&format!(
            "REVOKE ALL ON SCHEMA public FROM PUBLIC;
             GRANT USAGE, CREATE ON SCHEMA public TO {own};
             GRANT USAGE ON SCHEMA public TO {api};
             ALTER DEFAULT PRIVILEGES FOR ROLE {own} IN SCHEMA public GRANT SELECT ON TABLES TO {api};
             ALTER DEFAULT PRIVILEGES FOR ROLE {own} IN SCHEMA public GRANT SELECT ON SEQUENCES TO {api};"
        ))
        .await
        .expect("grant the schema privileges");
        let owner = admin.with_credentials(&own, &own_pw, &name);
        let api_s = admin.with_credentials(&api, &api_pw, &name);
        TestDb { admin, name, owner_role: own, api_role: api, owner, api: api_s }
    }

    /// Another provisioned database on the same server (cross-database isolation tests).
    pub async fn another(&self) -> TestDb {
        Self::create_on(self.admin.clone()).await
    }

    /// An environment (`DATABASE_URL` of `which`, plus what a plaintext test server needs) for the
    /// config loaders, with `extra` variables on top.
    pub fn env(&self, which: &DatabaseSettings, extra: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let mut m: std::collections::HashMap<String, String> =
            extra.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        m.insert("DATABASE_URL".into(), which.url_unredacted().to_owned());
        if !which.tls_enabled() {
            m.insert("ALLOW_INSECURE_DATABASE".into(), "1".into());
        }
        move |k| m.get(k).cloned()
    }

    /// The writer, connected as the owner (migrations applied, writer lock held).
    pub async fn store(&self) -> Store {
        Store::connect(&self.owner).await.expect("connect the writer")
    }

    /// The writer with a short lock wait (for tests that expect a second writer to be refused).
    pub async fn store_nowait(&self) -> crate::Result<Store> {
        Store::connect_with(&self.owner, ConnectOptions { lock_wait: Duration::ZERO, ..Default::default() }).await
    }

    /// The API's reader (SELECT-only role, a pool of `size`).
    pub fn reader(&self, size: usize) -> Reader {
        Reader::new(&self.api, size).expect("build the reader")
    }

    /// A superuser connection to this database (to inspect or damage it on purpose).
    pub async fn admin(&self) -> Client {
        admin_client(&self.admin, &self.name).await
    }
}

/// Drop a database made by [`TestDb`] (by name) and its two roles.
pub async fn drop_database(admin: &DatabaseSettings, name: &str) {
    assert!(
        name.starts_with("si_t_") && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "not a test database: {name}"
    );
    if let Ok(c) = admin.with_credentials(admin.user(), &admin_password(admin), "postgres").connect().await {
        let _ = c.batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).await;
        let _ = c.batch_execute(&format!("DROP ROLE IF EXISTS {name}_own")).await;
        let _ = c.batch_execute(&format!("DROP ROLE IF EXISTS {name}_api")).await;
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let (admin, name) = (self.admin.clone(), self.name.clone());
        // A separate thread and runtime: this can run while the test's own runtime is shutting down.
        let cleanup = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("cleanup runtime");
            rt.block_on(drop_database(&admin, &name));
        });
        let _ = cleanup.join();
    }
}

/// `let db = require_db!();` — the test's database, or return (skipped) when none is configured.
#[macro_export]
macro_rules! require_db {
    () => {
        match $crate::testdb::TestDb::create().await {
            Some(db) => db,
            None => return,
        }
    };
}
