import { DatabaseSync } from 'node:sqlite'
import { randomBytes } from 'node:crypto'

/**
 * Thrown by {@link Store.bindPackages} when a stream was indexed under a different package. Clearing
 * it would discard history the full node may already have pruned, so it needs an explicit
 * confirmation (`REINDEX_CONFIRM=<stream>`), and a backup is taken first.
 */
export class PackageChangedError extends Error {
  constructor(readonly streams: Stream[]) {
    super(
      `the package indexed by ${streams.join(', ')} changed (or its rows predate package binding). ` +
        `Re-indexing deletes the stored history, which the full node may have pruned. ` +
        `Set REINDEX_CONFIRM=${streams.join(',')} to back up the database and re-index from the start.`,
    )
    this.name = 'PackageChangedError'
  }
}

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
-- Counters maintained inside the ingest transaction, so serving a page never scans a stream.
CREATE TABLE IF NOT EXISTS stream_stats (
  stream TEXT PRIMARY KEY,
  from_checkpoint INTEGER,
  latest_checkpoint INTEGER,
  events INTEGER NOT NULL DEFAULT 0,
  undecodable INTEGER NOT NULL DEFAULT 0,
  gaps INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE TABLE IF NOT EXISTS gate_stats (
  gate_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  n INTEGER NOT NULL,
  PRIMARY KEY (gate_id, kind)
) STRICT;
CREATE TABLE IF NOT EXISTS gate_commission (gate_id TEXT PRIMARY KEY, total TEXT NOT NULL) STRICT;
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
    this.backfillStats()
  }

  /** Build the counters from existing rows once (databases created before they existed). */
  private backfillStats(): void {
    const done = this.db.prepare(`SELECT value FROM meta WHERE key = 'stats:v2'`).get()
    if (done) return
    this.db.exec('BEGIN')
    try {
      this.db.exec(`
        DELETE FROM stream_stats; DELETE FROM gate_stats; DELETE FROM gate_commission;
        INSERT INTO stream_stats (stream, from_checkpoint, latest_checkpoint, events)
          SELECT stream, MIN(checkpoint), MAX(checkpoint), COUNT(*) FROM events GROUP BY stream;
        INSERT INTO gate_stats (gate_id, kind, n)
          SELECT gate_id, kind, COUNT(*) FROM events WHERE stream = 'access-gate' AND gate_id IS NOT NULL GROUP BY gate_id, kind;
      `)
      const stmt = this.db.prepare(
        `SELECT gate_id AS g, commission_mist AS c FROM events WHERE stream = 'access-gate' AND gate_id IS NOT NULL AND commission_mist IS NOT NULL`,
      )
      stmt.setReadBigInts(true)
      const totals = new Map<string, bigint>()
      for (const r of stmt.iterate() as Iterable<{ g: string; c: bigint }>) totals.set(r.g, (totals.get(r.g) ?? 0n) + r.c)
      const put = this.db.prepare('INSERT INTO gate_commission (gate_id, total) VALUES (?, ?)')
      for (const [g, total] of totals) put.run(g, total.toString())
      this.db.prepare(`INSERT INTO meta (key, value) VALUES ('stats:v2', '1')`).run()
      this.db.exec('COMMIT')
    } catch (err) {
      this.db.exec('ROLLBACK')
      throw err
    }
  }

  /** Random per index build; API cursors carry it, and a cursor from another generation is refused. */
  get generation(): string {
    return this.gen
  }

  /**
   * Bind each stream to the package (original id) it indexes. A stream recorded under a different
   * package — or holding rows from before packages were recorded — would have to be cleared to
   * re-read its new package. That discards history the full node may already have pruned, so it is
   * refused with {@link PackageChangedError} unless the stream is listed in `confirm`; confirming
   * first copies the database to `backupPath` (`VACUUM INTO`), then clears the stream's rows, cursor
   * and counters and rotates the generation (so cursors served from the old index are refused).
   * Call once at startup, before ingesting.
   *
   * @returns the streams that were cleared.
   */
  bindPackages(
    packages: Readonly<Record<Stream, string>>,
    opts: { confirm?: readonly Stream[]; backupPath?: string } = {},
  ): Stream[] {
    const get = this.db.prepare('SELECT value FROM meta WHERE key = ?')
    const rows = this.db.prepare('SELECT COUNT(*) AS n FROM events WHERE stream = ?')
    const pending: Stream[] = []
    for (const [stream, pkg] of Object.entries(packages) as [Stream, string][]) {
      const recorded = (get.get(`package:${stream}`) as { value: string } | undefined)?.value
      if (recorded === pkg) continue
      const hasRows = Number((rows.get(stream) as { n: number }).n) > 0
      if (recorded !== undefined || hasRows || this.cursor(stream) !== null) pending.push(stream)
    }
    const confirmed = pending.filter((s) => opts.confirm?.includes(s))
    if (pending.length !== confirmed.length) throw new PackageChangedError(pending.filter((s) => !confirmed.includes(s)))
    if (confirmed.length > 0 && opts.backupPath) this.db.exec(`VACUUM INTO '${opts.backupPath.replace(/'/g, "''")}'`)

    const set = this.db.prepare('INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value')
    this.db.exec('BEGIN')
    try {
      for (const stream of confirmed) {
        this.db.prepare('DELETE FROM events WHERE stream = ?').run(stream)
        this.db.prepare('DELETE FROM cursors WHERE stream = ?').run(stream)
        this.db.prepare('DELETE FROM stream_stats WHERE stream = ?').run(stream)
        if (stream === 'access-gate') this.db.exec('DELETE FROM gate_stats; DELETE FROM gate_commission;')
      }
      for (const [stream, pkg] of Object.entries(packages) as [Stream, string][]) set.run(`package:${stream}`, pkg)
      if (confirmed.length > 0) {
        this.gen = randomBytes(6).toString('hex')
        set.run('generation', this.gen)
      }
      this.db.exec('COMMIT')
    } catch (err) {
      this.db.exec('ROLLBACK')
      throw err
    }
    return confirmed
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
    const bumpStream = this.db.prepare(
      `INSERT INTO stream_stats (stream, from_checkpoint, latest_checkpoint, events) VALUES (?, ?, ?, 1)
       ON CONFLICT (stream) DO UPDATE SET
         from_checkpoint = CASE WHEN excluded.from_checkpoint IS NULL THEN from_checkpoint
                                WHEN from_checkpoint IS NULL OR excluded.from_checkpoint < from_checkpoint THEN excluded.from_checkpoint
                                ELSE from_checkpoint END,
         latest_checkpoint = CASE WHEN excluded.latest_checkpoint IS NULL THEN latest_checkpoint
                                  WHEN latest_checkpoint IS NULL OR excluded.latest_checkpoint > latest_checkpoint THEN excluded.latest_checkpoint
                                  ELSE latest_checkpoint END,
         events = events + 1`,
    )
    const bumpGate = this.db.prepare(
      `INSERT INTO gate_stats (gate_id, kind, n) VALUES (?, ?, 1) ON CONFLICT (gate_id, kind) DO UPDATE SET n = n + 1`,
    )
    const readCommission = this.db.prepare('SELECT total FROM gate_commission WHERE gate_id = ?')
    const writeCommission = this.db.prepare(
      'INSERT INTO gate_commission (gate_id, total) VALUES (?, ?) ON CONFLICT (gate_id) DO UPDATE SET total = excluded.total',
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
        if (Number(res.changes) === 0) continue // already stored: counters were bumped then
        added++
        const ckpt = e.checkpoint === null ? null : Number(e.checkpoint)
        bumpStream.run(stream, ckpt, ckpt)
        if (stream === 'access-gate' && e.gateId !== null) {
          bumpGate.run(e.gateId, e.kind)
          if (e.commissionMist !== null) {
            const cur = (readCommission.get(e.gateId) as { total: string } | undefined)?.total
            writeCommission.run(e.gateId, ((cur ? BigInt(cur) : 0n) + e.commissionMist).toString())
          }
        }
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

  /** Oldest and newest indexed checkpoints and the event count of `stream` (maintained at ingest, O(1)). */
  coverage(stream: Stream): { fromCheckpoint: string | null; latestCheckpoint: string | null; events: number } {
    const row = this.db
      .prepare('SELECT from_checkpoint AS lo, latest_checkpoint AS hi, events AS n FROM stream_stats WHERE stream = ?')
      .get(stream) as { lo: number | null; hi: number | null; n: number } | undefined
    return {
      fromCheckpoint: row?.lo == null ? null : String(row.lo),
      latestCheckpoint: row?.hi == null ? null : String(row.hi),
      events: row?.n ?? 0,
    }
  }

  /** Events the stream's parser could not decode (skipped and counted), and cursor resets after pruning. */
  health(stream: Stream): { undecodable: number; gaps: number } {
    const row = this.db.prepare('SELECT undecodable, gaps FROM stream_stats WHERE stream = ?').get(stream) as
      | { undecodable: number; gaps: number }
      | undefined
    return { undecodable: row?.undecodable ?? 0, gaps: row?.gaps ?? 0 }
  }

  /** Record `n` events skipped because they did not decode (the page still advances past them). */
  noteUndecodable(stream: Stream, n: number): void {
    this.db
      .prepare(
        `INSERT INTO stream_stats (stream, undecodable) VALUES (?, ?)
         ON CONFLICT (stream) DO UPDATE SET undecodable = undecodable + excluded.undecodable`,
      )
      .run(stream, n)
  }

  /**
   * The stored cursor points below what the full node still serves (its history was pruned while no
   * event arrived to move the cursor). The events in between are gone for good, so restart from the
   * node's earliest data: rows already stored are untouched (re-inserting is a no-op). Counted as a gap.
   */
  resetCursorAfterGap(stream: Stream, reason: string): void {
    this.db.exec('BEGIN')
    try {
      this.db.prepare('DELETE FROM cursors WHERE stream = ?').run(stream)
      this.db
        .prepare(
          `INSERT INTO stream_stats (stream, gaps) VALUES (?, 1)
           ON CONFLICT (stream) DO UPDATE SET gaps = gaps + 1`,
        )
        .run(stream)
      this.db
        .prepare('INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value')
        .run(`gap:${stream}`, JSON.stringify({ at: new Date().toISOString(), reason: reason.slice(0, 200) }))
      this.db.exec('COMMIT')
    } catch (err) {
      this.db.exec('ROLLBACK')
      throw err
    }
  }

  /** Per-gate counts by kind and the commission its mints paid (maintained at ingest). */
  gateStats(gateId: string): { counts: Record<string, number>; commissionMist: bigint } {
    const counts: Record<string, number> = {}
    for (const r of this.db.prepare('SELECT kind, n FROM gate_stats WHERE gate_id = ?').all(gateId) as { kind: string; n: number }[]) {
      counts[r.kind] = r.n
    }
    const total = (this.db.prepare('SELECT total FROM gate_commission WHERE gate_id = ?').get(gateId) as { total: string } | undefined)?.total
    return { counts, commissionMist: total ? BigInt(total) : 0n }
  }

  close(): void {
    this.db.close()
  }
}
