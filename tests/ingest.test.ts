import { describe, it, expect, vi } from 'vitest'
import { parseAccessGateEvent } from '@meddleware/access-gate-client'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { PackageChangedError, Store } from '../src/store.js'
import { Ingestor, streamFilter, toIndexed } from '../src/ingest.js'
import { AG, GATE_A, GATE_B, SEAL, consumed, fakeSource, minted, sealed } from './fixtures.js'

const opts = { accessGateOriginalId: AG, sealOriginalId: SEAL }
const AG_FILTER = `${AG}::access_gate`
const SEAL_FILTER = `${SEAL}::sealed_content::SealedContentPublished`

describe('toIndexed', () => {
  it('keeps kind, gate and commission from the client parser', () => {
    expect(toIndexed('access-gate', minted(GATE_A, 5, 777), opts)).toMatchObject({ kind: 'AccessMinted', gateId: GATE_A, commissionMist: 777n, checkpoint: '5' })
    expect(toIndexed('access-gate', consumed(GATE_B, 6), opts)).toMatchObject({ kind: 'AccessConsumed', gateId: GATE_B, commissionMist: null })
    expect(toIndexed('sealed-content', sealed(GATE_A, 7), opts)).toMatchObject({ kind: 'SealedContentPublished', gateId: GATE_A })
  })

  it('drops events of another package', () => {
    const foreign = { ...minted(GATE_A, 1), eventType: '0xa1a1::access_gate::AccessMintedEvent' }
    expect(toIndexed('access-gate', foreign, opts)).toBeNull()
  })

  it('polls the whole access_gate module and the one sealed-content event', () => {
    expect(streamFilter('access-gate', opts)).toBe(AG_FILTER)
    expect(streamFilter('sealed-content', opts)).toBe(SEAL_FILTER)
  })
})

describe('Ingestor', () => {
  it('stores every page with its cursor, ascending, and resumes from the cursor', async () => {
    const store = new Store(':memory:')
    const pages = { [AG_FILTER]: [[minted(GATE_A, 1)], [consumed(GATE_A, 2), minted(GATE_B, 3)]], [SEAL_FILTER]: [[sealed(GATE_A, 4)]] }
    const source = fakeSource(pages)
    const spy = vi.spyOn(source.core, 'listEvents')
    const ingestor = new Ingestor(store, source, opts)
    await ingestor.poll()
    expect(store.coverage('access-gate')).toEqual({ fromCheckpoint: '1', latestCheckpoint: '3', events: 3 })
    expect(store.coverage('sealed-content').events).toBe(1)
    expect(store.cursor('access-gate')).toBe('1')
    expect(ingestor.lastSuccessAt).toBeGreaterThan(0)

    // A later poll continues after the stored cursor; nothing is duplicated.
    pages[AG_FILTER].push([minted(GATE_A, 9)])
    spy.mockClear()
    await ingestor.poll()
    expect(spy.mock.calls[0][0]).toMatchObject({ after: '1', order: 'ascending' })
    expect(store.coverage('access-gate')).toMatchObject({ latestCheckpoint: '9', events: 4 })
  })

  it('re-ingesting the same events is a no-op', () => {
    const store = new Store(':memory:')
    const e = toIndexed('access-gate', minted(GATE_A, 1), opts)!
    expect(store.ingest('access-gate', [e], 'c0')).toBe(1)
    expect(store.ingest('access-gate', [e], 'c0')).toBe(0)
  })

  it('keeps the cursor when the node reports no position', async () => {
    const store = new Store(':memory:')
    store.ingest('access-gate', [], 'kept')
    await new Ingestor(store, fakeSource({}), opts).pollStream('access-gate')
    expect(store.cursor('access-gate')).toBe('kept')
  })

  it('bounds every full-node request, so a hung call fails the poll instead of stalling it', async () => {
    const store = new Store(':memory:')
    const hung = {
      core: {
        listEvents: (o: { signal?: AbortSignal }) =>
          new Promise<never>((_, reject) => o.signal?.addEventListener('abort', () => reject(o.signal?.reason))),
      },
    }
    const ingestor = new Ingestor(store, hung, { ...opts, requestTimeoutMs: 20 })
    await expect(ingestor.pollStream('access-gate')).rejects.toThrow(/timed out|abort/i)
  })

  it('stops at the page budget so one stream cannot starve the other', async () => {
    const store = new Store(':memory:')
    const many = Array.from({ length: 5 }, (_, i) => [minted(GATE_A, i + 1)])
    await new Ingestor(store, fakeSource({ [AG_FILTER]: many }), { ...opts, maxPagesPerPoll: 2 }).pollStream('access-gate')
    expect(store.coverage('access-gate').events).toBe(2)
  })

  it('a stored row decodes back to the same event', () => {
    const store = new Store(':memory:')
    store.ingest('access-gate', [toIndexed('access-gate', consumed(GATE_A, 3), opts)!], null)
    const [row] = store.page('access-gate', { limit: 1 }).rows
    const decoded = parseAccessGateEvent({ ...row, bcs: row.bcs }, AG)
    expect(decoded).toMatchObject({ kind: 'AccessConsumed', gateId: GATE_A })
  })
})

