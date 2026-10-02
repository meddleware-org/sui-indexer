import { DatabaseSync } from 'node:sqlite'
import { randomBytes } from 'node:crypto'

/** An event stream this indexer keeps. */
export type Stream = 'access-gate' | 'sealed-content'

/** One indexed event: the full node's entry verbatim, plus the columns queries filter on. */
export interface IndexedEvent {
  txDigest: string
  eventIndex: number
  checkpoint: string | null
  sender: string
  eventType: string
  bcs: Uint8Array
  /** Event kind (`AccessMinted`, …, or `SealedContentPublished`). */
  kind: string
  gateId: string | null
  /** Commission of an `AccessMinted` event (fits in 63 bits: at most 10% of a u64 price). */
  commissionMist: bigint | null
}

/** A stored row as served: the event entry the clients decode. */
export interface EventRow {
  id: number
  eventType: string
  sender: string
  bcs: Uint8Array
  checkpoint: string | null
  transactionDigest: string
  eventIndex: number
}

export interface PageQuery {
  kinds?: readonly string[]
  gateId?: string
  /** Only rows with a smaller id (older). */
  beforeId?: number
  limit: number
}

const SCHEMA = `
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE IF NOT EXISTS cursors (stream TEXT PRIMARY KEY, cursor TEXT) STRICT;
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY,
  stream TEXT NOT NULL,
  tx_digest TEXT NOT NULL,
  event_index INTEGER NOT NULL,
  checkpoint INTEGER,
  sender TEXT NOT NULL,
  event_type TEXT NOT NULL,
  bcs BLOB NOT NULL,
  kind TEXT NOT NULL,
  gate_id TEXT,
  commission_mist INTEGER,
  UNIQUE (tx_digest, event_index)
) STRICT;
CREATE INDEX IF NOT EXISTS events_stream ON events (stream, id);
CREATE INDEX IF NOT EXISTS events_gate ON events (stream, gate_id, id);
`

/**
 * SQLite storage (WAL). Rows get ascending ids in ingest order, and ingest reads the ledger in
 * ascending order, so `id` is ledger order: a page bounded by `beforeId` never changes once served.
 * `generation` is random per database, so cursors from a rebuilt database are refused.
 */
export class Store {
  readonly db: DatabaseSync
  private gen: string

  constructor(path: string) {
    this.db = new DatabaseSync(path)
    this.db.exec('PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;')
    this.db.exec(SCHEMA)
    const row = this.db.prepare(`SELECT value FROM meta WHERE key = 'generation'`).get() as { value: string } | undefined
    if (row) {
      this.gen = row.value
    } else {
      this.gen = randomBytes(6).toString('hex')
      this.db.prepare(`INSERT INTO meta (key, value) VALUES ('generation', ?)`).run(this.gen)
    }
  }

  /** Random per index build; API cursors carry it, and a cursor from another generation is refused. */
  get generation(): string {
    return this.gen
  }

  /**
   * Bind each stream to the package (original id) it indexes. A stream recorded under a different
   * package — or holding rows from before packages were recorded — is cleared (rows and cursor) so it
   * re-reads its new package from the start, and the generation rotates so cursors served from the
   * old index are refused. Call once at startup, before ingesting.
   *
   * @returns the streams that were cleared.
   */
  bindPackages(packages: Readonly<Record<Stream, string>>): Stream[] {
    const cleared: Stream[] = []
    const get = this.db.prepare('SELECT value FROM meta WHERE key = ?')
    const set = this.db.prepare('INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value')
    const rows = this.db.prepare('SELECT COUNT(*) AS n FROM events WHERE stream = ?')
    this.db.exec('BEGIN')
    try {
      for (const [stream, pkg] of Object.entries(packages) as [Stream, string][]) {
        const key = `package:${stream}`
        const recorded = (get.get(key) as { value: string } | undefined)?.value
        const hasRows = Number((rows.get(stream) as { n: number }).n) > 0
        if (recorded === pkg) continue
        if (recorded !== undefined || hasRows || this.cursor(stream) !== null) {
          this.db.prepare('DELETE FROM events WHERE stream = ?').run(stream)
          this.db.prepare('DELETE FROM cursors WHERE stream = ?').run(stream)
          cleared.push(stream)
        }
        set.run(key, pkg)
      }
      if (cleared.length > 0) {
        this.gen = randomBytes(6).toString('hex')
        set.run('generation', this.gen)
      }
      this.db.exec('COMMIT')
    } catch (err) {
      this.db.exec('ROLLBACK')
      throw err
    }
    return cleared
  }

  cursor(stream: Stream): string | null {
    const row = this.db.prepare('SELECT cursor FROM cursors WHERE stream = ?').get(stream) as { cursor: string | null } | undefined
    return row?.cursor ?? null
  }

