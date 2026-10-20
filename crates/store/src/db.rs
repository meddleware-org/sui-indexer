//! Connection settings and TLS for the PostgreSQL tier.
//!
//! `DATABASE_URL` is a libpq-style URL: `postgres://user:password@host:5432/database?sslmode=verify-full&sslrootcert=/etc/postgres-ca/ca.crt`.
//!
//! - **TLS is verified or off.** `sslmode=verify-full` (the chain against `sslrootcert`, and the host name
//!   against the certificate) is the only mode accepted, unless `ALLOW_INSECURE_DATABASE=1` also admits
//!   `sslmode=disable` for local tests. `prefer`, `require` and `verify-ca` are refused: they would
//!   encrypt without authenticating the server.
//! - **No secret is ever logged.** [`DatabaseSettings`] has no `Display`, its `Debug` omits the password,
//!   and every error here is a fixed message that does not echo the URL.
//! - One crypto provider: rustls with `ring`, the same the gRPC client uses.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use tokio_postgres::config::Host;
use tokio_postgres::{Client, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::config::ConfigError;
use crate::error::{Result, StoreError};

const URL_HELP: &str = "postgres://user:password@host:5432/database?sslmode=verify-full&sslrootcert=/path/ca.crt";

/// How long a new connection may take to be established (TCP, TLS and authentication).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything needed to open connections: parsed and validated once, at startup.
#[derive(Clone)]
pub struct DatabaseSettings {
    pg: tokio_postgres::Config,
    /// The CA certificate (PEM) the server certificate is verified against; `None` only for
    /// `sslmode=disable`.
    ca_path: Option<String>,
    insecure: bool,
    /// The URL as given. A secret: only used to compare settings and to derive others.
    url: String,
}

impl PartialEq for DatabaseSettings {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url && self.insecure == other.insecure
    }
}

impl Eq for DatabaseSettings {}

impl fmt::Debug for DatabaseSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseSettings")
            .field("host", &self.host())
            .field("port", &self.port())
            .field("database", &self.dbname())
            .field("user", &self.user())
            .field("tls", &if self.ca_path.is_some() { "verify-full" } else { "disable" })
            .finish_non_exhaustive()
    }
}

fn bad(what: &str) -> ConfigError {
    ConfigError(format!("DATABASE_URL {what} (expected {URL_HELP})"))
}

impl DatabaseSettings {
    /// Parse and validate `url`. `allow_insecure` is `ALLOW_INSECURE_DATABASE=1`: it admits
    /// `sslmode=disable` and an empty password, for local tests only.
    pub fn parse(url: &str, allow_insecure: bool) -> std::result::Result<DatabaseSettings, ConfigError> {
        let rest = url
            .strip_prefix("postgres://")
            .or_else(|| url.strip_prefix("postgresql://"))
            .ok_or_else(|| bad("must start with postgres://"))?;
        // Split the query string; take sslmode and sslrootcert out of it (tokio-postgres knows neither
        // `verify-full` nor `sslrootcert`) and hand the rest to its own parser.
        let (base, query) = match rest.split_once('?') {
            Some((b, q)) => (b, q),
            None => (rest, ""),
        };
        let (mut sslmode, mut sslrootcert) = (None, None);
        let mut kept: Vec<&str> = Vec::new();
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k {
                "sslmode" => sslmode = Some(percent_decode_str(v).decode_utf8_lossy().into_owned()),
                "sslrootcert" => sslrootcert = Some(percent_decode_str(v).decode_utf8_lossy().into_owned()),
                // Settings that would weaken or redirect the connection are not accepted from the URL.
                "sslcert" | "sslkey" | "options" | "host" | "hostaddr" | "channel_binding" | "target_session_attrs" => {
                    return Err(bad(&format!("must not set {k}")));
                }
                _ => kept.push(pair),
            }
        }
        let mut cleaned = format!("postgres://{base}");
        if !kept.is_empty() {
            cleaned.push('?');
            cleaned.push_str(&kept.join("&"));
        }
        let mut pg = tokio_postgres::Config::from_str(&cleaned).map_err(|_| bad("is not a valid connection URL"))?;

