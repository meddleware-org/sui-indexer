import { normalizeSuiAddress } from '@mysten/sui/utils'
import { ACCESS_GATE_EVENT_KINDS } from '@meddleware/access-gate-client'
import type { EventRow, Store, Stream } from './store.js'
import type { RateLimiter } from './ratelimit.js'
import type { StreamHealth } from './ingest.js'

export interface ApiRequest {
  method: string
  /** Path and query, e.g. `/v1/testnet/access-gate/events?limit=5`. */
  url: string
  headers: Record<string, string | string[] | undefined>
  /** Socket address, used when no `CF-Connecting-IP` header is present. */
  remoteAddress?: string
}

export interface ApiResponse {
  status: number
  headers: Record<string, string>
  body: string
}

export interface ApiContext {
  store: Store
  network: string
  limiter: RateLimiter
  /** Poll state of one stream (last success, last error). */
  health: (stream: Stream) => StreamHealth
  pollMs: number
  now?: () => number
}

/** Older pages never change once served (row ids follow ledger order). */
const IMMUTABLE = 'public, max-age=31536000, immutable'
/** The newest page changes as events land: short browser TTL, slightly longer at the edge. */
const HEAD = 'public, max-age=10, s-maxage=30'

const BASE_HEADERS = {
  'Access-Control-Allow-Origin': '*',
  'X-Content-Type-Options': 'nosniff',
}

class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

function json(status: number, body: unknown, headers: Record<string, string> = {}): ApiResponse {
  return {
    status,
    headers: { ...BASE_HEADERS, 'Content-Type': 'application/json; charset=utf-8', ...headers },
    body: JSON.stringify(body),
  }
}

/**
 * The rate-limit bucket of a client address. IPv6 clients are keyed by their /64 (one subscriber
 * controls the whole prefix, so per-address buckets would be unlimited); IPv4 and anything else are used as is.
 */
export function clientKey(ip: string): string {
  const v = ip.trim().replace(/^\[|\]$/g, '').split('%')[0] ?? ''
  if (!v.includes(':') || v.includes('.')) return ip // IPv4, IPv4-mapped IPv6, or not an address
  const [head = '', tail = ''] = v.split('::')
  const h = head === '' ? [] : head.split(':')
  const t = tail === '' || !v.includes('::') ? [] : tail.split(':')
  const groups = v.includes('::') ? [...h, ...Array(Math.max(0, 8 - h.length - t.length)).fill('0'), ...t] : h
  return `${groups.slice(0, 4).map((g) => g.toLowerCase().replace(/^0+(?=.)/, '')).join(':')}::/64`
}

function header(req: ApiRequest, name: string): string | undefined {
  const v = req.headers[name]
  return Array.isArray(v) ? v[0] : v
}

function onlyParams(q: URLSearchParams, allowed: readonly string[]): void {
  for (const key of q.keys()) {
    if (!allowed.includes(key)) throw new HttpError(400, `unknown query parameter: ${key}`)
  }
}

function parseLimit(raw: string | null, fallback: number, max: number): number {
  if (raw === null) return fallback
  if (!/^\d{1,4}$/.test(raw)) throw new HttpError(400, 'limit must be an integer')
  const v = Number(raw)
  if (v < 1 || v > max) throw new HttpError(400, `limit must be in [1, ${max}]`)
  return v
}

function parseId(raw: string | null, what: string): string | undefined {
  if (raw === null) return undefined
  if (!/^0x[0-9a-fA-F]{1,64}$/.test(raw)) throw new HttpError(400, `${what} must be a 0x-prefixed hex id`)
  return normalizeSuiAddress(raw)
}

/** Cursor = `<generation>.<row id>`; a cursor from another database generation is refused. */
function parseBefore(raw: string | null, generation: string): number | undefined {
  if (raw === null) return undefined
  const m = /^([0-9a-f]{12})\.(\d{1,15})$/.exec(raw)
  if (!m) throw new HttpError(400, 'before must be a cursor from a previous page')
  if (m[1] !== generation) throw new HttpError(410, 'cursor is from an earlier index; start from the newest page')
  return Number(m[2])
}

function serialize(row: EventRow) {
  return {
    eventType: row.eventType,
    sender: row.sender,
    bcs: Buffer.from(row.bcs).toString('base64'),
    checkpoint: row.checkpoint,
    transactionDigest: row.transactionDigest,
    eventIndex: row.eventIndex,
  }
}

