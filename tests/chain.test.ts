import { describe, it, expect } from 'vitest'
import { KNOWN_CHAIN_IDS, shortChainId } from '../src/chain.js'

describe('shortChainId', () => {
  it('maps the full base58 genesis digest a node reports to the short id used by Move and wallets', () => {
    // Values observed from the public full nodes (x-sui-chain-id).
    expect(shortChainId('69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD')).toBe(KNOWN_CHAIN_IDS.testnet)
    expect(shortChainId('4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S')).toBe(KNOWN_CHAIN_IDS.mainnet)
  })
  it('rejects anything that is not base58 of at least four bytes', () => {
    expect(() => shortChainId('')).toThrow()
    expect(() => shortChainId('0OIl')).toThrow()
    expect(() => shortChainId('2')).toThrow(/unexpected chain identifier/)
  })
})
