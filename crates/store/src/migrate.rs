//! Versioned schema migrations, applied by the ingester (the database owner) at startup.
//!
//! - **Embedded and ordered.** [`MIGRATIONS`] holds the SQL files of `migrations/`; a migration is never
//!   edited after release (its SHA-256 is recorded in `schema_migrations` and re-checked at every start),
//!   a change is a new file.
//! - **Race-free.** Each migration runs in its own transaction that first takes
//!   `pg_advisory_xact_lock(MIGRATION_LOCK_KEY)`, so two starts never apply the same one (the writer
//!   lock already makes a second ingester impossible; this also covers the importer and tooling).
//! - **Expand, then contract.** The API runs the previous image's queries while a new ingester migrates, so
//!   a migration must keep the previous version's queries working (add, don't rename or drop in one step).
//!   The API serves once `schema_migrations` reaches [`SCHEMA_VERSION`] or higher.
//! - The API's role cannot run any of this (no DDL); it answers 503 until the schema is there.

use sha2::{Digest, Sha256};
use tokio_postgres::Client;

use crate::error::{Result, StoreError};

/// One embedded migration.
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Every migration, in order. The versions are 1, 2, 3, ... without gaps.
pub const MIGRATIONS: &[Migration] =
    &[Migration { version: 1, name: "init", sql: include_str!("../migrations/0001_init.sql") }];

/// The schema version this build needs (the last migration).
pub const SCHEMA_VERSION: i32 = 1;

/// The advisory-lock key serialising migrations (per database).
pub const MIGRATION_LOCK_KEY: i64 = 0x5355_4958_4d49_4701;

fn checksum(sql: &str) -> String {
    Sha256::digest(sql.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Apply every migration not yet applied; verify the applied ones. Returns the versions applied now.
pub async fn run(client: &mut Client) -> Result<Vec<i32>> {
    {
        // Under the same lock: two first starts must not both try to create the bookkeeping table.
        let tx = client.transaction().await?;
        tx.execute("SELECT pg_advisory_xact_lock($1)", &[&MIGRATION_LOCK_KEY]).await?;
        tx.batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
               version    integer     PRIMARY KEY,
               name       text        NOT NULL,
               checksum   text        NOT NULL,
               applied_at timestamptz NOT NULL DEFAULT now()
             )",
        )
        .await?;
        tx.commit().await?;
    }
    let mut applied = Vec::new();
    for m in MIGRATIONS {
        let tx = client.transaction().await?;
        tx.execute("SELECT pg_advisory_xact_lock($1)", &[&MIGRATION_LOCK_KEY]).await?;
        let sum = checksum(m.sql);
        let recorded = tx
            .query_opt("SELECT checksum FROM schema_migrations WHERE version = $1", &[&m.version])
            .await?
            .map(|r| r.get::<_, String>(0));
        match recorded {
            Some(c) if c == sum => {}
            Some(_) => {
                return Err(StoreError::Migration(format!(
                    "migration {} ({}) was applied from different SQL than this build embeds: refusing to continue \
                     (a released migration must never be edited; add a new one)",
                    m.version, m.name
                )));
            }
            None => {
                tx.batch_execute(m.sql).await.map_err(|e| {
                    StoreError::Migration(format!("migration {} ({}) failed: {}", m.version, m.name, StoreError::Pg(e)))
                })?;
                tx.execute(
                    "INSERT INTO schema_migrations (version, name, checksum) VALUES ($1, $2, $3)",
                    &[&m.version, &m.name, &sum],
                )
                .await?;
                applied.push(m.version);
            }
        }
        tx.commit().await?;
    }
    // A database migrated by a NEWER build is not touched: an older ingester must not run against a
    // schema it does not know.
    let newest: Option<i32> = client.query_one("SELECT max(version) FROM schema_migrations", &[]).await?.get(0);
    if newest.is_some_and(|v| v > SCHEMA_VERSION) {
        return Err(StoreError::Migration(format!(
            "the database schema is at version {}, newer than this build ({SCHEMA_VERSION}): refusing to run an older ingester",
            newest.unwrap_or_default()
        )));
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_numbered_without_gaps_and_the_last_is_the_schema_version() {
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(m.version, i as i32 + 1);
            assert!(!m.sql.trim().is_empty() && !m.name.is_empty());
        }
        assert_eq!(MIGRATIONS.last().map(|m| m.version), Some(SCHEMA_VERSION));
    }

    #[test]
    fn checksums_are_stable_sha256() {
        assert_eq!(checksum(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
