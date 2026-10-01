import { describe, it, expect } from 'vitest'
import { accessGateDeployment } from '@meddleware/access-gate-client/deployments'
import { loadConfig } from '../src/config.js'
import { RateLimiter } from '../src/ratelimit.js'

describe('loadConfig', () => {
  it('defaults to testnet with the recorded package ids', () => {
    const c = loadConfig({})
    expect(c).toMatchObject({ network: 'testnet', port: 8080, pollMs: 15_000, dbPath: '/data/indexer.db' })
    expect(c.accessGateOriginalId).toBe(accessGateDeployment('testnet').originalId)
    expect(c.sealOriginalId).toMatch(/^0x[0-9a-f]{64}$/)
  })

  it('refuses bad values and unknown networks', () => {
    expect(() => loadConfig({ POLL_MS: '10' })).toThrow(/POLL_MS/)
    expect(() => loadConfig({ PORT: 'x' })).toThrow(/PORT/)
    expect(() => loadConfig({ GRPC_URL: 'http://node.example' })).toThrow(/https/)
    expect(() => loadConfig({ NETWORK: 'devnet', GRPC_URL: 'https://n.example' })).toThrow(/no access_gate deployment/)
  })
})

describe('RateLimiter', () => {
  it('refills at the configured rate and sweeps idle buckets', () => {
    let t = 0
    const l = new RateLimiter(2, 2, () => t)
    expect([l.take('a'), l.take('a'), l.take('a')]).toEqual([true, true, false])
    expect(l.retryAfter('a')).toBe(1)
    t = 500
    expect(l.take('a')).toBe(true)
    t = 120_000
    l.take('b')
    expect(l.size).toBe(1)
  })
})
