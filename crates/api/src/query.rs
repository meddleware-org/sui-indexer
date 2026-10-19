//! The URL handling of the TypeScript API (`new URL(req.url, base)` + `URLSearchParams`), ported
//! exactly where it decides an answer: path dot-segment resolution, `application/x-www-form-urlencoded`
//! parsing and serialisation, `URLSearchParams.set`, and the canonical forms that earn a `301`.

use sui_indexer_store::sui::{is_hex_id, normalize_sui_address};

/// Decoded query pairs, in request order (`URLSearchParams`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Params(pub Vec<(String, String)>);

/// Percent-decode bytes (`%XX` with two hex digits; anything else is kept as is).
fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(input[i + 1]), hex(input[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// One name or value of `application/x-www-form-urlencoded`: `+` is a space, then percent-decoding,
/// then UTF-8 decoding with replacement characters.
fn form_decode(s: &str) -> String {
    let bytes: Vec<u8> = s.bytes().map(|b| if b == b'+' { b' ' } else { b }).collect();
    String::from_utf8_lossy(&percent_decode(&bytes)).into_owned()
}

/// `application/x-www-form-urlencoded` serialisation of one name or value (the URL Standard's
/// urlencoded byte serializer: `*-._`, digits and letters kept, space as `+`, the rest `%XX`).
fn form_encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        match b {
            b'*' | b'-' | b'.' | b'_' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
}

impl Params {
    /// `new URLSearchParams(query)` (without the leading `?`).
    pub fn parse(query: &str) -> Params {
        Params(
            query
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|s| match s.split_once('=') {
                    Some((k, v)) => (form_decode(k), form_decode(v)),
                    None => (form_decode(s), String::new()),
                })
                .collect(),
        )
    }

    /// `URLSearchParams.prototype.toString()`.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for (i, (k, v)) in self.0.iter().enumerate() {
            if i > 0 {
                out.push('&');
            }
            form_encode(k, &mut out);
            out.push('=');
            form_encode(v, &mut out);
        }
        out
    }

    /// `URLSearchParams.prototype.get`: the first value of `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// `URLSearchParams.prototype.set`: replace the first `name` and drop the others, or append.
    pub fn set(&mut self, name: &str, value: String) {
        match self.0.iter().position(|(k, _)| k == name) {
            Some(i) => {
                self.0[i].1 = value;
                let mut seen = 0;
                self.0.retain(|(k, _)| {
                    if k != name {
                        return true;
                    }
                    seen += 1;
                    seen == 1
                });
            }
            None => self.0.push((name.to_owned(), value)),
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
}

/// `^\d{1,4}$` with ASCII digits (JavaScript's `\d`).
pub fn is_limit_syntax(v: &str) -> bool {
    (1..=4).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit())
}

/// JavaScript's default `Array.prototype.sort` order: UTF-16 code units.
fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// The distinct comma-separated items of `v`, in first-seen order (`[...new Set(v.split(','))]`).
pub fn distinct_items(v: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in v.split(',') {
        if !out.iter().any(|o| o == item) {
            out.push(item.to_owned());
        }
    }
    out
}

/// The query with every value in canonical form (gate lower-case and 64 digits, limit without
/// leading zeros, `kinds` sorted and unique), keeping the order the client sent. A request whose
/// query differs is answered with a cacheable 301 to this form, so the many spellings of one query
/// share one cache entry instead of each being a separate miss that reaches the origin.
pub fn canonical_query(q: &Params) -> Params {
    let mut out = Params::default();
    for (k, v) in &q.0 {
        let value = if k == "gate" && is_hex_id(v) {
            normalize_sui_address(v)
        } else if k == "limit" && is_limit_syntax(v) {
            v.parse::<u32>().expect("1-4 digits").to_string()
        } else if k == "kinds" {
            let mut items = distinct_items(v);
            items.sort_by(|a, b| utf16_cmp(a, b));
            items.join(",")
        } else {
            v.clone()
        };
        out.set(k, value);
    }
    out
}

