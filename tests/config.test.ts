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

  it('validates and normalises package-id overrides', () => {
    const c = loadConfig({ ACCESS_GATE_ORIGINAL_ID: '0xABC', SEAL_ORIGINAL_ID: `0x${'D'.repeat(64)}` })
    expect(c.accessGateOriginalId).toBe(`0x${'0'.repeat(61)}abc`)
    expect(c.sealOriginalId).toBe(`0x${'d'.repeat(64)}`)
    expect(() => loadConfig({ ACCESS_GATE_ORIGINAL_ID: 'abc' })).toThrow(/ACCESS_GATE_ORIGINAL_ID/)
    expect(() => loadConfig({ SEAL_ORIGINAL_ID: '0x12;drop' })).toThrow(/SEAL_ORIGINAL_ID/)
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
