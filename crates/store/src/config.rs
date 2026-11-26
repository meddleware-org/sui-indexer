//! Configuration from the environment: the variables, defaults and validation of the TypeScript
//! `src/config.ts` (0.0.11). Each process reads the variables it uses:
//!
//! - the ingester: `NETWORK`, `GRPC_URL`, `DATABASE_URL`, `POLL_MS`, `ACCESS_GATE_ORIGINAL_ID`,
//!   `SEAL_ORIGINAL_ID`, `REINDEX_CONFIRM`;
//! - the API: `NETWORK`, `DATABASE_URL`, `DB_POOL_SIZE`, `PORT`, `METRICS_PORT`, `POLL_MS`, `RATE_LIMIT_PER_SEC`,
//!   `RATE_LIMIT_BURST`;
//! - both: `ALLOW_INSECURE_DATABASE=1` (local tests: admits `sslmode=disable` and an empty password).
//!
//! Errors carry the messages of `config.ts`; none of them contains the database URL.

use crate::Stream;
use crate::db::DatabaseSettings;
use crate::sui::{is_hex_id, normalize_sui_address};

/// One network's recorded package deployments (the original ids events are typed at).
#[derive(Clone, Copy, Debug)]
pub struct Deployment {
    /// The network name (`testnet` or `mainnet`).
    pub network: &'static str,
    /// The original id of the `access_gate` package on that network.
    pub access_gate_original_id: &'static str,
    /// The original id of the sealed-storage package on that network.
    pub seal_original_id: &'static str,
}

/// The recorded deployments of the published client packages (`@meddleware/access-gate-client`
/// 0.0.9 `ACCESS_GATE_DEPLOYMENTS`, `@meddleware/sealed-storage-client` 0.0.20 `SEAL_POLICIES_DEPLOYMENTS`).
/// Keep in step with them: a new deployment there is a new default here.
pub const DEPLOYMENTS: &[Deployment] = &[Deployment {
    network: "testnet",
    access_gate_original_id: "0xd7ddaa94b74330979b2b618fc81206d160a264f1c9ca148a77fa2144301388c9",
    seal_original_id: "0x0c8f73490b14836e6a7a724fb46b242cb061d04a5f193fd637159997f8a1773d",
}];

/// The public Mysten full node of each network (`GRPC_URL` default).
pub const DEFAULT_GRPC: &[(&str, &str)] =
    &[("testnet", "https://fullnode.testnet.sui.io:443"), ("mainnet", "https://fullnode.mainnet.sui.io:443")];

/// The ingester's configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestConfig {
    /// The one network this instance indexes.
    pub network: String,
    /// The full node's gRPC URL: `https`, or any scheme on `localhost` or `127.0.0.1` (validated; the error never repeats the value).
    pub grpc_url: String,
    /// The owner role's connection (DDL and writes). The ingester uses exactly one connection.
    pub database: DatabaseSettings,
    /// The poll interval in milliseconds (1,000 to 3,600,000).
    pub poll_ms: u64,
    /// The `access_gate` original package id, normalised to the long `0x` form.
    pub access_gate_original_id: String,
    /// The sealed-storage original package id, normalised.
    pub seal_original_id: String,
    /// Streams whose stored history may be deleted and re-indexed after a package change.
    pub reindex_confirm: Vec<Stream>,
}

/// The API's configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiConfig {
    /// The one network this instance serves (the first path segment of the API).
    pub network: String,
    /// The SELECT-only role's connection.
    pub database: DatabaseSettings,
    /// Connections in the API's pool.
    pub pool_size: usize,
    /// The public API port.
    pub port: u16,
    /// Where `/metrics` is served, apart from the public API (never in the Service or the Ingress); 0 = off.
    pub metrics_port: u16,
    /// The ingester's poll interval: `/healthz` calls a stream stale after five of them.
    pub poll_ms: u64,
    /// Requests per second per client IP (token refill rate).
    pub rate_limit_per_sec: u32,
    /// Token-bucket capacity per client IP.
    pub rate_limit_burst: u32,
}

/// A configuration error (the message names the variable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    /// Write the message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// A result whose error is a [`ConfigError`].
type Result<T> = std::result::Result<T, ConfigError>;

/// A variable, treating empty as unset (`env.X || default` in the TypeScript).
fn var(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name).filter(|v| !v.is_empty())
}

/// `NETWORK`, defaulting to `testnet`.
fn network(env: &dyn Fn(&str) -> Option<String>) -> String {
    var(env, "NETWORK").unwrap_or_else(|| "testnet".into())
}