        let hosts = pg.get_hosts();
        if hosts.len() != 1 || !matches!(hosts[0], Host::Tcp(ref h) if !h.is_empty()) {
            return Err(bad("must name exactly one TCP host"));
        }
        if pg.get_ports().len() > 1 {
            return Err(bad("must name exactly one port"));
        }
        if pg.get_user().is_none_or(str::is_empty) {
            return Err(bad("must name a user"));
        }
        if pg.get_dbname().is_none_or(str::is_empty) {
            return Err(bad("must name a database"));
        }
        if pg.get_password().is_none_or(<[u8]>::is_empty) && !allow_insecure {
            return Err(bad("must carry a password"));
        }
        let ca_path = match sslmode.as_deref() {
            Some("verify-full") => {
                let ca =
                    sslrootcert.filter(|c| !c.is_empty()).ok_or_else(|| bad("needs sslrootcert with verify-full"))?;
                Some(ca)
            }
            Some("disable") if allow_insecure => None,
            None if allow_insecure => None,
            Some("disable") | None => {
                return Err(bad("must set sslmode=verify-full (sslmode=disable needs ALLOW_INSECURE_DATABASE=1)"));
            }
            Some(_) => {
                return Err(bad("must set sslmode=verify-full (the other modes do not authenticate the server)"));
            }
        };
        pg.ssl_mode(if ca_path.is_some() {
            tokio_postgres::config::SslMode::Require
        } else {
            tokio_postgres::config::SslMode::Disable
        });
        pg.connect_timeout(CONNECT_TIMEOUT);
        pg.keepalives(true).keepalives_idle(Duration::from_secs(30));
        Ok(DatabaseSettings { pg, ca_path, insecure: allow_insecure, url: url.to_owned() })
    }

    /// The same server and TLS settings with other credentials and database (tests and tooling).
    pub fn with_credentials(&self, user: &str, password: &str, dbname: &str) -> DatabaseSettings {
        let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
        let mut url = format!("postgres://{}:{}@", enc(user), enc(password));
        let host = self.host();
        if host.contains(':') {
            url.push_str(&format!("[{host}]"));
        } else {
            url.push_str(host);
        }
        url.push_str(&format!(":{}/{}", self.port(), enc(dbname)));
        const KEEP: &AsciiSet = NON_ALPHANUMERIC;
        match &self.ca_path {
            Some(ca) => url.push_str(&format!("?sslmode=verify-full&sslrootcert={}", utf8_percent_encode(ca, KEEP))),
            None => url.push_str("?sslmode=disable"),
        }
        DatabaseSettings::parse(&url, self.insecure).expect("derived settings are valid")
    }

    /// The same settings, labelled in `pg_stat_activity`.
    pub fn with_application_name(mut self, name: &str) -> DatabaseSettings {
        self.pg.application_name(name);
        self
    }

    pub fn host(&self) -> &str {
        match &self.pg.get_hosts()[0] {
            Host::Tcp(h) => h,
            #[cfg(unix)]
            Host::Unix(_) => "",
        }
    }

    pub fn port(&self) -> u16 {
        self.pg.get_ports().first().copied().unwrap_or(5432)
    }

    pub fn dbname(&self) -> &str {
        self.pg.get_dbname().unwrap_or_default()
    }

    pub fn user(&self) -> &str {
        self.pg.get_user().unwrap_or_default()
    }

    /// Whether the connection is encrypted and the server verified.
    pub fn tls_enabled(&self) -> bool {
        self.ca_path.is_some()
    }

    /// The path of the CA certificate, when TLS is on.
    pub fn ca_path(&self) -> Option<&str> {
        self.ca_path.as_deref()
    }

    /// The URL as given, **including the password**: for handing the settings to a child process or a
    /// test's environment, never for logging.
    pub fn url_unredacted(&self) -> &str {
        &self.url
    }

    /// Whether this was parsed with `ALLOW_INSECURE_DATABASE=1`.
    pub fn is_insecure_allowed(&self) -> bool {
        self.insecure
    }

    /// The pg configuration (the pool builds on it).
    pub(crate) fn pg_config(&self) -> &tokio_postgres::Config {
        &self.pg
    }

    /// The rustls connector for this server, or `None` when TLS is off. Reads the CA file.
    pub(crate) fn tls(&self) -> Result<Option<MakeRustlsConnect>> {
        let Some(path) = &self.ca_path else { return Ok(None) };
        let mut roots = rustls::RootCertStore::empty();
        let certs = CertificateDer::pem_file_iter(path)
            .map_err(|e| StoreError::Config(format!("cannot read the database CA certificate {path}: {e}")))?;
        for cert in certs {
            let cert = cert.map_err(|e| StoreError::Config(format!("invalid certificate in {path}: {e}")))?;
            roots.add(cert).map_err(|e| StoreError::Config(format!("unusable CA certificate in {path}: {e}")))?;
        }
        if roots.is_empty() {
            return Err(StoreError::Config(format!("{path} holds no CA certificate")));
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| StoreError::Config(format!("TLS setup failed: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Some(MakeRustlsConnect::new(config)))
    }

    /// Open one connection (its I/O runs on a spawned task; the client is closed when the task ends).
    pub async fn connect(&self) -> Result<Client> {
        match self.tls()? {
            Some(tls) => {
                let (client, connection) = self.pg.connect(tls).await?;
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                Ok(client)
            }
            None => {
                let (client, connection) = self.pg.connect(NoTls).await?;
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                Ok(client)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "postgres://sui_indexer_api:s3cr3t-Passw0rd@postgres.database.svc.cluster.local:5432/sui_indexer?sslmode=verify-full&sslrootcert=/etc/postgres-ca/ca.crt";

    fn err(url: &str, insecure: bool) -> String {
        DatabaseSettings::parse(url, insecure).unwrap_err().0
    }

    #[test]
    fn parses_the_tier_url() {
        let s = DatabaseSettings::parse(GOOD, false).unwrap();
        assert_eq!(
            (s.host(), s.port(), s.dbname(), s.user(), s.ca_path()),
            (
                "postgres.database.svc.cluster.local",
                5432,
                "sui_indexer",
                "sui_indexer_api",
                Some("/etc/postgres-ca/ca.crt")
            )
        );
        assert!(s.tls_enabled());
        // The port defaults, and the URL scheme postgresql:// is the same.
        let s2 =
            DatabaseSettings::parse(&GOOD.replace("postgres://", "postgresql://").replace(":5432", ""), false).unwrap();
        assert_eq!(s2.port(), 5432);
    }

    #[test]
    fn requires_verify_full_and_a_root_certificate() {
        for url in [
            "postgres://u:pw@h/db",
            "postgres://u:pw@h/db?sslmode=require",
            "postgres://u:pw@h/db?sslmode=prefer",
            "postgres://u:pw@h/db?sslmode=verify-ca&sslrootcert=/ca.crt",
            "postgres://u:pw@h/db?sslmode=disable",
            "postgres://u:pw@h/db?sslmode=allow",
        ] {
            assert!(err(url, false).contains("sslmode=verify-full"), "{url}");
        }
        assert!(err("postgres://u:pw@h/db?sslmode=verify-full", false).contains("sslrootcert"));
        assert!(err("postgres://u:pw@h/db?sslmode=verify-full&sslrootcert=", false).contains("sslrootcert"));
    }

    #[test]
    fn insecure_mode_admits_only_a_plaintext_local_setup() {
        let s = DatabaseSettings::parse("postgres://u:pw@127.0.0.1:5433/db?sslmode=disable", true).unwrap();
        assert!(!s.tls_enabled());
        assert!(DatabaseSettings::parse("postgres://u@127.0.0.1/db", true).is_ok());
        // verify-full stays verified even in insecure mode, and weaker TLS modes stay refused.
        assert!(DatabaseSettings::parse("postgres://u:pw@h/db?sslmode=require", true).is_err());
        assert!(
            DatabaseSettings::parse("postgres://u:pw@h/db?sslmode=verify-full&sslrootcert=/x", true)
                .unwrap()
                .tls_enabled()
        );
    }

    #[test]
    fn refuses_an_empty_password_unless_insecure() {
        let url = "postgres://u@h/db?sslmode=verify-full&sslrootcert=/ca.crt";
        assert!(err(url, false).contains("password"));
        assert!(err("postgres://u:@h/db?sslmode=verify-full&sslrootcert=/ca.crt", false).contains("password"));
        assert!(DatabaseSettings::parse(url, true).is_ok());
    }

    #[test]
    fn refuses_what_is_not_a_single_tcp_server_with_a_user_and_a_database() {
        for (url, needle) in [
            ("mysql://u:pw@h/db", "postgres://"),
            ("postgres://u:pw@h", "database"),
            ("postgres://:pw@h/db", "user"),
            ("postgres://u:pw@h1,h2/db", "one TCP host"),
            ("postgres://u:pw@/db", "TCP host"),
            ("postgres://u:pw@h/db?sslcert=/c", "must not set sslcert"),
            ("postgres://u:pw@h/db?options=-c%20x", "must not set options"),
            ("postgres://u:pw@h/db?host=/var/run", "must not set host"),
            ("postgres://u:pw@h:notaport/db", "valid connection URL"),
        ] {
            assert!(err(url, true).contains(needle), "{url}: {}", err(url, true));
        }
    }

    #[test]
    fn nothing_prints_the_password() {
        let pw = "s3cr3t-Passw0rd";
        let s = DatabaseSettings::parse(GOOD, false).unwrap();
        assert!(!format!("{s:?}").contains(pw));
        for url in [
            format!("postgres://u:{pw}@h/db?sslmode=require"),
            format!("postgres://u:{pw}@h:x/db"),
            format!("postgres://u:{pw}@h/db?sslcert=/c"),
            format!("mysql://u:{pw}@h/db"),
            format!("postgres://u:{pw}@/db"),
        ] {
            let e = err(&url, false);
            assert!(!e.contains(pw) && !e.contains("u:"), "{e}");
        }
    }

    #[test]
    fn derived_settings_keep_the_server_and_tls_and_encode_credentials() {
        let s = DatabaseSettings::parse(GOOD, false).unwrap();
        let t = s.with_credentials("role x", "p@ss:w/rd", "other_db");
        assert_eq!((t.host(), t.port(), t.dbname(), t.user()), (s.host(), 5432, "other_db", "role x"));
        assert_eq!(t.ca_path(), s.ca_path());
        assert_eq!(t.pg_config().get_password(), Some(&b"p@ss:w/rd"[..]));
    }
}