/**
 * RFC 9110 `If-None-Match`: a list of entity tags (weak or strong, weak comparison) or `*`.
 * Cloudflare weakens ETags when it compresses, so an exact string comparison would never match.
 */
function etagMatches(header: string | undefined, etag: string): boolean {
  if (!header) return false
  if (header.trim() === '*') return true
  return header.split(',').some((t) => t.trim().replace(/^W\//, '') === etag)
}

/**
 * The query with every value in canonical form (gate lower-case and 64 digits, limit without leading
 * zeros, `kinds` sorted and unique), keeping the order the client sent. A request whose query differs
 * is answered with a cacheable 301 to this form, so the many spellings of one query share one cache
 * entry instead of each being a separate miss that reaches the origin.
 */
function canonicalQuery(q: URLSearchParams): string {
  const out = new URLSearchParams()
  for (const [k, v] of q) {
    if (k === 'gate' && /^0x[0-9a-fA-F]{1,64}$/.test(v)) out.set(k, normalizeSuiAddress(v))
    else if (k === 'limit' && /^\d{1,4}$/.test(v)) out.set(k, String(Number(v)))
    else if (k === 'kinds') out.set(k, [...new Set(v.split(','))].sort().join(','))
    else out.set(k, v)
  }
  return out.toString()
}

/** The path with a gate id in its canonical form (stats route), else unchanged. */
function canonicalPath(pathname: string): string {
  return pathname.replace(/^(\/v1\/[^/]+\/gates\/)(0x[0-9a-fA-F]{1,64})(\/stats)$/, (_m, a: string, id: string, b: string) => `${a}${normalizeSuiAddress(id)}${b}`)
}

function eventsPage(
  ctx: ApiContext,
  req: ApiRequest,
  stream: Stream,
  q: { kinds?: string[]; gateId?: string; beforeId?: number; limit: number },
): ApiResponse {
  const { store } = ctx
  const cacheControl = q.beforeId === undefined ? HEAD : IMMUTABLE
  const etag = `"${store.generation}-${q.beforeId === undefined ? store.headId(stream) : `b${q.beforeId}`}"`
  if (etagMatches(header(req, 'if-none-match'), etag)) {
    return { status: 304, headers: { ...BASE_HEADERS, 'Cache-Control': cacheControl, ETag: etag }, body: '' }
  }
  const { rows, next } = store.page(stream, q)
  const coverage = store.coverage(stream)
  const head = q.beforeId === undefined
  return json(
    200,
    {
      events: rows.map(serialize),
      cursor: next === null ? null : `${store.generation}.${next}`,
      ...(coverage.fromCheckpoint === null ? {} : { indexedFromCheckpoint: coverage.fromCheckpoint }),
      // Only the head page says how fresh the index is (older pages are immutable and cached for a
      // year): a client treats a head page whose `polledAt` is old as unavailable and falls back.
      ...(head ? { latestCheckpoint: coverage.latestCheckpoint, polledAt: ctx.health(stream).lastSuccessAt || null } : {}),
    },
    { 'Cache-Control': cacheControl, ETag: etag },
  )
}

function route(ctx: ApiContext, req: ApiRequest, url: URL): ApiResponse {
  const parts = url.pathname.split('/').filter(Boolean)
  const q = url.searchParams
  const now = (ctx.now ?? Date.now)()

  if (parts.length === 1 && parts[0] === 'readyz') {
    // Serving works while polls stall: stale data is still useful, and clients fall back anyway.
    onlyParams(q, [])
    return json(200, { ok: true }, { 'Cache-Control': 'no-store' })
  }

  if (parts.length === 1 && parts[0] === 'healthz') {
    onlyParams(q, [])
    const streams = Object.fromEntries(
      (['access-gate', 'sealed-content'] as const).map((s) => {
        const h = ctx.health(s)
        const ageMs = h.lastSuccessAt ? now - h.lastSuccessAt : null
        return [s, { ok: ageMs !== null && ageMs < ctx.pollMs * 5, lastPollAgeMs: ageMs, lastError: h.lastError }]
      }),
    )
    const ok = Object.values(streams).every((s) => s.ok)
    return json(ok ? 200 : 503, { ok, streams }, { 'Cache-Control': 'no-store' })
  }

  if (parts[0] !== 'v1' || parts.length < 3) throw new HttpError(404, 'not found')
  if (parts[1] !== ctx.network) throw new HttpError(404, `this indexer serves ${ctx.network} only`)
  const rest = parts.slice(2).join('/')

  if (rest === 'coverage') {
    onlyParams(q, [])
    return json(
      200,
      {
        network: ctx.network,
        generation: ctx.store.generation,
        accessGate: { ...ctx.store.coverage('access-gate'), ...ctx.store.health('access-gate') },
        sealedContent: { ...ctx.store.coverage('sealed-content'), ...ctx.store.health('sealed-content') },
      },
      { 'Cache-Control': HEAD },
    )
  }

  if (rest === 'access-gate/events') {
    onlyParams(q, ['kinds', 'gate', 'before', 'limit'])
    let kinds: string[] | undefined
    const rawKinds = q.get('kinds')
    if (rawKinds !== null) {
      kinds = [...new Set(rawKinds.split(','))]
      const unknown = kinds.filter((k) => !(ACCESS_GATE_EVENT_KINDS as readonly string[]).includes(k))
      if (!kinds.length || unknown.length) throw new HttpError(400, `unknown kinds: ${unknown.join(', ') || '(empty)'}`)
    }
    return eventsPage(ctx, req, 'access-gate', {
      kinds,
      gateId: parseId(q.get('gate'), 'gate'),
      beforeId: parseBefore(q.get('before'), ctx.store.generation),
      limit: parseLimit(q.get('limit'), 20, 100),
    })
  }

  if (rest === 'sealed-content') {
    onlyParams(q, ['gate', 'before', 'limit'])
    const gateId = parseId(q.get('gate'), 'gate')
    if (!gateId) throw new HttpError(400, 'gate is required')
    return eventsPage(ctx, req, 'sealed-content', {
      gateId,
      beforeId: parseBefore(q.get('before'), ctx.store.generation),
      limit: parseLimit(q.get('limit'), 50, 200),
    })
  }

  if (parts.length === 5 && parts[2] === 'gates' && parts[4] === 'stats') {
    onlyParams(q, [])
    const gateId = parseId(parts[3], 'gate id')!
    const { counts, commissionMist } = ctx.store.gateStats(gateId)
    return json(
      200,
      {
        gateId,
        minted: counts.AccessMinted ?? 0,
        consumed: counts.AccessConsumed ?? 0,
        burned: counts.AccessBurned ?? 0,
        commissionMist: commissionMist.toString(),
        indexedFromCheckpoint: ctx.store.coverage('access-gate').fromCheckpoint,
      },
      { 'Cache-Control': HEAD },
    )
  }

  throw new HttpError(404, 'not found')
}

/** Handle one request. Pure with respect to the transport, so it is tested without a socket. */
export function handle(ctx: ApiContext, req: ApiRequest): ApiResponse {
  if (req.method === 'OPTIONS') {
    return {
      status: 204,
      headers: { ...BASE_HEADERS, 'Access-Control-Allow-Methods': 'GET, HEAD, OPTIONS', 'Access-Control-Max-Age': '86400' },
      body: '',
    }
  }
  if (req.method !== 'GET' && req.method !== 'HEAD') {
    return json(405, { error: 'method not allowed' }, { Allow: 'GET, HEAD, OPTIONS' })
  }
  let url: URL
  try {
    url = new URL(req.url, 'http://indexer.invalid')
  } catch {
    return json(400, { error: 'bad request' })
  }
  if (url.pathname !== '/healthz' && url.pathname !== '/readyz') {
    const client = clientKey(header(req, 'cf-connecting-ip') ?? req.remoteAddress ?? 'unknown')
    if (!ctx.limiter.take(client)) {
      return json(429, { error: 'rate limited' }, { 'Retry-After': String(Math.max(1, ctx.limiter.retryAfter(client))), 'Cache-Control': 'no-store' })
    }
  }
  // Canonical spellings only: another spelling of the same query gets a cacheable 301 to the
  // canonical URL (after the rate limit, so redirects cannot be used to probe for free).
  if (url.pathname !== '/healthz' && url.pathname !== '/readyz') {
    const path = canonicalPath(url.pathname)
    const query = canonicalQuery(url.searchParams)
    if (path !== url.pathname || query !== url.searchParams.toString()) {
      return {
        status: 301,
        headers: { ...BASE_HEADERS, Location: `${path}${query ? `?${query}` : ''}`, 'Cache-Control': 'public, max-age=86400' },
        body: '',
      }
    }
  }
  try {
    const res = route(ctx, req, url)
    return req.method === 'HEAD' ? { ...res, body: '' } : res
  } catch (e) {
    if (e instanceof HttpError) {
      return json(e.status, { error: e.message }, { 'Cache-Control': e.status === 404 ? 'public, max-age=60' : 'no-store' })
    }
    throw e
  }
}
