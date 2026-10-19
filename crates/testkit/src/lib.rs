//! Event fixtures for the sui-indexer tests and the CI client-contract job.
//!
//! The BCS bytes are encoded here, independently of the client parsers, from the Move structs
//! (`access-gate-sui/sources/access_gate.move`, `seal-policies-sui/sources/sealed_content.move`),
//! and the `json` renderings use the full node's form (field names of the Move struct, `ID` and
//! `address` as `0x` hex, `u64` as decimal strings). Test code only: the ingester never decodes BCS.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use sui_indexer_store::sui::normalize_sui_address;
use sui_indexer_store::{IndexedEvent, Store, Stream};

pub fn addr(s: &str) -> String {
    normalize_sui_address(s)
}

/// The fixtures' `access_gate` package.
pub fn ag() -> String {
    addr("0xa1")
}
/// The fixtures' `seal_policies` package.
pub fn seal() -> String {
    addr("0x5e")
}
pub fn gate_a() -> String {
    addr("0xa")
}
pub fn gate_b() -> String {
    addr("0xb")
}
fn nft() -> String {
    addr("0x11")
}
fn alice() -> String {
    addr("0xa11ce")
}

/// A full-node event entry (what `ListEvents` returns for one event).
#[derive(Clone, Debug, PartialEq)]
pub struct Fixture {
    pub event_type: String,
    /// As the node may send it (not normalised).
    pub sender: String,
    pub bcs: Vec<u8>,
    pub json: Value,
    pub checkpoint: u64,
    pub transaction_digest: String,
    pub event_index: u32,
    /// What the row's columns must be.
    pub kind: &'static str,
    pub gate_id: Option<String>,
    pub commission_mist: Option<i64>,
}

impl Fixture {
    /// The row the ingester must store for this entry.
    pub fn indexed(&self) -> IndexedEvent {
        IndexedEvent {
            tx_digest: self.transaction_digest.clone(),
            event_index: self.event_index,
            checkpoint: Some(self.checkpoint),
            sender: addr(&self.sender),
            event_type: self.event_type.clone(),
            bcs: self.bcs.clone(),
            kind: self.kind.to_owned(),
            gate_id: self.gate_id.clone(),
            commission_mist: self.commission_mist,
        }
    }
}

/// Minimal BCS writer for the fixtures.
#[derive(Default)]
pub struct Bcs(pub Vec<u8>);

impl Bcs {
    pub fn address(mut self, a: &str) -> Self {
        let hex = addr(a);
        for i in 0..32 {
            self.0.push(u8::from_str_radix(&hex[2 + 2 * i..4 + 2 * i], 16).expect("hex address"));
        }
        self
    }
    pub fn bool(mut self, b: bool) -> Self {
        self.0.push(u8::from(b));
        self
    }
    pub fn u64(mut self, v: u64) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn uleb(mut self, mut v: u64) -> Self {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(byte);
                return self;
            }
            self.0.push(byte | 0x80);
        }
    }
    pub fn bytes(self, b: &[u8]) -> Self {
        let mut s = self.uleb(b.len() as u64);
        s.0.extend_from_slice(b);
        s
    }
    pub fn string(self, s: &str) -> Self {
        self.bytes(s.as_bytes())
    }
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn next_digest() -> String {
    format!("tx{}", SEQ.fetch_add(1, Ordering::Relaxed) + 1)
}

/// `AccessMintedEvent` for `gate` (commission default 2000 in the TypeScript fixtures).
pub fn minted(gate: &str, checkpoint: u64, commission: u64) -> Fixture {
    let bcs =
        Bcs::default().address(&nft()).address(gate).bool(false).u64(3).address(&alice()).u64(commission).u64(1).0;
    Fixture {
        event_type: format!("{}::access_gate::AccessMintedEvent", ag()),
        sender: "0xa11ce".into(),
        bcs,
        json: json!({
            "nft_id": nft(), "gate_id": addr(gate), "soulbound": false, "initial_uses": "3",
            "recipient": alice(), "commission_mist": commission.to_string(), "timestamp_ms": "1"
        }),
        checkpoint,
        transaction_digest: next_digest(),
        event_index: 0,
        kind: "AccessMinted",
        gate_id: Some(addr(gate)),
        commission_mist: Some(i64::try_from(commission).expect("fixture commission fits 63 bits")),
    }
}

