//! Sui identifier helpers: the exact semantics of `@mysten/sui/utils` that the indexer relied on.

/// `normalizeSuiAddress` of `@mysten/sui/utils`: lower-case, drop one `0x`, left-pad to 64 hex
/// digits, prefix `0x`. No validation (callers check [`is_hex_id`] first where input is untrusted).
pub fn normalize_sui_address(value: &str) -> String {
    let lower = value.to_lowercase();
    let digits = lower.strip_prefix("0x").unwrap_or(&lower);
    let pad = 64usize.saturating_sub(digits.chars().count());
    format!("0x{}{}", "0".repeat(pad), digits)
}

/// `^0x[0-9a-fA-F]{1,64}$`: the id syntax every input (query, path, env) must match.
pub fn is_hex_id(raw: &str) -> bool {
    match raw.strip_prefix("0x") {
        Some(d) => (1..=64).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_hexdigit()),
        None => false,
    }
}

/// A non-generic Move struct type `address::module::Name`, with the address normalised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructTag {
    pub address: String,
    pub module: String,
    pub name: String,
}

impl StructTag {
    /// Parse `addr::module::Name`. `None` for anything else, including generic types (`Name<T>`):
    /// none of the indexed events is generic, so such a type is never one of them.
    pub fn parse(t: &str) -> Option<StructTag> {
        let mut parts = t.split("::");
        let (a, m, n) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || !is_hex_id(a) || !is_ident(m) || !is_ident(n) {
            return None;
        }
        Some(StructTag { address: normalize_sui_address(a), module: m.to_owned(), name: n.to_owned() })
    }
}

impl std::fmt::Display for StructTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::{}::{}", self.address, self.module, self.name)
    }
}

/// A Move identifier: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_ident(s: &str) -> bool {
    let mut b = s.bytes();
    matches!(b.next(), Some(c) if c.is_ascii_alphabetic() || c == b'_')
        && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_like_the_ts_sdk() {
        assert_eq!(normalize_sui_address("0xABC"), format!("0x{}abc", "0".repeat(61)));
        assert_eq!(normalize_sui_address("abc"), format!("0x{}abc", "0".repeat(61)));
        assert_eq!(normalize_sui_address(&format!("0x{}", "D".repeat(64))), format!("0x{}", "d".repeat(64)));
    }

    #[test]
    fn hex_ids() {
        assert!(is_hex_id("0xA"));
        assert!(is_hex_id(&format!("0x{}", "f".repeat(64))));
        for bad in ["", "0x", "abc", "0x12;drop", "0Xab", &format!("0x{}", "f".repeat(65)), "0xg"] {
            assert!(!is_hex_id(bad), "{bad}");
        }
    }

    #[test]
    fn struct_tags() {
        let t = StructTag::parse("0xa1::access_gate::AccessMintedEvent").unwrap();
        assert_eq!(t.to_string(), format!("0x{}a1::access_gate::AccessMintedEvent", "0".repeat(62)));
        assert!(StructTag::parse("0xa1::access_gate::Foo<u64>").is_none());
        assert!(StructTag::parse("0xa1::access_gate").is_none());
        assert!(StructTag::parse("0xa1::a::b::c").is_none());
        assert!(StructTag::parse("zz::a::B").is_none());
    }
}
