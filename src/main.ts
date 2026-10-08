import { createServer } from 'node:http'
import { SuiGrpcClient } from '@mysten/sui/grpc'
import { loadConfig } from './config.js'
import { PackageChangedError, Store, type Stream } from './store.js'
import { Ingestor, type EventSource } from './ingest.js'
import { RateLimiter } from './ratelimit.js'
import { handle } from './api.js'
import { KNOWN_CHAIN_IDS, shortChainId } from './chain.js'

const log = (message: string) => console.log(`${new Date().toISOString()} ${message}`)

const config = loadConfig()
const store = new Store(config.dbPath)
let cleared: Stream[]
try {
  cleared = store.bindPackages(
    { 'access-gate': config.accessGateOriginalId, 'sealed-content': config.sealOriginalId },
    { confirm: config.reindexConfirm, backupPath: `${config.dbPath}.bak-${Date.now()}` },
  )
} catch (e) {
  if (e instanceof PackageChangedError) {
    log(`refusing to start: ${e.message}`)
    process.exit(1)
  }
  throw e
}
if (cleared.length > 0) log(`package changed for ${cleared.join(', ')}: backed up, cleared and re-indexing from the start`)
const client = new SuiGrpcClient({ network: config.network as 'testnet', baseUrl: config.grpcUrl })
// The node must serve the network this instance is configured for: a mainnet node behind a testnet
// instance would index nothing and stay green.
const want = KNOWN_CHAIN_IDS[config.network]
if (want) {
  const { chainIdentifier } = await client.core.getChainIdentifier()
  const got = shortChainId(chainIdentifier)
  if (got !== want) {
    log(`refusing to start: ${config.grpcUrl} serves chain ${got}, not ${config.network} (${want})`)
    process.exit(1)
  }
}
// No cast: tsc checks the real client's listEvents against the slice this indexer relies on.
const source: EventSource = client
const ingestor = new Ingestor(store, source, {
  accessGateOriginalId: config.accessGateOriginalId,
  sealOriginalId: config.sealOriginalId,
  log,
})
const limiter = new RateLimiter(config.rateLimitPerSec, config.rateLimitBurst)

const server = createServer((req, res) => {
  try {
    const out = handle(
      { store, network: config.network, limiter, health: (s) => ingestor.health(s), pollMs: config.pollMs },
      { method: req.method ?? 'GET', url: req.url ?? '/', headers: req.headers, remoteAddress: req.socket.remoteAddress },
    )
    res.writeHead(out.status, out.headers).end(out.body)
  } catch (e) {
    log(`request failed: ${e instanceof Error ? e.stack : String(e)}`)
    res.writeHead(500, { 'Content-Type': 'application/json' }).end('{"error":"internal error"}')
  }
})
server.requestTimeout = 10_000
server.headersTimeout = 5_000

server.listen(config.port, () => {
  log(
    `sui-indexer ${config.network} on :${config.port} — access_gate ${config.accessGateOriginalId}, ` +
      `seal_policies ${config.sealOriginalId}, generation ${store.generation}`,
  )
  ingestor.start(config.pollMs)
})

function shutdown(signal: string): void {
  log(`${signal}: shutting down`)
  ingestor.stop()
  server.close(() => {
    store.close()
    process.exit(0)
  })
  setTimeout(() => process.exit(0), 5_000).unref()
}
process.on('SIGTERM', () => shutdown('SIGTERM'))
process.on('SIGINT', () => shutdown('SIGINT'))
