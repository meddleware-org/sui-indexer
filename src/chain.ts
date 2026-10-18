import { fromBase58 } from '@mysten/sui/utils'

/** The short chain ids used in Move/Published.toml and wallets: the first four bytes of the genesis digest, hex. */
// A Map, not a plain object: the key is the operator's `NETWORK` string, and `KNOWN_CHAIN_IDS['constructor']`
// would resolve to an inherited function.
export const KNOWN_CHAIN_IDS: ReadonlyMap<string, string> = new Map([
  ['testnet', '4c78adac'],
  ['mainnet', '35834a8a'],
])

/**
 * The short chain id of a node's `getChainIdentifier()` result. The node reports the full base58
 * genesis checkpoint digest (`69WiPg3D…` for testnet); the short id is its first four bytes in hex.
 *
 * @throws {Error} if the identifier is not base58 of at least four bytes.
 */
export function shortChainId(chainIdentifier: string): string {
  const bytes = fromBase58(chainIdentifier)
  if (bytes.length < 4) throw new Error(`unexpected chain identifier: ${chainIdentifier}`)
  return Array.from(bytes.slice(0, 4), (b) => b.toString(16).padStart(2, '0')).join('')
}
