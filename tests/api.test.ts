import { describe, it, expect } from 'vitest'
import { listAccessGateEvents } from '@meddleware/access-gate-client'
import { listSealedContent } from '@meddleware/seal-client'
import { Store } from '../src/store.js'
import { RateLimiter } from '../src/ratelimit.js'
import { clientKey, handle, type ApiContext } from '../src/api.js'
import { toIndexed } from '../src/ingest.js'
import { AG, GATE_A, GATE_B, SEAL, consumed, minted, sealed } from './fixtures.js'

const opts = { accessGateOriginalId: AG, sealOriginalId: SEAL }

function setup(limiter = new RateLimiter(1000, 1000)) {
  const store = new Store(':memory:')
  store.ingest(
    'access-gate',
    [minted(GATE_A, 1), consumed(GATE_A, 2), minted(GATE_B, 3), minted(GATE_A, 4), consumed(GATE_B, 5)].map(
      (e) => toIndexed('access-gate', e, opts)!,
    ),
    'c',
  )
  store.ingest('sealed-content', [sealed(GATE_A, 6, 'one'), sealed(GATE_B, 7, 'other'), sealed(GATE_A, 8, 'two')].map((e) => toIndexed('sealed-content', e, opts)!), 's')
  const polls: Record<'access-gate' | 'sealed-content', { lastSuccessAt: number; lastError: string | null }> = {
    'access-gate': { lastSuccessAt: Date.now(), lastError: null },
    'sealed-content': { lastSuccessAt: Date.now(), lastError: null },
  }
  const ctx: ApiContext = { store, network: 'testnet', limiter, health: (s) => polls[s], pollMs: 15_000 }
  const get = (url: string, headers: Record<string, string> = {}) => handle(ctx, { method: 'GET', url, headers, remoteAddress: '10.0.0.1' })
  const setLastPoll = (t: number, stream: 'access-gate' | 'sealed-content' = 'access-gate') => (polls[stream].lastSuccessAt = t)
  return { store, ctx, get, setLastPoll, polls }
}

const body = (r: { body: string }) => JSON.parse(r.body)

describe('events pages', () => {
  it('serves the newest page with short cache headers and an ETag', () => {
    const { get } = setup()
    const r = get('/v1/testnet/access-gate/events?limit=2')
    expect(r.status).toBe(200)
    expect(r.headers['Cache-Control']).toBe('public, max-age=10, s-maxage=30')
    expect(r.headers['Access-Control-Allow-Origin']).toBe('*')
    const b = body(r)
    expect(b.events.map((e: { checkpoint: string }) => e.checkpoint)).toEqual(['5', '4'])
    expect(b.cursor).toMatch(/^[0-9a-f]{12}\.\d+$/)
    expect(b.indexedFromCheckpoint).toBe('1')
    expect(get('/v1/testnet/access-gate/events?limit=2', { 'if-none-match': r.headers.ETag }).status).toBe(304)
  })

  it('serves older pages as immutable and ends with a null cursor', () => {
    const { get } = setup()
    const first = body(get('/v1/testnet/access-gate/events?limit=2'))
    const second = get(`/v1/testnet/access-gate/events?limit=2&before=${first.cursor}`)
    expect(second.headers['Cache-Control']).toBe('public, max-age=31536000, immutable')
    const b2 = body(second)
    expect(b2.events.map((e: { checkpoint: string }) => e.checkpoint)).toEqual(['3', '2'])
    const b3 = body(get(`/v1/testnet/access-gate/events?limit=2&before=${b2.cursor}`))
    expect(b3.events.map((e: { checkpoint: string }) => e.checkpoint)).toEqual(['1'])
    expect(b3.cursor).toBeNull()
  })

  it('filters by kinds and gate', () => {
    const { get } = setup()
    const b = body(get(`/v1/testnet/access-gate/events?kinds=AccessConsumed&gate=${GATE_A}`))
    expect(b.events).toHaveLength(1)
    expect(b.events[0].eventType).toMatch(/AccessConsumedEvent$/)
  })

  it('refuses a cursor from another index generation', () => {
    const { get } = setup()
    expect(get('/v1/testnet/access-gate/events?before=000000000000.5').status).toBe(410)
  })

  it('validates strictly', () => {
    const { get } = setup()
    for (const url of [
      '/v1/testnet/access-gate/events?limit=0',
      '/v1/testnet/access-gate/events?limit=101',
      '/v1/testnet/access-gate/events?limit=abc',
      '/v1/testnet/access-gate/events?gate=zz',
      '/v1/testnet/access-gate/events?kinds=Nope',
      '/v1/testnet/access-gate/events?kinds=',
      '/v1/testnet/access-gate/events?before=garbage',
      '/v1/testnet/access-gate/events?cachebuster=1',
      '/v1/testnet/sealed-content',
    ]) {
      expect(get(url).status, url).toBe(400)
    }
    expect(get('/v1/mainnet/access-gate/events').status).toBe(404)
    expect(get('/v1/testnet/nothing').status).toBe(404)
  })

  it('lists sealed content for one gate', () => {
    const { get } = setup()
    const b = body(get(`/v1/testnet/sealed-content?gate=${GATE_A}`))
    expect(b.events).toHaveLength(2)
  })
})

