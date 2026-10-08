/**
 * Per-key token buckets in memory. It backs the Cloudflare edge limit for requests that reach the
 * origin (cache misses). Idle buckets are swept, so memory is bounded by the active clients.
 */
export class RateLimiter {
  private readonly buckets = new Map<string, { tokens: number; at: number }>()
  private lastSweep = 0

  constructor(
    private readonly perSec: number,
    private readonly burst: number,
    private readonly now: () => number = Date.now,
    /** Most buckets kept; the oldest are evicted past it, so memory stays bounded whatever the key churn. */
    private readonly maxBuckets = 50_000,
  ) {}

  /** Take one token for `key`; false when the bucket is empty. */
  take(key: string): boolean {
    const t = this.now()
    this.sweep(t)
    const b = this.buckets.get(key) ?? { tokens: this.burst, at: t }
    b.tokens = Math.min(this.burst, b.tokens + ((t - b.at) / 1000) * this.perSec)
    b.at = t
    const ok = b.tokens >= 1
    if (ok) b.tokens -= 1
    this.buckets.delete(key) // re-insert so Map order is least-recently-used first
    this.buckets.set(key, b)
    while (this.buckets.size > this.maxBuckets) this.buckets.delete(this.buckets.keys().next().value as string)
    return ok
  }

  /** Seconds until `key` has a token again (for `Retry-After`). */
  retryAfter(key: string): number {
    const b = this.buckets.get(key)
    if (!b || b.tokens >= 1) return 0
    return Math.ceil((1 - b.tokens) / this.perSec)
  }

  get size(): number {
    return this.buckets.size
  }

  private sweep(t: number): void {
    if (t - this.lastSweep < 60_000) return
    this.lastSweep = t
    const full = (this.burst / this.perSec) * 1000
    for (const [k, b] of this.buckets) if (t - b.at > full) this.buckets.delete(k)
  }
}
