//! The full node as an event source: `LedgerService.ListEvents` (server streaming) and
//! `GetServiceInfo`, through Mysten's `sui-rpc` client.
//!
//! [`PageBuilder`] turns the stream's frames into one page with exactly the semantics of
//! `SuiGrpcClient.core.listEvents` in `@mysten/sui` 2.36 (which the TypeScript indexer used), so the
//! cursor stored in the database — base64 of the watermark cursor bytes — is the same value either
//! implementation would store, and a database written by one continues under the other.

use std::future::Future;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sui_rpc::proto::sui::rpc::v2::{
    EventFilter, EventLiteral, EventTerm, EventTypeFilter, GetServiceInfoRequest, ListEventsRequest,
    ListEventsResponse, Ordering, QueryEndReason, QueryOptions, event_literal,
};

/// The read mask of `core.listEvents` (`json` included: the ingester reads `gate_id` and
/// `commission_mist` from it).
pub const READ_MASK: [&str; 9] = [
    "package_id",
    "module",
    "sender",
    "event_type",
    "contents",
    "json",
    "checkpoint",
    "transaction_digest",
    "event_index",
];

/// Events per page (the public full nodes cap `listEvents` pages at 50).
pub const PAGE_SIZE: u32 = 50;

/// One event as the full node returned it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawEvent {
    pub event_type: String,
    pub sender: String,
    /// The event's BCS bytes, stored unchanged.
    pub bcs: Vec<u8>,
    /// The node's JSON rendering of the event (`None` when the node sent none).
    pub json: Option<serde_json::Value>,
    pub checkpoint: Option<u64>,
    pub transaction_digest: Option<String>,
    pub event_index: Option<u32>,
}

/// One page of a listing, ascending.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourcePage {
    pub events: Vec<RawEvent>,
    pub has_next_page: bool,
    /// Where the next page starts (base64 watermark cursor); `None` keeps the stored position.
    pub end_cursor: Option<String>,
}

/// An ascending listing request: events whose type matches `event_type` (`pkg::module` or a full
/// type), strictly after `after`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRequest {
    pub event_type: String,
    pub after: Option<String>,
    pub limit: u32,
}

/// The slice of the full node this indexer reads. Errors are the node's messages (matched against
/// the pruned-cursor wording).
pub trait EventSource {
    fn list_events(&self, req: ListRequest) -> impl Future<Output = Result<SourcePage, String>>;
    /// The node's chain identifier: the base58 digest of the genesis checkpoint.
    fn chain_identifier(&self) -> impl Future<Output = Result<String, String>>;
}

/// One stream frame, reduced to what the page logic reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub event: Option<RawEvent>,
    /// `watermark.cursor` when present.
    pub cursor: Option<Vec<u8>>,
    /// `end.reason` when this is the terminal frame (`Some(None)`: an end without a reason).
    pub end: Option<Option<i32>>,
}

/// Collects one page from a `ListEvents` stream requested with `limit + 1` items.
#[derive(Debug)]
pub struct PageBuilder {
    limit: usize,
    events: Vec<RawEvent>,
    end_cursor: Option<String>,
    frontier: Option<String>,
    end: Option<Option<i32>>,
    saw_lookahead: bool,
}

impl PageBuilder {
    pub fn new(limit: u32) -> PageBuilder {
        PageBuilder {
            limit: limit as usize,
            events: vec![],
            end_cursor: None,
            frontier: None,
            end: None,
            saw_lookahead: false,
        }
    }

    pub fn push(&mut self, frame: Frame) {
        if let Some(c) = frame.cursor {
            self.frontier = Some(B64.encode(c));
        }
        if let Some(event) = frame.event {
            if self.events.len() >= self.limit {
                self.saw_lookahead = true;
            } else {
                self.end_cursor = self.frontier.clone();
                self.events.push(event);
            }
        }
        if let Some(end) = frame.end {
            self.end = Some(end);
        }
    }

    pub fn finish(self) -> SourcePage {
        let reason = self.end.flatten();
        let has_next_page = self.saw_lookahead
            || reason == Some(QueryEndReason::ScanLimit as i32)
            || reason == Some(QueryEndReason::ItemLimit as i32);
        let end_cursor = if self.saw_lookahead {
            self.end_cursor
        } else if has_next_page {
            self.frontier.or(self.end_cursor)
        } else {
            self.end_cursor
        };
        SourcePage { events: self.events, has_next_page, end_cursor }
    }
}

