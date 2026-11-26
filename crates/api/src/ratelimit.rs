//! Per-key token buckets in memory. They back the Cloudflare edge for requests that reach the origin
//! (cache misses). Idle buckets are swept and the map is capped (least recently used evicted first),
//! so memory stays bounded whatever the key churn.

use std::collections::{BTreeMap, HashMap};

/// One client's token bucket: the tokens left, when it was last refilled and where it sits in the recency order.
struct Bucket {
    /// Tokens left, fractional because refills are continuous.
    tokens: f64,
    /// When the bucket was last refilled (epoch milliseconds).
    at: i64,
    /// Position in the recency order.
    seq: u64,
}

/// The `RateLimiter` of the TypeScript `src/ratelimit.ts`, with the same arithmetic.
pub struct RateLimiter {
    /// Tokens added per second.
    per_sec: f64,
    /// The most tokens a bucket holds, which is also the largest burst.
    burst: f64,
    /// The most buckets kept at once; the least recently used are evicted past it.
    max_buckets: usize,
    /// The buckets by client key.
    buckets: HashMap<String, Bucket>,
    /// Recency order: seq → key (oldest first).
    order: BTreeMap<u64, String>,
    /// The next recency sequence number to hand out.
    next_seq: u64,
    /// When idle buckets were last swept (epoch milliseconds).
    last_sweep: i64,
}

impl RateLimiter {
    /// A limiter with the default cap of 50,000 buckets.
    pub fn new(per_sec: u32, burst: u32) -> RateLimiter {
        RateLimiter::with_capacity(per_sec, burst, 50_000)
    }

    /// At most `max_buckets` buckets are kept.
    pub fn with_capacity(per_sec: u32, burst: u32, max_buckets: usize) -> RateLimiter {
        RateLimiter {
            per_sec: f64::from(per_sec),
            burst: f64::from(burst),
            max_buckets,
            buckets: HashMap::new(),
            order: BTreeMap::new(),
            next_seq: 0,
            last_sweep: 0,
        }
    }

    /// Take one token for `key` at time `t` (epoch ms); false when the bucket is empty.
    pub fn take(&mut self, key: &str, t: i64) -> bool {
        // Housekeeping first, at most once a minute, so the map cannot fill with idle clients.
        self.sweep(t);
        // Each call takes the next recency number; the bucket it touches moves to the newest end of the order.
        let seq = self.next_seq;
        self.next_seq += 1;
        let (per_sec, burst) = (self.per_sec, self.burst);
        let ok = match self.buckets.get_mut(key) {
            // A known client: refill for the time since its last call (never above the burst), then try to take one token.
            Some(b) => {
                b.tokens = burst.min(b.tokens + ((t - b.at) as f64 / 1000.0) * per_sec);
                b.at = t;
                let ok = b.tokens >= 1.0;
                if ok {
                    b.tokens -= 1.0;
                }
                self.order.remove(&b.seq);
                b.seq = seq;
                ok
            }
            // A new client starts with a full bucket, so its first request is never limited.
            None => {
                let mut b = Bucket { tokens: burst, at: t, seq };
                let ok = b.tokens >= 1.0;
                if ok {
                    b.tokens -= 1.0;
                }
                self.buckets.insert(key.to_owned(), b);
                ok
            }
        };
        // Record the key at its new recency position.
        self.order.insert(seq, key.to_owned());
        // Over the cap: evict from the oldest end until the map fits, so memory stays bounded however many clients appear.
        while self.buckets.len() > self.max_buckets {
            let Some((_, oldest)) = self.order.pop_first() else { break };
            self.buckets.remove(&oldest);
        }
        ok
    }

    /// Seconds until `key` has a token again (for `Retry-After`).
    pub fn retry_after(&self, key: &str) -> u64 {
        match self.buckets.get(key) {
            // Time until a whole token exists again, rounded up so a client that waits that long is not refused twice.
            Some(b) if b.tokens < 1.0 => ((1.0 - b.tokens) / self.per_sec).ceil() as u64,
            _ => 0,
        }
    }

    /// How many client buckets are held.
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    /// Whether no bucket is held.
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    /// Drop buckets idle for longer than a full refill, at most once a minute: a full bucket carries no information, so removing it changes nothing a client can see.
    fn sweep(&mut self, t: i64) {
        if t - self.last_sweep < 60_000 {
            return;
        }
        self.last_sweep = t;
        // How long an untouched bucket takes to refill completely, in milliseconds.
        let full = (self.burst / self.per_sec) * 1000.0;
        let stale: Vec<(String, u64)> =
            self.buckets.iter().filter(|(_, b)| (t - b.at) as f64 > full).map(|(k, b)| (k.clone(), b.seq)).collect();
        for (k, seq) in stale {
            self.buckets.remove(&k);
            self.order.remove(&seq);
        }
    }
}

