import { normalizeSuiAddress } from '@mysten/sui/utils'
import { eventGateId, parseAccessGateEvent } from '@meddleware/access-gate-client'
import type { CoreEventEntry } from '@meddleware/access-gate-client'
import { parseSealedContentEvent, sealedContentEventType } from '@meddleware/seal-client'
import type { IndexedEvent, Store, Stream } from './store.js'

/** The core-API slice this indexer reads (`SuiGrpcClient`). */
export interface EventSource {
  core: {
    listEvents(options: {
      filter: { eventType: string }
      limit?: number
      after?: string | null
      order?: 'ascending' | 'descending'
      signal?: AbortSignal
    }): Promise<{ events: CoreEventEntry[]; hasNextPage: boolean; endCursor: string | null }>
  }
}

export interface IngestOptions {
  accessGateOriginalId: string
  sealOriginalId: string
  /** Pages read per stream per poll, so one busy stream cannot starve the other (default 20). */
  maxPagesPerPoll?: number
  /** Per-request timeout for the full node, so a hung call fails the poll instead of stalling it (default 30 s). */
  requestTimeoutMs?: number
  log?: (message: string) => void
}

/** The event-type filter each stream polls: the whole `access_gate` module, or the one event. */
export function streamFilter(stream: Stream, opts: IngestOptions): string {
  return stream === 'access-gate'
    ? `${normalizeSuiAddress(opts.accessGateOriginalId)}::access_gate`
    : sealedContentEventType(opts.sealOriginalId)
}

/**
 * Decode an entry with the published client parsers — the same code the apps use — and keep the
 * columns queries need. `null` for an entry the parser does not recognise.
 *
 * @throws {Error} if a recognised event's BCS does not decode.
 */
export function toIndexed(stream: Stream, entry: CoreEventEntry, opts: IngestOptions): IndexedEvent | null {
  const bcs = typeof entry.bcs === 'string' ? Uint8Array.from(Buffer.from(entry.bcs, 'base64')) : entry.bcs
  const base = {
    txDigest: entry.transactionDigest,
    eventIndex: entry.eventIndex,
    checkpoint: entry.checkpoint,
    sender: normalizeSuiAddress(entry.sender),
    eventType: entry.eventType,
    bcs,
  }
  if (stream === 'access-gate') {
    const event = parseAccessGateEvent(entry, opts.accessGateOriginalId)
    if (!event) return null
    return {
      ...base,
      kind: event.kind,
      gateId: eventGateId(event),
      commissionMist: event.kind === 'AccessMinted' ? event.commissionMist : null,
    }
  }
  const pointer = parseSealedContentEvent(entry, opts.sealOriginalId)
  if (!pointer) return null
  return { ...base, kind: 'SealedContentPublished', gateId: pointer.gateId, commissionMist: null }
}

/** What the full node says when a cursor falls below the data it still serves (history pruned). */
const PRUNED = /below earliest available|earliest available|has been pruned/i

/** The streams this indexer keeps, in poll order. */
const STREAMS = ['access-gate', 'sealed-content'] as const

/** Per-stream poll state, for `/healthz` and the head pages. */
export interface StreamHealth {
  /** Epoch ms of the last poll of this stream that completed (0 before the first). */
  lastSuccessAt: number
  /** The error of the last poll, or null after a success. */
  lastError: string | null
}

/**
 * Polls each stream forward from its stored cursor (ascending, so row ids follow ledger order) and
 * stores every recognised event with the cursor, atomically per page. The streams are polled
 * independently: one failing never stops the other. An event that does not decode is counted and
 * skipped (the page advances past it), and a cursor that fell below the node's retention restarts
 * from the node's earliest data instead of retrying forever.
 */
export class Ingestor {
  private timer: NodeJS.Timeout | undefined
  private stopped = false
  private readonly state: Record<Stream, StreamHealth> = {
    'access-gate': { lastSuccessAt: 0, lastError: null },
    'sealed-content': { lastSuccessAt: 0, lastError: null },
  }

  constructor(
    private readonly store: Store,
    private readonly source: EventSource,
    private readonly opts: IngestOptions,
  ) {}

  /** Read one stream up to the page budget. Returns the number of new events. */
  async pollStream(stream: Stream): Promise<number> {
    const eventType = streamFilter(stream, this.opts)
    let added = 0
    let cursor = this.store.cursor(stream)
    for (let page = 0; page < (this.opts.maxPagesPerPoll ?? 20); page++) {
      const res = await this.source.core.listEvents({
        filter: { eventType },
        limit: 50,
        order: 'ascending',
        after: cursor,
        signal: AbortSignal.timeout(this.opts.requestTimeoutMs ?? 30_000),
      })
      const events: IndexedEvent[] = []
      let skipped = 0
      let undecodable = 0
      for (const entry of res.events) {
        try {
          const indexed = toIndexed(stream, entry, this.opts)
          if (indexed) events.push(indexed)
          else skipped++
        } catch (e) {
          // A recognised event that does not decode (parser/package version skew, a truncated
          // payload): do not let it stop the stream. Count it, log it loudly, and move past it.
          undecodable++
          this.opts.log?.(
            `${stream}: UNDECODABLE event ${entry.transactionDigest}#${entry.eventIndex}: ${e instanceof Error ? e.message : String(e)}`,
          )
        }
      }
      if (skipped) this.opts.log?.(`${stream}: skipped ${skipped} unrecognised event(s)`)
      // A null endCursor (nothing further) keeps the stored position.
      added += this.store.ingest(stream, events, res.endCursor)
      if (undecodable) this.store.noteUndecodable(stream, undecodable)
      if (res.endCursor) cursor = res.endCursor
      if (!res.hasNextPage) break
    }
    return added
  }

  /** Poll one stream, recovering from a pruned cursor, and record its health. */
  private async pollOne(stream: Stream): Promise<void> {
    try {
      let added: number
      try {
        added = await this.pollStream(stream)
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e)
        if (!PRUNED.test(message) || this.store.cursor(stream) === null) throw e
        // The events between the stored cursor and the node's retention are gone for good.
        this.opts.log?.(`${stream}: cursor is below the node's retention (${message}); restarting from its earliest data`)
        this.store.resetCursorAfterGap(stream, message)
        added = await this.pollStream(stream)
      }
      if (added) this.opts.log?.(`${stream}: +${added}`)
      this.state[stream] = { lastSuccessAt: Date.now(), lastError: null }
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e)
      this.state[stream] = { ...this.state[stream], lastError: message }
      this.opts.log?.(`${stream}: poll failed: ${message}`)
    }
  }

  /** Poll both streams once, independently. */
  async poll(): Promise<void> {
    for (const stream of STREAMS) await this.pollOne(stream)
  }

  /** Poll now and then every `intervalMs`; a failed poll is logged and retried next interval. */
  start(intervalMs: number): void {
    const run = async () => {
      try {
        await this.poll()
      } catch (e) {
        this.opts.log?.(`poll failed: ${e instanceof Error ? e.message : String(e)}`)
      }
      if (!this.stopped) this.timer = setTimeout(run, intervalMs)
    }
    void run()
  }

  stop(): void {
    this.stopped = true
    clearTimeout(this.timer)
  }

  /** Per-stream poll state. */
  health(stream: Stream): StreamHealth {
    return this.state[stream]
  }

  /** Epoch ms of the last poll in which EVERY stream succeeded (0 until then): the oldest stream's success. */
  get lastSuccessAt(): number {
    return Math.min(...STREAMS.map((s) => this.state[s].lastSuccessAt))
  }
}
