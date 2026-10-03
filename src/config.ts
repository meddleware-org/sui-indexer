import { normalizeSuiAddress } from '@mysten/sui/utils'
import { accessGateDeployment } from '@meddleware/access-gate-client/deployments'
import { sealPoliciesDeployment } from '@meddleware/seal-client/deployments'

export interface Config {
  /** The one network this instance indexes (it is also the first path segment of the API). */
  network: string
  grpcUrl: string
  dbPath: string
  port: number
  pollMs: number
  accessGateOriginalId: string
  sealOriginalId: string
  /** Requests per second per client IP (token refill rate). */
  rateLimitPerSec: number
  /** Token-bucket capacity per client IP. */
  rateLimitBurst: number
}

const DEFAULT_GRPC: Record<string, string> = {
  testnet: 'https://fullnode.testnet.sui.io:443',
  mainnet: 'https://fullnode.mainnet.sui.io:443',
}

/** A package original-id override: `0x` + hex, normalised so the stored binding compares exactly. */
function packageId(env: NodeJS.ProcessEnv, name: string, fallback: () => string): string {
  const raw = env[name]
  if (raw === undefined || raw === '') return fallback()
  if (!/^0x[0-9a-fA-F]{1,64}$/.test(raw)) throw new Error(`${name} must be a 0x-prefixed hex package id, got ${raw}`)
  return normalizeSuiAddress(raw)
}

function int(env: NodeJS.ProcessEnv, name: string, fallback: number, min: number, max: number): number {
  const raw = env[name]
  if (raw === undefined || raw === '') return fallback
  const v = Number(raw)
  if (!Number.isInteger(v) || v < min || v > max) throw new Error(`${name} must be an integer in [${min}, ${max}], got ${raw}`)
  return v
}

/**
 * Configuration from the environment. Package ids default to the recorded deployments of the
 * published client packages, so a normal deploy sets only `NETWORK`.
 *
 * @throws {Error} for an invalid value or a network without a recorded deployment.
 */
export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const network = env.NETWORK || 'testnet'
  const grpcUrl = env.GRPC_URL || DEFAULT_GRPC[network]
  if (!grpcUrl) throw new Error(`GRPC_URL is required for network ${network}`)
  const url = new URL(grpcUrl)
  if (url.protocol !== 'https:' && url.hostname !== 'localhost' && url.hostname !== '127.0.0.1') {
    throw new Error('GRPC_URL must use https')
  }
  return {
    network,
    grpcUrl,
    dbPath: env.DB_PATH || '/data/indexer.db',
    port: int(env, 'PORT', 8080, 1, 65535),
    pollMs: int(env, 'POLL_MS', 15_000, 1_000, 3_600_000),
    accessGateOriginalId: packageId(env, 'ACCESS_GATE_ORIGINAL_ID', () => accessGateDeployment(network).originalId),
    sealOriginalId: packageId(env, 'SEAL_ORIGINAL_ID', () => sealPoliciesDeployment(network).originalId),
    rateLimitPerSec: int(env, 'RATE_LIMIT_PER_SEC', 10, 1, 10_000),
    rateLimitBurst: int(env, 'RATE_LIMIT_BURST', 40, 1, 100_000),
  }
}
