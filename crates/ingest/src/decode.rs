//! Raw event → stored row, **without decoding BCS** ("one parser": the BCS layouts live only in
//! `@meddleware/access-gate-client` and `@meddleware/seal-client`, which decode every served row).
//!
//! - `kind` comes from the event type's struct name (the event must be typed at the stream's
//!   package original id and module, exactly as the client parsers require).
//! - `gate_id` and `commission_mist` come from the full node's `json` rendering of the event, whose
//!   field names are the Move struct's (see CLAUDE.md "JSON fields relied on").
//! - The BCS bytes are stored unchanged.

use sui_indexer_store::sui::{StructTag, is_hex_id, normalize_sui_address};
use sui_indexer_store::{IndexedEvent, Stream};

use crate::source::RawEvent;

/// `access_gate` event kind ↔ Move struct name (the `ACCESS_GATE_EVENT_KINDS` of the client).
pub const ACCESS_GATE_KINDS: [(&str, &str); 8] = [
    ("GateCreated", "GateCreatedEvent"),
    ("AccessMinted", "AccessMintedEvent"),
    ("AccessConsumed", "AccessConsumedEvent"),
    ("AccessBurned", "AccessBurnedEvent"),
    ("GateFrozen", "GateFrozenEvent"),
    ("GateMadeFree", "GateMadeFreeEvent"),
    ("PlatformConfigUpdated", "PlatformConfigUpdatedEvent"),
    ("PlatformMigrated", "PlatformMigratedEvent"),
];

/// Kinds that concern one gate (their struct has a `gate_id: ID` field).
const GATE_KINDS: [&str; 6] =
    ["GateCreated", "AccessMinted", "AccessConsumed", "AccessBurned", "GateFrozen", "GateMadeFree"];

/// The packages the streams index.
#[derive(Clone, Debug)]
pub struct Packages {
    pub access_gate_original_id: String,
    pub seal_original_id: String,
}

/// The event-type filter each stream polls: the whole `access_gate` module, or the one event.
pub fn stream_filter(stream: Stream, p: &Packages) -> String {
    match stream {
        Stream::AccessGate => format!("{}::access_gate", normalize_sui_address(&p.access_gate_original_id)),
        Stream::SealedContent => {
            format!("{}::sealed_content::SealedContentPublished", normalize_sui_address(&p.seal_original_id))
        }
    }
}

/// The kind of an event of `stream`, or `None` if it is not one of the stream's recognised events.
pub fn kind_of(stream: Stream, event_type: &str, p: &Packages) -> Option<(&'static str, StructTag)> {
    let tag = StructTag::parse(event_type)?;
    match stream {
        Stream::AccessGate => {
            if tag.address != normalize_sui_address(&p.access_gate_original_id) || tag.module != "access_gate" {
                return None;
            }
            ACCESS_GATE_KINDS.iter().find(|(_, s)| *s == tag.name).map(|(k, _)| (*k, tag))
        }
        Stream::SealedContent => (tag.address == normalize_sui_address(&p.seal_original_id)
            && tag.module == "sealed_content"
            && tag.name == "SealedContentPublished")
            .then_some(("SealedContentPublished", tag)),
    }
}

fn field<'a>(json: &'a serde_json::Value, name: &str) -> Result<&'a serde_json::Value, String> {
    json.get(name).ok_or_else(|| format!("json has no `{name}`"))
}

/// An `ID` field: the node renders it as a `0x` hex string.
fn id_field(json: &serde_json::Value, name: &str) -> Result<String, String> {
    match field(json, name)? {
        serde_json::Value::String(s) if is_hex_id(s) => Ok(normalize_sui_address(s)),
        v => Err(format!("json `{name}` is not an id: {v}")),
    }
}

/// A `u64` field: the node renders it as a decimal string (a JSON number is accepted when it is an
/// exact integer). Stored as INTEGER, so it must fit 63 bits (commission is at most 10% of a price).
fn u64_field(json: &serde_json::Value, name: &str) -> Result<i64, String> {
    let v = field(json, name)?;
    let n = match v {
        serde_json::Value::String(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => s.parse::<u64>().ok(),
        serde_json::Value::Number(n) => n.as_u64(),
        _ => None,
    };
    let n = n.ok_or_else(|| format!("json `{name}` is not a u64: {v}"))?;
    i64::try_from(n).map_err(|_| format!("json `{name}` = {n} exceeds 63 bits"))
}

/// The row of a recognised event; `Ok(None)` for an event the stream does not recognise (another
/// package's, or an unknown struct).
///
/// # Errors
/// A recognised event the ingester cannot store: no BCS, no position, or a `json` rendering
/// without the expected field. It is counted as undecodable and skipped.
pub fn to_indexed(stream: Stream, e: &RawEvent, p: &Packages) -> Result<Option<IndexedEvent>, String> {
    let Some((kind, tag)) = kind_of(stream, &e.event_type, p) else { return Ok(None) };
    let tx_digest = e.transaction_digest.clone().filter(|d| !d.is_empty()).ok_or("no transaction digest")?;
    let event_index = e.event_index.ok_or("no event index")?;
    if e.bcs.is_empty() {
        return Err("no BCS contents".into());
    }
    let needs_gate = stream == Stream::SealedContent || GATE_KINDS.contains(&kind);
    let needs_json = needs_gate || kind == "AccessMinted";
    let json = match &e.json {
        Some(j @ serde_json::Value::Object(_)) => Some(j),
        _ if needs_json => return Err("no json rendering".into()),
        _ => None,
    };
    let gate_id = match json {
        Some(j) if needs_gate => Some(id_field(j, "gate_id")?),
        _ => None,
    };
    let commission_mist = match json {
        Some(j) if kind == "AccessMinted" => Some(u64_field(j, "commission_mist")?),
        _ => None,
    };
    Ok(Some(IndexedEvent {
        tx_digest,
        event_index,
        checkpoint: e.checkpoint,
        sender: normalize_sui_address(&e.sender),
        event_type: tag.to_string(),
        bcs: e.bcs.clone(),
        kind: kind.to_owned(),
        gate_id,
        commission_mist,
    }))
}