describe('Store.bindPackages', () => {
  const OTHER = '0x' + '77'.repeat(32)
  const bound = { 'access-gate': AG, 'sealed-content': SEAL } as const

  it('records the packages of a fresh index without clearing anything', () => {
    const store = new Store(':memory:')
    const generation = store.generation
    expect(store.bindPackages(bound)).toEqual([])
    expect(store.generation).toBe(generation)
  })

  it('keeps an index whose packages are unchanged', () => {
    const store = new Store(':memory:')
    store.bindPackages(bound)
    store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 1), opts)!], 'c1')
    const generation = store.generation
    expect(store.bindPackages(bound)).toEqual([])
    expect(store.coverage('access-gate').events).toBe(1)
    expect(store.cursor('access-gate')).toBe('c1')
    expect(store.generation).toBe(generation)
  })

  it('refuses to clear history without confirmation, and changes nothing', () => {
    const store = new Store(':memory:')
    store.bindPackages(bound)
    store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 1), opts)!], 'c1')
    const generation = store.generation
    expect(() => store.bindPackages({ ...bound, 'access-gate': OTHER })).toThrow(PackageChangedError)
    expect(() => store.bindPackages({ ...bound, 'access-gate': OTHER }, { confirm: ['sealed-content'] })).toThrow(/REINDEX_CONFIRM=access-gate/)
    expect(store.coverage('access-gate').events).toBe(1)
    expect(store.cursor('access-gate')).toBe('c1')
    expect(store.generation).toBe(generation)
    // The recorded binding is untouched, so the same refusal repeats until the operator decides.
    expect(() => store.bindPackages({ ...bound, 'access-gate': OTHER })).toThrow(PackageChangedError)
  })

  it('clears only the confirmed stream, backs the database up first, and rotates the generation', () => {
    const store = new Store(':memory:')
    store.bindPackages(bound)
    store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 1), opts)!], 'c1')
    store.ingest('sealed-content', [toIndexed('sealed-content', sealed(GATE_A, 2), opts)!], 's1')
    const generation = store.generation
    expect(store.bindPackages({ ...bound, 'access-gate': OTHER }, { confirm: ['access-gate'] })).toEqual(['access-gate'])
    expect(store.coverage('access-gate').events).toBe(0)
    expect(store.cursor('access-gate')).toBeNull()
    expect(store.coverage('sealed-content').events).toBe(1)
    expect(store.cursor('sealed-content')).toBe('s1')
    expect(store.generation).not.toBe(generation)
    // The new binding sticks.
    expect(store.bindPackages({ ...bound, 'access-gate': OTHER })).toEqual([])
  })

  it('treats rows indexed before packages were recorded like a package change', () => {
    const store = new Store(':memory:')
    store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 1), opts)!], 'c1')
    expect(() => store.bindPackages(bound)).toThrow(PackageChangedError)
    expect(store.bindPackages(bound, { confirm: ['access-gate'] })).toEqual(['access-gate'])
    expect(store.coverage('access-gate').events).toBe(0)
  })

  it('writes a restorable backup before clearing', () => {
    const dir = mkdtempSync(join(tmpdir(), 'idx-'))
    try {
      const store = new Store(join(dir, 'indexer.db'))
      store.bindPackages(bound)
      store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 1), opts)!], 'c1')
      const backupPath = join(dir, 'indexer.db.bak')
      store.bindPackages({ ...bound, 'access-gate': OTHER }, { confirm: ['access-gate'], backupPath })
      const backup = new Store(backupPath)
      expect(backup.coverage('access-gate').events).toBe(1)
      expect(backup.cursor('access-gate')).toBe('c1')
      backup.close()
      store.close()
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})