/// `AccessConsumedEvent` for `gate`.
pub fn consumed(gate: &str, checkpoint: u64) -> Fixture {
    let bcs = Bcs::default().address(&nft()).address(gate).bytes(b"nonce-123").address(&alice()).u64(2).u64(2).0;
    let nonce: Vec<Value> = b"nonce-123".iter().map(|b| json!(*b)).collect();
    Fixture {
        event_type: format!("{}::access_gate::AccessConsumedEvent", ag()),
        sender: "0xa11ce".into(),
        bcs,
        json: json!({
            "nft_id": nft(), "gate_id": addr(gate), "nonce": nonce, "consumer": alice(),
            "uses_after": "2", "timestamp_ms": "2"
        }),
        checkpoint,
        transaction_digest: next_digest(),
        event_index: 0,
        kind: "AccessConsumed",
        gate_id: Some(addr(gate)),
        commission_mist: None,
    }
}

/// `PlatformConfigUpdatedEvent` (concerns no gate).
pub fn platform_updated(checkpoint: u64) -> Fixture {
    let bcs = Bcs::default().address(&alice()).u64(20).u64(500_000).u64(1_000_000_000).0;
    Fixture {
        event_type: format!("{}::access_gate::PlatformConfigUpdatedEvent", ag()),
        sender: "0xa11ce".into(),
        bcs,
        json: json!({
            "treasury": alice(), "commission_bps": "20", "min_commission_mist": "500000",
            "free_gate_fee_mist": "1000000000"
        }),
        checkpoint,
        transaction_digest: next_digest(),
        event_index: 0,
        kind: "PlatformConfigUpdated",
        gate_id: None,
        commission_mist: None,
    }
}

/// `SealedContentPublished` for `gate`.
pub fn sealed(gate: &str, checkpoint: u64, label: &str) -> Fixture {
    let bcs =
        Bcs::default().address(&nft()).address(gate).string("blob").string("seal").string(label).address(&alice()).0;
    Fixture {
        event_type: format!("{}::sealed_content::SealedContentPublished", seal()),
        sender: "0xa11ce".into(),
        bcs,
        json: json!({
            "content_id": nft(), "gate_id": addr(gate), "blob_id": "blob", "seal_id": "seal",
            "label": label, "publisher": alice()
        }),
        checkpoint,
        transaction_digest: next_digest(),
        event_index: 0,
        kind: "SealedContentPublished",
        gate_id: Some(addr(gate)),
        commission_mist: None,
    }
}

/// The API tests' and the contract job's database: five `access_gate` events (gates A and B,
/// checkpoints 1–5) and three sealed-content pointers (labels `one`, `other`, `two`).
pub fn seed(store: &Store) {
    let ag: Vec<IndexedEvent> = [
        minted(&gate_a(), 1, 2000),
        consumed(&gate_a(), 2),
        minted(&gate_b(), 3, 2000),
        minted(&gate_a(), 4, 2000),
        consumed(&gate_b(), 5),
    ]
    .iter()
    .map(Fixture::indexed)
    .collect();
    store.ingest(Stream::AccessGate, &ag, Some("Yw==")).expect("seed access-gate");
    let sc: Vec<IndexedEvent> =
        [sealed(&gate_a(), 6, "one"), sealed(&gate_b(), 7, "other"), sealed(&gate_a(), 8, "two")]
            .iter()
            .map(Fixture::indexed)
            .collect();
    store.ingest(Stream::SealedContent, &sc, Some("cw==")).expect("seed sealed-content");
}
