//! The store's error type: one enum for the database, the configuration and the invariants the store
//! enforces itself. Messages never contain the connection URL or a password.

use std::fmt;

/// An error of the store.
#[derive(Debug)]
pub enum StoreError {
    /// The database or the connection to it failed.
    Pg(tokio_postgres::Error),
    /// The database settings are unusable (TLS material, unreachable host before connecting).
    Config(String),
    /// Another process holds the single-writer lock (`pg_try_advisory_lock`).
    WriterLockHeld,
    /// A migration could not be applied or does not match what was applied before.
    Migration(String),
    /// The database holds, or was given, data the store refuses (an out-of-range value, a mismatch).
    Data(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Pg(e) => match e.as_db_error() {
                Some(d) => write!(f, "{}: {} (SQLSTATE {})", d.severity(), d.message(), d.code().code()),
                None => {
                    write!(f, "{e}")?;
                    let mut source = std::error::Error::source(e);
                    while let Some(s) = source {
                        write!(f, ": {s}")?;
                        source = s.source();
                    }
                    Ok(())
                }
            },
            StoreError::Config(m) | StoreError::Migration(m) | StoreError::Data(m) => f.write_str(m),
            StoreError::WriterLockHeld => f.write_str(
                "another process holds the writer lock of this database (only one ingester may write: \
                 check for a second ingest pod or a running import)",
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<tokio_postgres::Error> for StoreError {
    fn from(e: tokio_postgres::Error) -> Self {
        StoreError::Pg(e)
    }
}

impl StoreError {
    /// The SQLSTATE of a database error, if this is one (`42P01` undefined table, ...).
    pub fn sqlstate(&self) -> Option<&str> {
        match self {
            StoreError::Pg(e) => e.code().map(|c| c.code()),
            _ => None,
        }
    }
}

/// `Result` with the store's error.
pub type Result<T> = std::result::Result<T, StoreError>;