describe('Ingestor resilience', () => {
  const bad = (e: ReturnType<typeof minted>) => ({ ...e, bcs: new Uint8Array([1, 2, 3]) })

  it('skips and counts an undecodable event, keeps its page-mates, and advances', async () => {
    const store = new Store(':memory:')
    const logs: string[] = []
    const source = fakeSource({ [AG_FILTER]: [[minted(GATE_A, 1), bad(minted(GATE_A, 2)), minted(GATE_B, 3)]] })
    await new Ingestor(store, source, { ...opts, log: (m) => logs.push(m) }).poll()
    expect(store.coverage('access-gate').events).toBe(2)
    expect(store.health('access-gate').undecodable).toBe(1)
    expect(store.cursor('access-gate')).toBe('0') // advanced past the page
    expect(logs.some((l) => /UNDECODABLE/.test(l))).toBe(true)
  })

  it('polls the streams independently: one failing does not stop the other, and health names it', async () => {
    const store = new Store(':memory:')
    const ok = fakeSource({ [SEAL_FILTER]: [[sealed(GATE_A, 4)]] })
    const source = {
      core: {
        listEvents: (o: Parameters<typeof ok.core.listEvents>[0]) => {
          if (o.filter.eventType === AG_FILTER) throw new Error('node unavailable')
          return ok.core.listEvents(o)
        },
      },
    }
    const ingestor = new Ingestor(store, source, opts)
    await ingestor.poll()
    expect(store.coverage('sealed-content').events).toBe(1)
    expect(ingestor.health('access-gate')).toMatchObject({ lastSuccessAt: 0, lastError: 'node unavailable' })
    expect(ingestor.health('sealed-content')).toMatchObject({ lastError: null })
    expect(ingestor.health('sealed-content').lastSuccessAt).toBeGreaterThan(0)
    expect(ingestor.lastSuccessAt).toBe(0) // the oldest stream's success: not healthy as a whole
  })

  it('restarts from the node\'s earliest data when the stored cursor fell below its retention', async () => {
    const store = new Store(':memory:')
    store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 1), opts)!], 'stale-cursor')
    const logs: string[] = []
    let calls = 0
    const source = {
      core: {
        listEvents: async (o: { filter: { eventType: string }; after?: string | null }) => {
          calls++
          if (o.after === 'stale-cursor') throw new Error('requested data below earliest available; lowest available tx_seq is 4094238283')
          if (o.filter.eventType !== AG_FILTER) return { events: [], hasNextPage: false, endCursor: null }
          return o.after ? { events: [], hasNextPage: false, endCursor: null } : { events: [minted(GATE_A, 50)], hasNextPage: false, endCursor: 'fresh' }
        },
      },
    }
    const ingestor = new Ingestor(store, source, { ...opts, log: (m) => logs.push(m) })
    await ingestor.poll()
    expect(store.cursor('access-gate')).toBe('fresh')
    expect(store.coverage('access-gate')).toMatchObject({ events: 2, latestCheckpoint: '50' })
    expect(store.health('access-gate').gaps).toBe(1)
    expect(ingestor.health('access-gate').lastError).toBeNull()
    expect(logs.some((l) => /below the node's retention/.test(l))).toBe(true)
    expect(calls).toBeGreaterThan(1)
  })

  it('does not swallow other failures as a gap', async () => {
    const store = new Store(':memory:')
    store.ingest('access-gate', [], 'c')
    const source = { core: { listEvents: async () => { throw new Error('503 from fullnode') } } }
    const ingestor = new Ingestor(store, source, opts)
    await ingestor.poll()
    expect(store.cursor('access-gate')).toBe('c')
    expect(store.health('access-gate').gaps).toBe(0)
    expect(ingestor.health('access-gate').lastError).toBe('503 from fullnode')
  })
})