/// The rate-limit bucket of a client address. IPv6 clients are keyed by their /64 (one subscriber
/// controls the whole prefix, so per-address buckets would be unlimited); IPv4 and anything else are
/// used as is.
pub fn client_key(ip: &str) -> String {
    // Accept the spellings a proxy or socket may give: bracketed, with a zone id (`%eth0`), or plain.
    let t = ip.trim();
    let t = t.strip_prefix('[').unwrap_or(t);
    let t = t.strip_suffix(']').unwrap_or(t);
    let v = t.split('%').next().unwrap_or("");
    // No colon means IPv4; a dot means IPv4-mapped IPv6 or not an address: all keyed as given.
    if !v.contains(':') || v.contains('.') {
        return ip.to_owned(); // IPv4, IPv4-mapped IPv6, or not an address
    }
    let mut halves = v.split("::");
    let head = halves.next().unwrap_or("");
    let tail = halves.next().unwrap_or("");
    let h: Vec<&str> = if head.is_empty() { vec![] } else { head.split(':').collect() };
    // A `::` stands for as many zero groups as it takes to make eight.
    let compressed = v.contains("::");
    let t: Vec<&str> = if tail.is_empty() || !compressed { vec![] } else { tail.split(':').collect() };
    let groups: Vec<&str> = if compressed {
        let fill = 8usize.saturating_sub(h.len() + t.len());
        h.iter().copied().chain(std::iter::repeat_n("0", fill)).chain(t.iter().copied()).collect()
    } else {
        h
    };
    // The /64 is the first four groups.
    let prefix: Vec<String> = groups
        .iter()
        .take(4)
        .map(|g| {
            let g = g.to_lowercase();
            // `replace(/^0+(?=.)/, '')`: strip leading zeros but keep the last character.
            let trimmed = g.trim_start_matches('0');
            if trimmed.is_empty() && !g.is_empty() {
                g[g.len() - 1..].to_owned()
            } else if g.is_empty() {
                g
            } else {
                trimmed.to_owned()
            }
        })
        .collect();
    // The key is the prefix in a canonical spelling, so `2001:db8::1` and `2001:0db8:0:0::2` share one bucket.
    format!("{}::/64", prefix.join(":"))
}

/// Unit tests for the buckets and the client key.
#[cfg(test)]
mod tests {
    use super::*;

    /// Refills at the configured rate and sweeps idle buckets.
    #[test]
    fn refills_at_the_configured_rate_and_sweeps_idle_buckets() {
        let mut l = RateLimiter::new(2, 2);
        assert_eq!([l.take("a", 0), l.take("a", 0), l.take("a", 0)], [true, true, false]);
        assert_eq!(l.retry_after("a"), 1);
        assert!(l.take("a", 500));
        l.take("b", 120_000);
        assert_eq!(l.len(), 1);
        assert_eq!(l.retry_after("unknown"), 0);
    }

    /// Keys ipv6 clients by their 64 and caps the bucket map.
    #[test]
    fn keys_ipv6_clients_by_their_64_and_caps_the_bucket_map() {
        assert_eq!(client_key("2001:db8:1:2:aaaa:bbbb:cccc:dddd"), client_key("2001:db8:1:2::1"));
        assert_ne!(client_key("2001:db8:1:2::1"), client_key("2001:db8:1:3::1"));
        assert_eq!(client_key("2001:db8:1:2::1"), "2001:db8:1:2::/64");
        assert_eq!(client_key("[2001:DB8:0001::1]"), "2001:db8:1:0::/64");
        assert_eq!(client_key("fe80::1%eth0"), "fe80:0:0:0::/64");
        assert_eq!(client_key("::1"), "0:0:0:0::/64");
        assert_eq!(client_key("203.0.113.9"), "203.0.113.9");
        assert_eq!(client_key("::ffff:203.0.113.9"), "::ffff:203.0.113.9");
        assert_eq!(client_key("unknown"), "unknown");
        let mut l = RateLimiter::with_capacity(1, 1, 100);
        for i in 0..1000 {
            l.take(&format!("k{i}"), 0);
        }
        assert!(l.len() <= 100);
    }

    /// Evicts the least recently used bucket.
    #[test]
    fn evicts_the_least_recently_used_bucket() {
        let mut l = RateLimiter::with_capacity(1, 1, 2);
        assert!(l.take("a", 0));
        assert!(l.take("b", 0));
        assert!(!l.take("a", 0)); // a is now the most recent
        l.take("c", 0); // evicts b
        assert!(!l.take("a", 0)); // a kept its (empty) bucket
        assert!(l.take("b", 0)); // b starts full again
    }
}
