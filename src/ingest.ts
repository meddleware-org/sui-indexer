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

/**
 * Polls each stream forward from its stored cursor (ascending, so row ids follow ledger order) and
 * stores every recognised event with the cursor, atomically per page.
 */
export class Ingestor {
  private lastSuccess = 0
  private timer: NodeJS.Timeout | undefined
  private stopped = false

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
      for (const entry of res.events) {
        const indexed = toIndexed(stream, entry, this.opts)
        if (indexed) events.push(indexed)
        else skipped++
      }
      if (skipped) this.opts.log?.(`${stream}: skipped ${skipped} unrecognised event(s)`)
      // A null endCursor (nothing further) keeps the stored position.
      added += this.store.ingest(stream, events, res.endCursor)
      if (res.endCursor) cursor = res.endCursor
      if (!res.hasNextPage) break
    }
    return added
  }

  /** Poll both streams once. */
  async poll(): Promise<void> {
    for (const stream of ['access-gate', 'sealed-content'] as const) {
      const added = await this.pollStream(stream)
      if (added) this.opts.log?.(`${stream}: +${added}`)
    }
    this.lastSuccess = Date.now()
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

  /** Epoch ms of the last complete poll (0 before the first). */
  get lastSuccessAt(): number {
    return this.lastSuccess
  }
}