/// `DATABASE_URL` (required) validated by [`DatabaseSettings::parse`]; `ALLOW_INSECURE_DATABASE=1` admits
/// a plaintext, passwordless local database. The URL is never echoed in an error.
pub fn database(env: &dyn Fn(&str) -> Option<String>) -> Result<DatabaseSettings> {
    let url = var(env, "DATABASE_URL").ok_or_else(|| {
        ConfigError(
            "DATABASE_URL is required (postgres://user:password@host:5432/database?sslmode=verify-full&sslrootcert=/path/ca.crt)"
                .into(),
        )
    })?;
    let insecure = match var(env, "ALLOW_INSECURE_DATABASE").as_deref() {
        None | Some("0") => false,
        Some("1") => true,
        Some(_) => return Err(ConfigError("ALLOW_INSECURE_DATABASE must be 1 (or unset)".into())),
    };
    DatabaseSettings::parse(&url, insecure)
}

/// JavaScript `Number(raw)` for the strings an operator plausibly writes: surrounding whitespace,
/// an optional sign, decimals with fraction or exponent, and `0x`/`0o`/`0b` integers. `None` is NaN.
fn js_number(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return Some(0.0);
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(d) = s.strip_prefix(prefix) {
            return u64::from_str_radix(d, radix).ok().map(|v| v as f64);
        }
    }
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    if body == "Infinity" {
        return Some(if s.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    // Rust also accepts `inf`, `nan` and `infinity` in any case; JavaScript does not.
    if !body.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return None;
    }
    s.parse::<f64>().ok()
}

/// A bounded integer variable: the fallback when unset, otherwise a value in `[min, max]` read as JavaScript would read it, else an error naming the variable.
fn int(env: &dyn Fn(&str) -> Option<String>, name: &str, fallback: u64, min: u64, max: u64) -> Result<u64> {
    let Some(raw) = var(env, name) else { return Ok(fallback) };
    match js_number(&raw) {
        Some(v) if v.fract() == 0.0 && v >= min as f64 && v <= max as f64 => Ok(v as u64),
        _ => Err(ConfigError(format!("{name} must be an integer in [{min}, {max}], got {raw}"))),
    }
}

/// The recorded deployment of a network, if there is one.
fn deployment(network: &str) -> Option<&'static Deployment> {
    DEPLOYMENTS.iter().find(|d| d.network == network)
}

/// A package original-id override: `0x` + hex, normalised so the stored binding compares exactly.
fn package_id(
    env: &dyn Fn(&str) -> Option<String>,
    name: &str,
    fallback: impl FnOnce() -> Result<String>,
) -> Result<String> {
    let Some(raw) = var(env, name) else { return fallback() };
    if !is_hex_id(&raw) {
        return Err(ConfigError(format!("{name} must be a 0x-prefixed hex package id, got {raw}")));
    }
    Ok(normalize_sui_address(&raw))
}

/// `REINDEX_CONFIRM=access-gate,sealed-content`: the streams the operator agrees to delete and re-index.
fn reindex_confirm(env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Stream>> {
    let Some(raw) = var(env, "REINDEX_CONFIRM") else { return Ok(vec![]) };
    raw.split(',')
        .map(|s| {
            let s = s.trim();
            Stream::parse(s).ok_or_else(|| {
                ConfigError(format!("REINDEX_CONFIRM must list access-gate and/or sealed-content, got {s}"))
            })
        })
        .collect()
}

/// `GRPC_URL` (or the network's public full node): `https`, or any scheme on `localhost`/`127.0.0.1`.
fn grpc_url(env: &dyn Fn(&str) -> Option<String>, network: &str) -> Result<String> {
    // The configured URL, else the network's public node; a network with neither cannot start.
    let url = var(env, "GRPC_URL")
        .or_else(|| DEFAULT_GRPC.iter().find(|(n, _)| *n == network).map(|(_, u)| (*u).to_owned()))
        .ok_or_else(|| ConfigError(format!("GRPC_URL is required for network {network}")))?;
    // A URL with no `://` is not a URL; the error does not repeat it.
    let (scheme, rest) = url.split_once("://").ok_or_else(|| ConfigError("Invalid URL".into()))?;
    // The authority ends at the first path, query or fragment mark.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Anything before an `@` is credentials; it is dropped here and never shown.
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host_port.starts_with('[') {
        host_port.split_once(']').map_or(host_port, |(h, _)| h).trim_start_matches('[')
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if scheme.is_empty() || host.is_empty() {
        return Err(ConfigError("Invalid URL".into()));
    }
    // Host names compare case-insensitively.
    let host = host.to_ascii_lowercase();
    // Plain http is allowed only to a local test node.
    if !scheme.eq_ignore_ascii_case("https") && host != "localhost" && host != "127.0.0.1" {
        return Err(ConfigError("GRPC_URL must use https".into()));
    }
    Ok(url)
}

/// Name a configured URL in a message without repeating anything that can carry a credential: only an
/// `http` or `https` scheme, the host and the port are returned. A URL can hold a user name, a password,
/// a token in its path or a key in its query, and a message reaches the log (CONF lens), so a value that is
/// not a URL, has no host or has another scheme is described by a fixed phrase instead of being echoed.
pub fn describe_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return "(not a valid URL)".into() };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return "(an address that is not http or https)".into();
    }
    // The authority ends at the first `/`, `?` or `#`; anything before an `@` is a user name and password.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host_port.is_empty() {
        return "(an address with no host)".into();
    }
    format!("{}://{host_port}", scheme.to_ascii_lowercase())
}

