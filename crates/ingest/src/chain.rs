//! Chain identity: the node must serve the network this instance is configured for.

/// The short chain ids used in Move/Published.toml and wallets: the first four bytes of the genesis
/// checkpoint digest, hex. A slice, not a map keyed by strings that could collide with anything else.
pub const KNOWN_CHAIN_IDS: &[(&str, &str)] = &[("testnet", "4c78adac"), ("mainnet", "35834a8a")];

/// The recorded short chain id of `network`, if any.
pub fn known_chain_id(network: &str) -> Option<&'static str> {
    KNOWN_CHAIN_IDS.iter().find(|(n, _)| *n == network).map(|(_, id)| *id)
}

/// The short chain id of a node's chain identifier: the node reports the full base58 genesis
/// checkpoint digest (`69WiPg3D…` for testnet); the short id is its first four bytes in hex.
///
/// # Errors
/// If the identifier is not base58 of at least four bytes.
pub fn short_chain_id(chain_identifier: &str) -> Result<String, String> {
    let bytes = bs58::decode(chain_identifier)
        .into_vec()
        .map_err(|_| format!("unexpected chain identifier: {chain_identifier}"))?;
    if bytes.len() < 4 {
        return Err(format!("unexpected chain identifier: {chain_identifier}"));
    }
    Ok(bytes[..4].iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_full_genesis_digest_to_the_short_id() {
        // Values observed from the public full nodes (x-sui-chain-id).
        assert_eq!(
            short_chain_id("69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD").unwrap(),
            known_chain_id("testnet").unwrap()
        );
        assert_eq!(
            short_chain_id("4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S").unwrap(),
            known_chain_id("mainnet").unwrap()
        );
    }

    /// F20: only the recorded networks resolve.
    #[test]
    fn has_no_inherited_entries() {
        for k in ["constructor", "toString", "__proto__", "hasOwnProperty", "devnet", ""] {
            assert_eq!(known_chain_id(k), None);
        }
        assert_eq!(KNOWN_CHAIN_IDS.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["testnet", "mainnet"]);
    }

    #[test]
    fn rejects_anything_that_is_not_base58_of_at_least_four_bytes() {
        assert!(short_chain_id("").is_err());
        assert!(short_chain_id("0OIl").is_err());
        assert!(short_chain_id("2").unwrap_err().contains("unexpected chain identifier"));
    }
}
