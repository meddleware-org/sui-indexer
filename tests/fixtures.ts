// Event fixtures encoded independently of the client parsers (layouts written from the Move structs).
import { bcs } from '@mysten/sui/bcs'
import { normalizeSuiAddress } from '@mysten/sui/utils'
import type { CoreEventEntry } from '@meddleware/access-gate-client'

export const AG = normalizeSuiAddress('0xa1')
export const SEAL = normalizeSuiAddress('0x5e')
export const GATE_A = normalizeSuiAddress('0xa')
export const GATE_B = normalizeSuiAddress('0xb')
const NFT = normalizeSuiAddress('0x11')
const ALICE = normalizeSuiAddress('0xa11ce')

const Minted = bcs.struct('AccessMintedEvent', {
  nft_id: bcs.Address,
  gate_id: bcs.Address,
  soulbound: bcs.bool(),
  initial_uses: bcs.u64(),
  recipient: bcs.Address,
  commission_mist: bcs.u64(),
  timestamp_ms: bcs.u64(),
})
const Consumed = bcs.struct('AccessConsumedEvent', {
  nft_id: bcs.Address,
  gate_id: bcs.Address,
  nonce: bcs.vector(bcs.u8()),
  consumer: bcs.Address,
  uses_after: bcs.u64(),
  timestamp_ms: bcs.u64(),
})
const Published = bcs.struct('SealedContentPublished', {
  content_id: bcs.Address,
  gate_id: bcs.Address,
  blob_id: bcs.string(),
  seal_id: bcs.string(),
  label: bcs.string(),
  publisher: bcs.Address,
})

let seq = 0
function entry(eventType: string, bytes: Uint8Array, checkpoint: number): CoreEventEntry {
  seq += 1
  return { eventType, sender: '0xa11ce', bcs: bytes, checkpoint: String(checkpoint), transactionDigest: `tx${seq}`, eventIndex: 0 }
}

export function minted(gate: string, checkpoint: number, commission: bigint | number = 2000): CoreEventEntry {
  return entry(
    `${AG}::access_gate::AccessMintedEvent`,
    Minted.serialize({ nft_id: NFT, gate_id: gate, soulbound: false, initial_uses: 3, recipient: ALICE, commission_mist: commission, timestamp_ms: 1 }).toBytes(),
    checkpoint,
  )
}

export function consumed(gate: string, checkpoint: number): CoreEventEntry {
  return entry(
    `${AG}::access_gate::AccessConsumedEvent`,
    Consumed.serialize({ nft_id: NFT, gate_id: gate, nonce: [...Buffer.from('nonce-123')], consumer: ALICE, uses_after: 2, timestamp_ms: 2 }).toBytes(),
    checkpoint,
  )
}

export function sealed(gate: string, checkpoint: number, label = 'Episode'): CoreEventEntry {
  return entry(
    `${SEAL}::sealed_content::SealedContentPublished`,
    Published.serialize({ content_id: NFT, gate_id: gate, blob_id: 'blob', seal_id: 'seal', label, publisher: ALICE }).toBytes(),
    checkpoint,
  )
}

/** A full node serving `pages[filter]` in ascending order (cursor = page index). */
export function fakeSource(pages: Record<string, CoreEventEntry[][]>) {
  const listEvents = async ({ filter, after }: { filter: { eventType: string }; after?: string | null }) => {
    const stream = pages[filter.eventType] ?? []
    const i = after ? Number(after) + 1 : 0
    const events = stream[i] ?? []
    const hasNextPage = i + 1 < stream.length
    return { events, hasNextPage, endCursor: events.length || hasNextPage ? String(i) : null }
  }
  return { core: { listEvents } }
}