impl IngestConfig {
    /// The ingester's configuration from the process environment.
    pub fn from_env() -> Result<IngestConfig> {
        Self::load(&|k| std::env::var(k).ok())
    }

    /// The ingester's configuration from `env`. Package ids default to the recorded deployments of
    /// the published client packages, so a normal deploy sets only `NETWORK`.
    pub fn load(env: &dyn Fn(&str) -> Option<String>) -> Result<IngestConfig> {
        // The network decides the default node and the recorded package ids.
        let network = network(env);
        let grpc_url = grpc_url(env, &network)?;
        let database = database(env)?;
        let poll_ms = int(env, "POLL_MS", 15_000, 1_000, 3_600_000)?;
        let access_gate_original_id = package_id(env, "ACCESS_GATE_ORIGINAL_ID", || {
            deployment(&network)
                .map(|d| d.access_gate_original_id.to_owned())
                .ok_or_else(|| ConfigError(format!("no access_gate deployment recorded for {network}")))
        })?;
        let seal_original_id = package_id(env, "SEAL_ORIGINAL_ID", || {
            deployment(&network)
                .map(|d| d.seal_original_id.to_owned())
                .ok_or_else(|| ConfigError(format!("no sealed_storage deployment recorded for {network}")))
        })?;
        // The operator's explicit permission to delete and re-index a stream after a package change.
        let reindex_confirm = reindex_confirm(env)?;
        Ok(IngestConfig {
            network,
            grpc_url,
            database,
            poll_ms,
            access_gate_original_id,
            seal_original_id,
            reindex_confirm,
        })
    }
}

impl ApiConfig {
    /// The API's configuration from the process environment.
    pub fn from_env() -> Result<ApiConfig> {
        Self::load(&|k| std::env::var(k).ok())
    }

    /// The API's configuration from `env`.
    pub fn load(env: &dyn Fn(&str) -> Option<String>) -> Result<ApiConfig> {
        Ok(ApiConfig {
            network: network(env),
            database: database(env)?,
            pool_size: int(env, "DB_POOL_SIZE", 8, 1, 64)? as usize,
            port: int(env, "PORT", 8080, 1, 65_535)? as u16,
            metrics_port: int(env, "METRICS_PORT", 9184, 0, 65_535)? as u16,
            poll_ms: int(env, "POLL_MS", 15_000, 1_000, 3_600_000)?,
            rate_limit_per_sec: int(env, "RATE_LIMIT_PER_SEC", 10, 1, 10_000)? as u32,
            rate_limit_burst: int(env, "RATE_LIMIT_BURST", 40, 1, 100_000)? as u32,
        })
    }
}

