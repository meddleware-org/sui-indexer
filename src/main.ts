import { createServer } from 'node:http'
import { SuiGrpcClient } from '@mysten/sui/grpc'
import { loadConfig } from './config.js'
import { Store } from './store.js'
import { Ingestor, type EventSource } from './ingest.js'
import { RateLimiter } from './ratelimit.js'
import { handle } from './api.js'

const log = (message: string) => console.log(`${new Date().toISOString()} ${message}`)

const config = loadConfig()
const store = new Store(config.dbPath)
const cleared = store.bindPackages({
  'access-gate': config.accessGateOriginalId,
  'sealed-content': config.sealOriginalId,
})
if (cleared.length > 0) log(`package changed for ${cleared.join(', ')}: cleared and re-indexing from the start`)
const client = new SuiGrpcClient({ network: config.network as 'testnet', baseUrl: config.grpcUrl })
const ingestor = new Ingestor(store, client as unknown as EventSource, {
  accessGateOriginalId: config.accessGateOriginalId,
  sealOriginalId: config.sealOriginalId,
  log,
})
const limiter = new RateLimiter(config.rateLimitPerSec, config.rateLimitBurst)

const server = createServer((req, res) => {
  try {
    const out = handle(
      { store, network: config.network, limiter, lastPollAt: () => ingestor.lastSuccessAt, pollMs: config.pollMs },
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