describe('stats, coverage and health', () => {
  it('counts a gate\'s events and sums commission exactly', () => {
    const { store, get } = setup()
    const big = 2n ** 60n
    store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 10, big), opts)!, toIndexed('access-gate', minted(GATE_A, 11, big), opts)!], 'c2')
    const b = body(get(`/v1/testnet/gates/${GATE_A}/stats`))
    expect(b).toMatchObject({ minted: 4, consumed: 1, burned: 0 })
    expect(b.commissionMist).toBe((2n * big + 4000n).toString())
  })

  it('reports coverage per stream', () => {
    const { get } = setup()
    const b = body(get('/v1/testnet/coverage'))
    expect(b.accessGate).toEqual({ fromCheckpoint: '1', latestCheckpoint: '5', events: 5, undecodable: 0, gaps: 0 })
    expect(b.sealedContent.events).toBe(3)
  })

  it('healthz is 503 before the first poll and after polls stall, and names the stream and its error', () => {
    const { get, setLastPoll, polls } = setup()
    expect(get('/healthz').status).toBe(200)
    setLastPoll(0)
    expect(get('/healthz').status).toBe(503)
    setLastPoll(Date.now() - 5 * 15_000 - 1)
    polls['access-gate'].lastError = 'requested data below earliest available'
    const res = get('/healthz')
    expect(res.status).toBe(503)
    expect(body(res).streams['access-gate']).toMatchObject({ ok: false, lastError: 'requested data below earliest available' })
    expect(body(res).streams['sealed-content'].ok).toBe(true) // the other stream is fine
    expect(get('/readyz').status).toBe(200) // still serving
  })

  it('head pages say how fresh the index is; older pages stay immutable and free of it', () => {
    const { get } = setup()
    const head = body(get('/v1/testnet/access-gate/events?limit=2'))
    expect(head).toMatchObject({ latestCheckpoint: '5', polledAt: expect.any(Number) })
    const older = body(get(`/v1/testnet/access-gate/events?limit=2&before=${head.cursor}`))
    expect(older).not.toHaveProperty('polledAt')
    expect(older).not.toHaveProperty('latestCheckpoint')
  })
})

describe('canonical URLs (one cache entry per query)', () => {
  const loc = (r: { status: number; headers: Record<string, string> }) => (r.status === 301 ? r.headers.Location : null)

  it('redirects other spellings of a query to the canonical form, cacheably', () => {
    const { get } = setup()
    const full = `0x${'0'.repeat(63)}a`
    expect(loc(get('/v1/testnet/access-gate/events?gate=0xA'))).toBe(`/v1/testnet/access-gate/events?gate=${full}`)
    expect(loc(get('/v1/testnet/access-gate/events?gate=0x0a'))).toBe(`/v1/testnet/access-gate/events?gate=${full}`)
    expect(loc(get(`/v1/testnet/access-gate/events?gate=${full.toUpperCase().replace('0X', '0x')}`))).toBe(`/v1/testnet/access-gate/events?gate=${full}`)
    expect(loc(get('/v1/testnet/access-gate/events?limit=05'))).toBe('/v1/testnet/access-gate/events?limit=5')
    expect(loc(get('/v1/testnet/access-gate/events?kinds=AccessMinted,AccessConsumed,AccessMinted'))).toBe(
      '/v1/testnet/access-gate/events?kinds=AccessConsumed%2CAccessMinted',
    )
    expect(loc(get('/v1/testnet/gates/0xA/stats'))).toBe(`/v1/testnet/gates/${full}/stats`)
    const r = get('/v1/testnet/access-gate/events?limit=05')
    expect(r.headers['Cache-Control']).toBe('public, max-age=86400')
    expect(r.body).toBe('')
  })

  it('serves canonical queries (what the client packages send) without a redirect', () => {
    const { get } = setup()
    const g = `0x${'0'.repeat(63)}a`
    for (const url of [
      `/v1/testnet/access-gate/events?kinds=AccessConsumed%2CAccessMinted&gate=${g}&limit=10`,
      `/v1/testnet/access-gate/events?gate=${g}&limit=5`,
      '/v1/testnet/coverage',
      `/v1/testnet/gates/${g}/stats`,
    ]) {
      expect(get(url).status, url).toBe(200)
    }
  })

  it('the redirect target is itself canonical (no loops)', () => {
    const { get } = setup()
    const first = get('/v1/testnet/access-gate/events?limit=05&gate=0xA')
    expect(first.status).toBe(301)
    expect(get(first.headers.Location!).status).toBe(200)
  })
})

