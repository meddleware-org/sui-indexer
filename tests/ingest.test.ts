import { describe, it, expect, vi } from 'vitest'
import { parseAccessGateEvent } from '@meddleware/access-gate-client'
import { Store } from '../src/store.js'
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

  it('clears only the stream whose package changed and rotates the generation', () => {
    const store = new Store(':memory:')
    store.bindPackages(bound)
    store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 1), opts)!], 'c1')
    store.ingest('sealed-content', [toIndexed('sealed-content', sealed(GATE_A, 2), opts)!], 's1')
    const generation = store.generation
    expect(store.bindPackages({ ...bound, 'access-gate': OTHER })).toEqual(['access-gate'])
    expect(store.coverage('access-gate').events).toBe(0)
    expect(store.cursor('access-gate')).toBeNull()
    expect(store.coverage('sealed-content').events).toBe(1)
    expect(store.cursor('sealed-content')).toBe('s1')
    expect(store.generation).not.toBe(generation)
    // The new binding sticks.
    expect(store.bindPackages({ ...bound, 'access-gate': OTHER })).toEqual([])
  })

  it('clears rows indexed before packages were recorded', () => {
    const store = new Store(':memory:')
    store.ingest('access-gate', [toIndexed('access-gate', minted(GATE_A, 1), opts)!], 'c1')
    expect(store.bindPackages(bound)).toEqual(['access-gate'])
    expect(store.coverage('access-gate').events).toBe(0)
  })
})