/// The path with a gate id in its canonical form (stats route), else unchanged.
pub fn canonical_path(pathname: &str) -> String {
    let segs: Vec<&str> = pathname.split('/').collect();
    // ["", "v1", <network>, "gates", <id>, "stats"]
    if segs.len() == 6
        && segs[0].is_empty()
        && segs[1] == "v1"
        && !segs[2].is_empty()
        && segs[3] == "gates"
        && is_hex_id(segs[4])
        && segs[5] == "stats"
    {
        return format!("/v1/{}/gates/{}/stats", segs[2], normalize_sui_address(segs[4]));
    }
    pathname.to_owned()
}

fn is_single_dot(s: &str) -> bool {
    s == "." || s.eq_ignore_ascii_case("%2e")
}

fn is_double_dot(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), ".." | ".%2e" | "%2e." | "%2e%2e")
}

/// The pathname the URL Standard's parser produces for an absolute path: backslashes are slashes
/// (special schemes) and `.`/`..` segments (also percent-encoded) are resolved.
pub fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let segs: Vec<&str> = path.split('/').skip(1).collect();
    let mut out: Vec<&str> = Vec::new();
    let n = segs.len();
    for (i, seg) in segs.iter().enumerate() {
        let last = i + 1 == n;
        if is_double_dot(seg) {
            out.pop();
            if last {
                out.push("");
            }
        } else if is_single_dot(seg) {
            if last {
                out.push("");
            }
        } else {
            out.push(seg);
        }
    }
    format!("/{}", out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_like_urlsearchparams() {
        let p = Params::parse("a=1&&b&c=x+y%2Cz&d=%zz&=e&f=a=b");
        assert_eq!(
            p.0,
            vec![
                ("a".into(), "1".into()),
                ("b".into(), "".into()),
                ("c".into(), "x y,z".into()),
                ("d".into(), "%zz".into()),
                ("".into(), "e".into()),
                ("f".into(), "a=b".into()),
            ]
        );
        assert_eq!(p.serialize(), "a=1&b=&c=x+y%2Cz&d=%25zz&=e&f=a%3Db");
        assert_eq!(
            Params::parse("kinds=AccessConsumed,AccessMinted").serialize(),
            "kinds=AccessConsumed%2CAccessMinted"
        );
        assert_eq!(Params::parse("x=%C3%A9%FF").get("x"), Some("é\u{FFFD}"));
        assert_eq!(Params::parse("").serialize(), "");
    }

    #[test]
    fn set_replaces_the_first_and_drops_the_rest() {
        let mut p = Params::parse("limit=5&gate=0x1&limit=6");
        p.set("limit", "7".into());
        assert_eq!(p.serialize(), "limit=7&gate=0x1");
        p.set("kinds", "A".into());
        assert_eq!(p.serialize(), "limit=7&gate=0x1&kinds=A");
    }

    #[test]
    fn canonical_forms() {
        let full = format!("0x{}a", "0".repeat(63));
        assert_eq!(canonical_query(&Params::parse("gate=0xA")).serialize(), format!("gate={full}"));
        assert_eq!(canonical_query(&Params::parse("limit=05")).serialize(), "limit=5");
        assert_eq!(canonical_query(&Params::parse("limit=0005")).serialize(), "limit=5");
        assert_eq!(canonical_query(&Params::parse("limit=00000")).serialize(), "limit=00000");
        assert_eq!(
            canonical_query(&Params::parse("kinds=AccessMinted,AccessConsumed,AccessMinted")).serialize(),
            "kinds=AccessConsumed%2CAccessMinted"
        );
        // Duplicates collapse onto the first position with the last value (URLSearchParams.set).
        assert_eq!(
            canonical_query(&Params::parse("limit=5&gate=0xa&limit=6")).serialize(),
            format!("limit=6&gate={full}")
        );
        assert_eq!(canonical_path("/v1/testnet/gates/0xA/stats"), format!("/v1/testnet/gates/{full}/stats"));
        assert_eq!(canonical_path("/v1/testnet/gates/zz/stats"), "/v1/testnet/gates/zz/stats");
        assert_eq!(canonical_path("/v1//gates/0xA/stats"), "/v1//gates/0xA/stats");
    }

    #[test]
    fn resolves_dot_segments() {
        assert_eq!(normalize_path("/v1/./testnet/../testnet/coverage"), "/v1/testnet/coverage");
        assert_eq!(normalize_path("/v1/%2E%2e/x"), "/x");
        assert_eq!(normalize_path("/a/b/.."), "/a/");
        assert_eq!(normalize_path("/a/."), "/a/");
        assert_eq!(normalize_path("/../.."), "/");
        assert_eq!(normalize_path("/a\\b"), "/a/b");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("//healthz"), "//healthz");
    }

    fn query_text() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                Just("&".to_owned()),
                Just("=".to_owned()),
                Just("+".to_owned()),
                Just("%2C".to_owned()),
                Just("%".to_owned()),
                Just(",".to_owned()),
                Just("gate".to_owned()),
                Just("limit".to_owned()),
                Just("kinds".to_owned()),
                Just("0x0A".to_owned()),
                "[a-zA-Z0-9]{0,4}".prop_map(|s| s),
                "\\PC{0,2}".prop_map(|s| s),
            ],
            0..12,
        )
        .prop_map(|v| v.concat())
    }

    proptest! {
        /// Serialising and parsing again is the identity on decoded pairs.
        #[test]
        fn serialize_then_parse_round_trips(q in query_text()) {
            let p = Params::parse(&q);
            let again = Params::parse(&p.serialize());
            // Pairs with an empty name and value vanish in serialisation only as "=" (kept).
            prop_assert_eq!(again, p);
        }

        /// Canonicalisation is idempotent: the redirect target is itself canonical (no 301 loops).
        #[test]
        fn canonical_query_is_idempotent(q in query_text()) {
            let once = canonical_query(&Params::parse(&q));
            let twice = canonical_query(&Params::parse(&once.serialize()));
            prop_assert_eq!(once.serialize(), twice.serialize());
        }

        /// Every key appears once in the canonical query, in first-seen order.
        #[test]
        fn canonical_query_keys_are_unique(q in query_text()) {
            let c = canonical_query(&Params::parse(&q));
            let keys: Vec<String> = c.keys().map(str::to_owned).collect();
            let parsed = Params::parse(&q);
            let mut seen: Vec<String> = Vec::new();
            for k in parsed.keys() { if !seen.iter().any(|s| s == k) { seen.push(k.to_owned()); } }
            prop_assert_eq!(keys, seen);
        }

        #[test]
        fn canonical_path_is_idempotent(id in "0x[0-9a-fA-F]{1,64}", net in "[a-z]{1,8}") {
            let p = canonical_path(&format!("/v1/{net}/gates/{id}/stats"));
            prop_assert_eq!(canonical_path(&p), p.clone());
            let expected_len = format!("/v1/{net}/gates//stats").len() + 66;
            prop_assert!(p.ends_with("/stats") && p.len() == expected_len);
        }

        /// Path normalisation never panics, always yields an absolute path, and is idempotent.
        #[test]
        fn normalize_path_is_idempotent(segs in proptest::collection::vec(prop_oneof![Just(".".to_owned()), Just("..".to_owned()), Just("%2e".to_owned()), "[a-z0-9]{0,3}".prop_map(|s| s)], 0..8)) {
            let p = format!("/{}", segs.join("/"));
            let n = normalize_path(&p);
            prop_assert!(n.starts_with('/'));
            prop_assert_eq!(normalize_path(&n), n);
        }

        /// limit canonicalisation: digits only, no leading zeros, same value.
        #[test]
        fn limit_canonical(v in "[0-9]{1,4}") {
            let c = canonical_query(&Params::parse(&format!("limit={v}")));
            let got = c.get("limit").unwrap();
            prop_assert_eq!(got.parse::<u32>().unwrap(), v.parse::<u32>().unwrap());
            prop_assert!(got == "0" || !got.starts_with('0'));
        }
    }
}