describe('transport rules', () => {
  it('keys IPv6 clients by their /64 and caps the bucket map', () => {
    expect(clientKey('2001:db8:1:2:aaaa:bbbb:cccc:dddd')).toBe(clientKey('2001:db8:1:2::1'))
    expect(clientKey('2001:db8:1:2::1')).not.toBe(clientKey('2001:db8:1:3::1'))
    expect(clientKey('203.0.113.9')).toBe('203.0.113.9')
    expect(clientKey('::ffff:203.0.113.9')).toBe('::ffff:203.0.113.9')
    const limiter = new RateLimiter(1, 1, () => 0, 100)
    for (let i = 0; i < 1000; i++) limiter.take(`k${i}`)
    expect(limiter.size).toBeLessThanOrEqual(100)
  })

  it('revalidates with weak, listed or wildcard If-None-Match', () => {
    const { get } = setup()
    const etag = get('/v1/testnet/access-gate/events').headers.ETag!
    for (const h of [etag, `W/${etag}`, `"zzz", ${etag}`, '*']) {
      expect(get('/v1/testnet/access-gate/events', { 'if-none-match': h }).status, h).toBe(304)
    }
    expect(get('/v1/testnet/access-gate/events', { 'if-none-match': '"other"' }).status).toBe(200)
  })

  it('rate-limits per client IP, preferring CF-Connecting-IP, and never limits healthz', () => {
    const { get } = setup(new RateLimiter(1, 2))
    expect(get('/v1/testnet/coverage', { 'cf-connecting-ip': '1.1.1.1' }).status).toBe(200)
    expect(get('/v1/testnet/coverage', { 'cf-connecting-ip': '1.1.1.1' }).status).toBe(200)
    const limited = get('/v1/testnet/coverage', { 'cf-connecting-ip': '1.1.1.1' })
    expect(limited.status).toBe(429)
    expect(Number(limited.headers['Retry-After'])).toBeGreaterThanOrEqual(1)
    expect(get('/v1/testnet/coverage', { 'cf-connecting-ip': '2.2.2.2' }).status).toBe(200)
    expect(get('/healthz', { 'cf-connecting-ip': '1.1.1.1' }).status).toBe(200)
  })

  it('answers preflight, refuses writes, and sends no body for HEAD', () => {
    const { ctx } = setup()
    expect(handle(ctx, { method: 'OPTIONS', url: '/v1/testnet/coverage', headers: {} }).status).toBe(204)
    expect(handle(ctx, { method: 'POST', url: '/v1/testnet/coverage', headers: {} }).status).toBe(405)
    const head = handle(ctx, { method: 'HEAD', url: '/v1/testnet/coverage', headers: {} })
    expect(head.status).toBe(200)
    expect(head.body).toBe('')
  })
})

describe('client contract', () => {
  function indexerFetch(ctx: ApiContext): typeof fetch {
    return (async (input: URL | string | Request) => {
      const url = new URL(String(input))
      const r = handle(ctx, { method: 'GET', url: url.pathname + url.search, headers: {} })
      return new Response(r.body || null, { status: r.status, headers: r.headers })
    }) as typeof fetch
  }
  const noRpc = { core: { listEvents: async () => { throw new Error('the full node must not be called') } } }

  it('access-gate-client pages through the indexer', async () => {
    const { ctx } = setup()
    const indexer = { url: 'https://indexer.example', network: 'testnet', fetch: indexerFetch(ctx) }
    const first = await listAccessGateEvents(noRpc, { originalId: AG, limit: 3, indexer })
    expect(first.source).toBe('indexer')
    expect(first.indexerError).toBeUndefined()
    expect(first.events.map((e) => e.kind)).toEqual(['AccessConsumed', 'AccessMinted', 'AccessMinted'])
    const second = await listAccessGateEvents(noRpc, { originalId: AG, limit: 3, indexer, cursor: first.cursor })
    expect(second.events).toHaveLength(2)
    expect(second.cursor).toBeNull()
    const consumes = await listAccessGateEvents(noRpc, { originalId: AG, kinds: ['AccessConsumed'], gateId: GATE_B, indexer })
    expect(consumes.events).toMatchObject([{ kind: 'AccessConsumed', gateId: GATE_B }])
  })

  it('seal-client lists sealed content through the indexer', async () => {
    const { ctx } = setup()
    const page = await listSealedContent(noRpc, {
      originalId: SEAL,
      gateId: GATE_A,
      indexer: { url: 'https://indexer.example/', network: 'testnet', fetch: indexerFetch(ctx) },
    })
    expect(page.source).toBe('indexer')
    expect(page.pointers.map((p) => p.label)).toEqual(['two', 'one'])
  })
})