  /**
   * Insert a page of events and advance the stream's cursor in one transaction, so a crash never
   * records a cursor past events it did not store. Re-inserting an event is a no-op.
   *
   * @returns how many events were new.
   */
  ingest(stream: Stream, events: readonly IndexedEvent[], cursor: string | null): number {
    const insert = this.db.prepare(
      `INSERT OR IGNORE INTO events
         (stream, tx_digest, event_index, checkpoint, sender, event_type, bcs, kind, gate_id, commission_mist)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    )
    const setCursor = this.db.prepare(
      'INSERT INTO cursors (stream, cursor) VALUES (?, ?) ON CONFLICT (stream) DO UPDATE SET cursor = excluded.cursor',
    )
    let added = 0
    this.db.exec('BEGIN')
    try {
      for (const e of events) {
        const res = insert.run(
          stream,
          e.txDigest,
          e.eventIndex,
          e.checkpoint === null ? null : Number(e.checkpoint),
          e.sender,
          e.eventType,
          e.bcs,
          e.kind,
          e.gateId,
          e.commissionMist,
        )
        added += Number(res.changes)
      }
      if (cursor !== null) setCursor.run(stream, cursor)
      this.db.exec('COMMIT')
    } catch (err) {
      this.db.exec('ROLLBACK')
      throw err
    }
    return added
  }

  /** Newest first. `next` is the id to pass as `beforeId` for older rows, or `null`. */
  page(stream: Stream, q: PageQuery): { rows: EventRow[]; next: number | null } {
    const where = ['stream = ?']
    const params: (string | number)[] = [stream]
    if (q.kinds?.length) {
      where.push(`kind IN (${q.kinds.map(() => '?').join(', ')})`)
      params.push(...q.kinds)
    }
    if (q.gateId !== undefined) {
      where.push('gate_id = ?')
      params.push(q.gateId)
    }
    if (q.beforeId !== undefined) {
      where.push('id < ?')
      params.push(q.beforeId)
    }
    const rows = this.db
      .prepare(
        `SELECT id, event_type, sender, bcs, checkpoint, tx_digest, event_index FROM events
         WHERE ${where.join(' AND ')} ORDER BY id DESC LIMIT ?`,
      )
      .all(...params, q.limit + 1) as {
      id: number
      event_type: string
      sender: string
      bcs: Uint8Array
      checkpoint: number | null
      tx_digest: string
      event_index: number
    }[]
    const more = rows.length > q.limit
    const page = rows.slice(0, q.limit).map((r) => ({
      id: r.id,
      eventType: r.event_type,
      sender: r.sender,
      bcs: r.bcs,
      checkpoint: r.checkpoint === null ? null : String(r.checkpoint),
      transactionDigest: r.tx_digest,
      eventIndex: r.event_index,
    }))
    return { rows: page, next: more ? page[page.length - 1].id : null }
  }

  /** The id of the newest row in `stream` (0 when empty) — part of the head page's ETag. */
  headId(stream: Stream): number {
    const row = this.db.prepare('SELECT MAX(id) AS id FROM events WHERE stream = ?').get(stream) as { id: number | null }
    return row.id ?? 0
  }

  /** Oldest and newest indexed checkpoints and the event count of `stream`. */
  coverage(stream: Stream): { fromCheckpoint: string | null; latestCheckpoint: string | null; events: number } {
    const row = this.db
      .prepare('SELECT MIN(checkpoint) AS lo, MAX(checkpoint) AS hi, COUNT(*) AS n FROM events WHERE stream = ?')
      .get(stream) as { lo: number | null; hi: number | null; n: number }
    return {
      fromCheckpoint: row.lo === null ? null : String(row.lo),
      latestCheckpoint: row.hi === null ? null : String(row.hi),
      events: row.n,
    }
  }

  /** Per-gate counts by kind and the commission its mints paid. */
  gateStats(gateId: string): { counts: Record<string, number>; commissionMist: bigint } {
    const counts: Record<string, number> = {}
    for (const r of this.db
      .prepare(`SELECT kind, COUNT(*) AS n FROM events WHERE stream = 'access-gate' AND gate_id = ? GROUP BY kind`)
      .all(gateId) as { kind: string; n: number }[]) {
      counts[r.kind] = r.n
    }
    // Summed as bigint: many commissions can exceed SQLite's 64-bit SUM.
    let commissionMist = 0n
    const stmt = this.db.prepare(
      `SELECT commission_mist AS c FROM events WHERE stream = 'access-gate' AND gate_id = ? AND commission_mist IS NOT NULL`,
    )
    stmt.setReadBigInts(true) // values above 2^53 must not round
    for (const r of stmt.iterate(gateId) as Iterable<{ c: bigint }>) commissionMist += r.c
    return { counts, commissionMist }
  }

  close(): void {
    this.db.close()
  }
}