/// `google.protobuf.Value` → JSON, as protobuf's canonical JSON mapping (`Value.toJson`) does.
pub fn value_to_json(v: &prost_types::Value) -> serde_json::Value {
    use prost_types::value::Kind;
    match &v.kind {
        None | Some(Kind::NullValue(_)) => serde_json::Value::Null,
        // An integral number prints as JavaScript prints it (`110`, not `110.0`).
        Some(Kind::NumberValue(n)) if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0 => {
            serde_json::Value::Number(serde_json::Number::from(*n as i64))
        }
        Some(Kind::NumberValue(n)) => {
            serde_json::Number::from_f64(*n).map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        Some(Kind::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Kind::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Kind::StructValue(s)) => {
            serde_json::Value::Object(s.fields.iter().map(|(k, v)| (k.clone(), value_to_json(v))).collect())
        }
        Some(Kind::ListValue(l)) => serde_json::Value::Array(l.values.iter().map(value_to_json).collect()),
    }
}

/// A `ListEventsResponse` frame → [`Frame`].
pub fn frame_of(r: ListEventsResponse) -> Frame {
    Frame {
        cursor: r.watermark.and_then(|w| w.cursor).map(|c| c.to_vec()),
        end: r.end.map(|e| e.reason),
        event: r.event.map(|e| RawEvent {
            event_type: e.event_type.unwrap_or_default(),
            sender: e.sender.unwrap_or_default(),
            bcs: e.contents.and_then(|c| c.value).map(|v| v.to_vec()).unwrap_or_default(),
            json: e.json.as_deref().map(value_to_json),
            checkpoint: e.checkpoint,
            transaction_digest: e.transaction_digest,
            event_index: e.event_index,
        }),
    }
}

/// The `ListEventsRequest` for `req`: one `event_type` literal, ascending, `limit + 1` items (the
/// look-ahead item tells whether another page exists), resuming after the decoded cursor.
pub fn grpc_request(req: &ListRequest) -> Result<ListEventsRequest, String> {
    let after = match &req.after {
        Some(c) => Some(B64.decode(c).map_err(|e| format!("stored cursor is not base64: {e}"))?.into()),
        None => None,
    };
    let mut type_filter = EventTypeFilter::default();
    type_filter.event_type = Some(req.event_type.clone());
    let mut literal = EventLiteral::default();
    literal.predicate = Some(event_literal::Predicate::EventType(type_filter));
    let mut term = EventTerm::default();
    term.literals = vec![literal];
    let mut filter = EventFilter::default();
    filter.terms = vec![term];
    let mut options = QueryOptions::default();
    options.limit = Some(req.limit + 1);
    options.ordering = Some(Ordering::Ascending as i32);
    options.after = after;
    let mut r = ListEventsRequest::default();
    r.read_mask = Some(prost_types::FieldMask { paths: READ_MASK.iter().map(|p| (*p).to_owned()).collect() });
    r.filter = Some(filter);
    r.options = Some(options);
    Ok(r)
}

/// A gRPC status as the message the node sent (its code when it sent none), followed by the causes of
/// a transport error (`tcp connect error: Connection refused (os error 111)`).
pub fn status_message(s: &tonic::Status) -> String {
    let mut out = if s.message().is_empty() { format!("{:?}", s.code()) } else { s.message().to_owned() };
    let mut cause = std::error::Error::source(s);
    while let Some(c) = cause {
        let text = c.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        cause = c.source();
    }
    out
}

/// The full node over gRPC (`sui_rpc::Client`: TLS with the webpki roots, HTTP/2).
#[derive(Clone)]
pub struct GrpcSource {
    client: sui_rpc::Client,
}

impl GrpcSource {
    pub fn new(url: &str) -> Result<GrpcSource, String> {
        sui_rpc::Client::new(url.to_owned()).map(|client| GrpcSource { client }).map_err(|s| status_message(&s))
    }
}

impl EventSource for GrpcSource {
    async fn list_events(&self, req: ListRequest) -> Result<SourcePage, String> {
        let request = grpc_request(&req)?;
        let mut ledger = self.client.clone().ledger_client();
        let mut stream = ledger.list_events(request).await.map_err(|s| status_message(&s))?.into_inner();
        let mut page = PageBuilder::new(req.limit);
        while let Some(frame) = stream.message().await.map_err(|s| status_message(&s))? {
            page.push(frame_of(frame));
        }
        Ok(page.finish())
    }

    async fn chain_identifier(&self) -> Result<String, String> {
        let mut ledger = self.client.clone().ledger_client();
        let info = ledger.get_service_info(GetServiceInfoRequest::default()).await.map_err(|s| status_message(&s))?;
        info.into_inner().chain_id.ok_or_else(|| "the node reported no chain identifier".to_owned())
    }
}
