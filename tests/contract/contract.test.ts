// Client contract: the real `listAccessGateEvents` (@meddleware/access-gate-client) and
// `listSealedContent` (@meddleware/seal-client) read the built `api` binary over HTTP. The database is a
// throwaway PostgreSQL database with the two production roles, seeded by the testkit's `seed-fixture`
// (events BCS-encoded independently of the client parsers); the API runs as the SELECT-only role.
//
//   TEST_DATABASE_URL=postgres://postgres:pw@127.0.0.1:55432/postgres?sslmode=verify-full&sslrootcert=ca.crt \
//   API_BIN=../../target/release/api SEED_BIN=../../target/release/seed-fixture npm test
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { resolve } from 'node:path'
import { listAccessGateEvents } from '@meddleware/access-gate-client'
import { listSealedContent } from '@meddleware/seal-client'
import { normalizeSuiAddress } from '@mysten/sui/utils'

const API_BIN = resolve(process.env.API_BIN ?? '../../target/release/api')
const SEED_BIN = resolve(process.env.SEED_BIN ?? '../../target/release/seed-fixture')
const AG = normalizeSuiAddress('0xa1')
const SEAL = normalizeSuiAddress('0x5e')
const GATE_A = normalizeSuiAddress('0xa')
const GATE_B = normalizeSuiAddress('0xb')
const PORT = 18000 + Math.floor(Math.random() * 1000)
const BASE = `http://127.0.0.1:${PORT}`

/** A full node that must never be reached: every page has to come from the indexer. */
const noRpc = {
  core: {
    listEvents: async () => {
      throw new Error('the full node must not be called')
    },
  },
}

const ADMIN_URL = process.env.TEST_DATABASE_URL
let database: string
let api: ChildProcess

beforeAll(async () => {
  if (!ADMIN_URL) throw new Error('TEST_DATABASE_URL (a superuser URL of a PostgreSQL) is required')
  const seeded = spawnSync(SEED_BIN, ['seed', ADMIN_URL], { encoding: 'utf8' })
  if (seeded.status !== 0) throw new Error(`seed-fixture failed: ${seeded.stderr}`)
  const fixture = JSON.parse(seeded.stdout.trim().split('\n').pop() ?? '') as {
    database: string
    databaseUrl: string
    allowInsecure: boolean
  }
  database = fixture.database
  api = spawn(API_BIN, [], {
    env: {
      NETWORK: 'testnet',
      DATABASE_URL: fixture.databaseUrl,
      ...(fixture.allowInsecure ? { ALLOW_INSECURE_DATABASE: '1' } : {}),
      PORT: String(PORT),
      RATE_LIMIT_PER_SEC: '1000',
      RATE_LIMIT_BURST: '1000',
    },
    stdio: 'inherit',
  })
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`${BASE}/readyz`)).ok) return
    } catch {
      // not listening yet
    }
    await new Promise((r) => setTimeout(r, 100))
  }
  throw new Error('the api never became ready')
})

afterAll(() => {
  api?.kill('SIGTERM')
  if (ADMIN_URL && database) spawnSync(SEED_BIN, ['drop', ADMIN_URL, database], { encoding: 'utf8' })
})

const indexer = (url = BASE) => ({ url, network: 'testnet' })

describe('client contract (real client packages against the api binary)', () => {
  it('access-gate-client pages through the indexer', async () => {
    const first = await listAccessGateEvents(noRpc, { originalId: AG, limit: 3, indexer: indexer() })
    expect(first.source).toBe('indexer')
    expect(first.indexerError).toBeUndefined()
    expect(first.invalidRows ?? 0).toBe(0)
    expect(first.indexedFromCheckpoint).toBe('1')
    expect(first.events.map((e) => e.kind)).toEqual(['AccessConsumed', 'AccessMinted', 'AccessMinted'])
    const second = await listAccessGateEvents(noRpc, { originalId: AG, limit: 3, indexer: indexer(), cursor: first.cursor })
    expect(second.events).toHaveLength(2)
    expect(second.cursor).toBeNull()
    const consumes = await listAccessGateEvents(noRpc, { originalId: AG, kinds: ['AccessConsumed'], gateId: GATE_B, indexer: indexer() })
    expect(consumes.events).toMatchObject([{ kind: 'AccessConsumed', gateId: GATE_B }])
  })

  it('decodes every served row to the event the fixture encoded', async () => {
    const page = await listAccessGateEvents(noRpc, { originalId: AG, limit: 100, indexer: indexer() })
    expect(page.events).toHaveLength(5)
    const minted = page.events.filter((e) => e.kind === 'AccessMinted')
    expect(minted.map((e) => (e.kind === 'AccessMinted' ? e.commissionMist : null))).toEqual([2000n, 2000n, 2000n])
    expect(page.events.map((e) => e.checkpoint)).toEqual(['5', '4', '3', '2', '1'])
  })

  it('seal-client lists sealed content through the indexer', async () => {
    const page = await listSealedContent(noRpc, { originalId: SEAL, gateId: GATE_A, indexer: indexer(`${BASE}/`) })
    expect(page.source).toBe('indexer')
    expect(page.pointers.map((p) => p.label)).toEqual(['two', 'one'])
  })

  it('serves what the shared reader requires: no redirect for canonical queries, bounded size, typed fields', async () => {
    // readIndexerEvents fetches with `redirect: 'error'`: the client's canonical query must be served directly.
    const res = await fetch(`${BASE}/v1/testnet/access-gate/events?gate=${GATE_A}&limit=100`, { redirect: 'error' })
    expect(res.status).toBe(200)
    expect(res.headers.get('access-control-allow-origin')).toBe('*')
    const text = await res.text()
    expect(text.length).toBeLessThan(1 << 20) // MAX_INDEXER_RESPONSE
    const body = JSON.parse(text) as { events: Record<string, unknown>[] }
    for (const e of body.events) {
      expect(typeof e.eventType).toBe('string')
      expect(typeof e.sender).toBe('string')
      expect(typeof e.bcs).toBe('string')
      expect(typeof e.transactionDigest).toBe('string')
      expect(Number.isSafeInteger(e.eventIndex)).toBe(true)
      expect(e.checkpoint === null || typeof e.checkpoint === 'string').toBe(true)
    }
  })

  it('a non-canonical spelling is a 301 the client never follows', async () => {
    await expect(fetch(`${BASE}/v1/testnet/access-gate/events?limit=05`, { redirect: 'error' })).rejects.toThrow()
  })
})