/// Unit tests for the configuration (pure functions over a fake environment).
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A well-formed (throwaway) database URL for the tests.
    const URL: &str = "postgres://sui_indexer_api:a-long-throwaway-password@db.example:5432/sui_indexer?sslmode=verify-full&sslrootcert=/etc/postgres-ca/ca.crt";

    /// The environment of `pairs` plus a valid `DATABASE_URL` (unless the test sets its own).
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let mut m: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        m.entry("DATABASE_URL".into()).or_insert_with(|| URL.into());
        move |k| m.get(k).filter(|v| v.as_str() != "<unset>").cloned()
    }

    /// Load the ingester configuration from key-value pairs.
    fn ingest(pairs: &[(&str, &str)]) -> Result<IngestConfig> {
        IngestConfig::load(&env(pairs))
    }

    /// Load the API configuration from key-value pairs.
    fn api(pairs: &[(&str, &str)]) -> Result<ApiConfig> {
        ApiConfig::load(&env(pairs))
    }

    /// The error message of a result that must be an error.
    fn err<T: std::fmt::Debug>(r: Result<T>) -> String {
        r.unwrap_err().0
    }

    /// Defaults to testnet with the recorded package ids.
    #[test]
    fn defaults_to_testnet_with_the_recorded_package_ids() {
        let c = ingest(&[]).unwrap();
        assert_eq!(c.network, "testnet");
        assert_eq!(c.poll_ms, 15_000);
        assert_eq!(
            (c.database.host(), c.database.dbname(), c.database.user()),
            ("db.example", "sui_indexer", "sui_indexer_api")
        );
        assert_eq!(c.grpc_url, "https://fullnode.testnet.sui.io:443");
        assert_eq!(c.access_gate_original_id, DEPLOYMENTS[0].access_gate_original_id);
        assert!(is_hex_id(&c.seal_original_id) && c.seal_original_id.len() == 66);
        let a = api(&[]).unwrap();
        assert_eq!((a.network.as_str(), a.port, a.poll_ms, a.pool_size), ("testnet", 8080, 15_000, 8));
        assert_eq!((a.rate_limit_per_sec, a.rate_limit_burst), (10, 40));
        // Empty means unset, as `env.X || default` did.
        assert_eq!(api(&[("PORT", ""), ("NETWORK", "")]).unwrap().port, 8080);
    }

    /// The database url is required and validated without echoing it.
    #[test]
    fn the_database_url_is_required_and_validated_without_echoing_it() {
        let pw = "a-long-throwaway-password";
        assert!(err(ingest(&[("DATABASE_URL", "<unset>")])).contains("DATABASE_URL is required"));
        assert!(err(api(&[("DATABASE_URL", "")])).contains("DATABASE_URL is required"));
        for bad in [
            format!("postgres://u:{pw}@h/db"),
            format!("postgres://u:{pw}@h/db?sslmode=require&sslrootcert=/c"),
            format!("mysql://u:{pw}@h/db"),
        ] {
            let e = err(api(&[("DATABASE_URL", &bad)]));
            assert!(e.contains("DATABASE_URL") && !e.contains(pw), "{e}");
        }
        // The insecure switch is explicit and only for 1.
        let plain = "postgres://u:pw@127.0.0.1:5432/db?sslmode=disable";
        assert!(api(&[("DATABASE_URL", plain)]).is_err());
        assert!(api(&[("DATABASE_URL", plain), ("ALLOW_INSECURE_DATABASE", "1")]).is_ok());
        assert!(
            err(api(&[("DATABASE_URL", plain), ("ALLOW_INSECURE_DATABASE", "yes")]))
                .contains("ALLOW_INSECURE_DATABASE")
        );
        assert!(!format!("{:?}", api(&[]).unwrap()).contains(pw));
    }

    /// The pool size is bounded.
    #[test]
    fn the_pool_size_is_bounded() {
        assert_eq!(api(&[("DB_POOL_SIZE", "3")]).unwrap().pool_size, 3);
        for bad in ["0", "65", "x"] {
            assert!(err(api(&[("DB_POOL_SIZE", bad)])).contains("DB_POOL_SIZE"), "{bad}");
        }
    }

    /// A URL is named by its scheme, host and port only, never its credentials, path or query.
    #[test]
    fn a_url_is_named_by_its_origin_only() {
        let secret = "https://user:s3cret@node.example:443/tok3n/path?key=k3y#frag";
        assert_eq!(describe_url(secret), "https://node.example:443");
        for part in ["user", "s3cret", "tok3n", "k3y", "frag"] {
            assert!(!describe_url(secret).contains(part), "{part}");
        }
        assert_eq!(describe_url("HTTP://Node.Example/x"), "http://Node.Example");
        assert_eq!(describe_url("s3cret://host.example"), "(an address that is not http or https)");
        assert_eq!(describe_url("user:s3cret@host.example"), "(not a valid URL)");
        assert_eq!(describe_url("https://user:s3cret@"), "(an address with no host)");
    }

    /// Refuses bad values and unknown networks.
    #[test]
    fn refuses_bad_values_and_unknown_networks() {
        assert!(err(ingest(&[("POLL_MS", "10")])).contains("POLL_MS"));
        assert!(err(api(&[("POLL_MS", "10")])).contains("POLL_MS"));
        assert!(err(api(&[("PORT", "x")])).contains("PORT"));
        assert!(err(ingest(&[("GRPC_URL", "http://node.example")])).contains("https"));
        assert!(
            err(ingest(&[("NETWORK", "devnet"), ("GRPC_URL", "https://n.example")]))
                .contains("no access_gate deployment")
        );
        assert_eq!(err(api(&[("PORT", "0")])), "PORT must be an integer in [1, 65535], got 0");
        assert!(err(api(&[("RATE_LIMIT_BURST", "1.5")])).contains("RATE_LIMIT_BURST"));
        assert!(err(api(&[("RATE_LIMIT_PER_SEC", "nan")])).contains("RATE_LIMIT_PER_SEC"));
        assert!(err(ingest(&[("GRPC_URL", "not a url")])).contains("Invalid URL"));
    }

    /// Accepts what javascript number accepts.
    #[test]
    fn accepts_what_javascript_number_accepts() {
        assert_eq!(api(&[("PORT", " 8081 ")]).unwrap().port, 8081);
        assert_eq!(api(&[("POLL_MS", "1e4")]).unwrap().poll_ms, 10_000);
        assert_eq!(api(&[("PORT", "0x1F90")]).unwrap().port, 8080);
        assert_eq!(api(&[("PORT", "8080.0")]).unwrap().port, 8080);
        assert!(api(&[("PORT", "Infinity")]).is_err());
    }

    /// Plain http only for loopback.
    #[test]
    fn plain_http_only_for_loopback() {
        assert!(ingest(&[("GRPC_URL", "http://localhost:9000")]).is_ok());
        assert!(ingest(&[("GRPC_URL", "http://127.0.0.1:9")]).is_ok());
        assert!(ingest(&[("GRPC_URL", "http://LOCALHOST:9000")]).is_ok());
        assert!(ingest(&[("GRPC_URL", "http://[::1]:9000")]).is_err());
    }

    /// F20: `NETWORK` is data; names that are object-prototype members in JavaScript are not networks.
    #[test]
    fn treats_network_as_data() {
        for network in ["constructor", "toString", "__proto__", "hasOwnProperty"] {
            assert_eq!(err(ingest(&[("NETWORK", network)])), format!("GRPC_URL is required for network {network}"));
            assert!(
                err(ingest(&[("NETWORK", network), ("GRPC_URL", "https://n.example")]))
                    .contains("no access_gate deployment")
            );
        }
    }

    /// Validates and normalises package id overrides.
    #[test]
    fn validates_and_normalises_package_id_overrides() {
        let c = ingest(&[("ACCESS_GATE_ORIGINAL_ID", "0xABC"), ("SEAL_ORIGINAL_ID", &format!("0x{}", "D".repeat(64)))])
            .unwrap();
        assert_eq!(c.access_gate_original_id, format!("0x{}abc", "0".repeat(61)));
        assert_eq!(c.seal_original_id, format!("0x{}", "d".repeat(64)));
        assert!(err(ingest(&[("ACCESS_GATE_ORIGINAL_ID", "abc")])).contains("ACCESS_GATE_ORIGINAL_ID"));
        assert!(err(ingest(&[("SEAL_ORIGINAL_ID", "0x12;drop")])).contains("SEAL_ORIGINAL_ID"));
        // Overrides make an unrecorded network usable.
        let local = ingest(&[
            ("NETWORK", "localnet"),
            ("GRPC_URL", "http://127.0.0.1:9000"),
            ("ACCESS_GATE_ORIGINAL_ID", "0x1111"),
            ("SEAL_ORIGINAL_ID", "0x2222"),
        ])
        .unwrap();
        assert_eq!(local.network, "localnet");
    }

    /// Reindex confirm lists streams.
    #[test]
    fn reindex_confirm_lists_streams() {
        assert_eq!(
            ingest(&[("REINDEX_CONFIRM", "access-gate, sealed-content")]).unwrap().reindex_confirm,
            vec![Stream::AccessGate, Stream::SealedContent]
        );
        assert_eq!(
            err(ingest(&[("REINDEX_CONFIRM", "access-gate,nope")])),
            "REINDEX_CONFIRM must list access-gate and/or sealed-content, got nope"
        );
    }
}