describe('Store counters', () => {
  it('stay exact across duplicates and agree with a recount (coverage and gate stats never scan)', () => {
    const store = new Store(':memory:')
    const rows = [minted(GATE_A, 7, 100), minted(GATE_A, 3, 200), consumed(GATE_A, 5), minted(GATE_B, 9, 50)].map((e) => toIndexed('access-gate', e, opts)!)
    store.ingest('access-gate', rows, 'c')
    store.ingest('access-gate', rows, 'c') // duplicates change nothing
    expect(store.coverage('access-gate')).toEqual({ fromCheckpoint: '3', latestCheckpoint: '9', events: 4 })
    expect(store.gateStats(GATE_A)).toEqual({ counts: { AccessMinted: 2, AccessConsumed: 1 }, commissionMist: 300n })
    expect(store.gateStats(GATE_B).commissionMist).toBe(50n)
    const recount = store.db.prepare(`SELECT COUNT(*) AS n, MIN(checkpoint) AS lo, MAX(checkpoint) AS hi FROM events WHERE stream = 'access-gate'`).get() as { n: number; lo: number; hi: number }
    expect([recount.n, recount.lo, recount.hi]).toEqual([4, 3, 9])
  })

  it('are rebuilt from existing rows for a database that predates them', () => {
    const dir = mkdtempSync(join(tmpdir(), 'idx-'))
    try {
      const path = join(dir, 'indexer.db')
      const first = new Store(path)
      first.ingest('access-gate', [minted(GATE_A, 1, 100), minted(GATE_A, 2, 250)].map((e) => toIndexed('access-gate', e, opts)!), 'c')
      first.db.exec(`DELETE FROM stream_stats; DELETE FROM gate_stats; DELETE FROM gate_commission; DELETE FROM meta WHERE key = 'stats:v2'`)
      first.close()
      const again = new Store(path)
      expect(again.coverage('access-gate')).toEqual({ fromCheckpoint: '1', latestCheckpoint: '2', events: 2 })
      expect(again.gateStats(GATE_A)).toEqual({ counts: { AccessMinted: 2 }, commissionMist: 350n })
      again.close()
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  it('coverage over 50k rows is a constant-time read', () => {
    const store = new Store(':memory:')
    store.db.exec('BEGIN')
    const ins = store.db.prepare(`INSERT INTO events (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind) VALUES ('access-gate', ?, 0, ?, 's', 't', x'00', 'AccessMinted')`)
    for (let i = 0; i < 50_000; i++) ins.run(`d${i}`, i)
    store.db.exec('COMMIT')
    store.db.exec(`DELETE FROM meta WHERE key = 'stats:v2'`)
    const t = performance.now()
    for (let i = 0; i < 200; i++) store.coverage('access-gate')
    expect(performance.now() - t).toBeLessThan(200) // 200 reads: a scan would take seconds
  })
})

