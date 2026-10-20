//! Ingest tests: decoding without BCS, the poll loop against a scripted full node, page assembly
//! from stream frames, the real gRPC client against a local LedgerService stub, and the startup
//! wiring (`start`, `run_loop`, `healthcheck`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use proptest::prelude::*;
use serde_json::json;
use sui_indexer_store::testdb::TestDb;
use sui_indexer_store::{ALIVE_KEY, DatabaseSettings, Stream, require_db};
use sui_indexer_testkit::{self as kit, Fixture, addr, consumed, gate_a, gate_b, minted, platform_updated, sealed};

use super::*;
use crate::decode::{Packages, kind_of, stream_filter, to_indexed};
use crate::ingestor::{IngestOptions, Ingestor, is_pruned};
use crate::source::{EventSource, Frame, ListRequest, PageBuilder, RawEvent, SourcePage, grpc_request};

const TESTNET_GENESIS: &str = "69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD";
const MAINNET_GENESIS: &str = "4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S";

fn packages() -> Packages {
    Packages { access_gate_original_id: kit::ag(), seal_original_id: kit::seal() }
}

fn ag_filter() -> String {
    format!("{}::access_gate", kit::ag())
}

fn seal_filter() -> String {
    format!("{}::sealed_content::SealedContentPublished", kit::seal())
}

fn raw(f: &Fixture) -> RawEvent {
    RawEvent {
        event_type: f.event_type.clone(),
        sender: f.sender.clone(),
        bcs: f.bcs.clone(),
        json: Some(f.json.clone()),
        checkpoint: Some(f.checkpoint),
        transaction_digest: Some(f.transaction_digest.clone()),
        event_index: Some(f.event_index),
    }
}

type Handler = Box<dyn Fn(&ListRequest) -> Result<SourcePage, String>>;

/// A scripted full node.
struct Fake {
    handler: Handler,
    calls: RefCell<Vec<ListRequest>>,
    chain: Result<String, String>,
    hang: bool,
}

impl Fake {
    fn new(handler: Handler) -> Fake {
        Fake { handler, calls: RefCell::new(vec![]), chain: Ok(TESTNET_GENESIS.into()), hang: false }
    }
}

impl EventSource for Fake {
    async fn list_events(&self, req: ListRequest) -> Result<SourcePage, String> {
        self.calls.borrow_mut().push(req.clone());
        if self.hang {
            std::future::pending::<()>().await;
        }
        (self.handler)(&req)
    }
    async fn chain_identifier(&self) -> Result<String, String> {
        self.chain.clone()
    }
}

type Pages = Rc<RefCell<HashMap<String, Vec<Vec<RawEvent>>>>>;

/// A full node serving `pages[filter]` in ascending order (cursor = page index), like the
/// TypeScript tests' `fakeSource`.
fn paged(pages: Pages) -> Handler {
    Box::new(move |req| {
        let pages = pages.borrow();
        let stream = pages.get(&req.event_type).cloned().unwrap_or_default();
        let i = req.after.as_ref().map_or(0, |a| a.parse::<usize>().map_or(usize::MAX, |n| n + 1));
        let events = stream.get(i).cloned().unwrap_or_default();
        let has_next_page = i.saturating_add(1) < stream.len();
        let end_cursor = (!events.is_empty() || has_next_page).then(|| i.to_string());
        Ok(SourcePage { events, has_next_page, end_cursor })
    })
}

fn pages(entries: Vec<(String, Vec<Vec<RawEvent>>)>) -> Pages {
    Rc::new(RefCell::new(entries.into_iter().collect()))
}

type Logs = Rc<RefCell<Vec<String>>>;

async fn ingestor_with(db: &TestDb, source: Fake, opts: IngestOptions) -> (Ingestor<Fake>, Logs) {
    let logs: Logs = Rc::default();
    let sink = logs.clone();
    let log: Logger = Box::new(move |m| sink.borrow_mut().push(m.to_owned()));
    (Ingestor::new(db.store().await, source, opts, log).await, logs)
}

async fn ingestor(db: &TestDb, source: Fake) -> (Ingestor<Fake>, Logs) {
    ingestor_with(db, source, IngestOptions::new(packages())).await
}

// ── to_indexed ──────────────────────────────────────────────────────────────

#[test]
fn keeps_kind_gate_and_commission_from_the_json_rendering() {
    let p = packages();
    let m = to_indexed(Stream::AccessGate, &raw(&minted(&gate_a(), 5, 777)), &p).unwrap().unwrap();
    assert_eq!(
        (m.kind.as_str(), m.gate_id.clone(), m.commission_mist, m.checkpoint),
        ("AccessMinted", Some(gate_a()), Some(777), Some(5))
    );
    let c = to_indexed(Stream::AccessGate, &raw(&consumed(&gate_b(), 6)), &p).unwrap().unwrap();
    assert_eq!((c.kind.as_str(), c.gate_id.clone(), c.commission_mist), ("AccessConsumed", Some(gate_b()), None));
    let s = to_indexed(Stream::SealedContent, &raw(&sealed(&gate_a(), 7, "x")), &p).unwrap().unwrap();
    assert_eq!((s.kind.as_str(), s.gate_id.clone()), ("SealedContentPublished", Some(gate_a())));
    let u = to_indexed(Stream::AccessGate, &raw(&platform_updated(8)), &p).unwrap().unwrap();
    assert_eq!((u.kind.as_str(), u.gate_id.clone(), u.commission_mist), ("PlatformConfigUpdated", None, None));
    // Verbatim: the BCS bytes and the normalised sender and type.
    let f = minted(&gate_a(), 5, 777);
    let row = to_indexed(Stream::AccessGate, &raw(&f), &p).unwrap().unwrap();
    assert_eq!(row, f.indexed());
    assert_eq!(row.sender, addr("0xa11ce"));
}

#[test]
fn every_kind_maps_from_its_struct_name() {
    let p = packages();
    for (kind, name) in crate::decode::ACCESS_GATE_KINDS {
        let t = format!("{}::access_gate::{name}", kit::ag());
        assert_eq!(kind_of(Stream::AccessGate, &t, &p).map(|(k, _)| k), Some(kind));
        // The short form of the package address is the same type.
        assert_eq!(kind_of(Stream::AccessGate, &format!("0xa1::access_gate::{name}"), &p).map(|(k, _)| k), Some(kind));
    }
}

#[test]
fn drops_events_of_another_package_module_or_struct() {
    let p = packages();
    let mut foreign = raw(&minted(&gate_a(), 1, 1));
    foreign.event_type = "0xa1a1::access_gate::AccessMintedEvent".into();
    assert_eq!(to_indexed(Stream::AccessGate, &foreign, &p), Ok(None));
    for t in [
        format!("{}::other::AccessMintedEvent", kit::ag()),
        format!("{}::access_gate::Unknown", kit::ag()),
        format!("{}::access_gate::AccessMintedEvent<u64>", kit::ag()),
        format!("{}::sealed_content::SealedContentPublished", kit::ag()),
        "garbage".to_owned(),
    ] {
        let mut e = raw(&minted(&gate_a(), 1, 1));
        e.event_type = t.clone();
        assert_eq!(to_indexed(Stream::AccessGate, &e, &p), Ok(None), "{t}");
    }
    // A sealed-content event read on the access-gate stream (and the reverse) is not recognised.
    assert_eq!(to_indexed(Stream::AccessGate, &raw(&sealed(&gate_a(), 1, "x")), &p), Ok(None));
    assert_eq!(to_indexed(Stream::SealedContent, &raw(&minted(&gate_a(), 1, 1)), &p), Ok(None));
}

#[test]
fn a_recognised_event_without_what_the_row_needs_is_undecodable() {
    let p = packages();
    let base = raw(&minted(&gate_a(), 1, 1));
    type Mutation = Box<dyn Fn(&mut RawEvent)>;
    let cases: Vec<(&str, Mutation)> = vec![
        ("no json", Box::new(|e| e.json = None)),
        ("json not an object", Box::new(|e| e.json = Some(json!("x")))),
        ("no gate_id", Box::new(|e| e.json.as_mut().unwrap().as_object_mut().unwrap().retain(|k, _| k != "gate_id"))),
        ("bad gate_id", Box::new(|e| e.json.as_mut().unwrap()["gate_id"] = json!("zz"))),
        (
            "no commission",
            Box::new(|e| e.json.as_mut().unwrap().as_object_mut().unwrap().retain(|k, _| k != "commission_mist")),
        ),
        ("bad commission", Box::new(|e| e.json.as_mut().unwrap()["commission_mist"] = json!("-1"))),
        ("63-bit overflow", Box::new(|e| e.json.as_mut().unwrap()["commission_mist"] = json!(u64::MAX.to_string()))),
        ("no bcs", Box::new(|e| e.bcs.clear())),
        ("no digest", Box::new(|e| e.transaction_digest = None)),
        ("no index", Box::new(|e| e.event_index = None)),
    ];
    for (name, mutate) in cases {
        let mut e = base.clone();
        mutate(&mut e);
        assert!(to_indexed(Stream::AccessGate, &e, &p).is_err(), "{name}");
    }
    // An exact JSON number is accepted for a u64 (strings are what the node sends).
    let mut e = base.clone();
    e.json.as_mut().unwrap()["commission_mist"] = json!(5);
    assert_eq!(to_indexed(Stream::AccessGate, &e, &p).unwrap().unwrap().commission_mist, Some(5));
    // A kind without a gate needs no json.
    let mut u = raw(&platform_updated(1));
    u.json = None;
    assert!(to_indexed(Stream::AccessGate, &u, &p).unwrap().is_some());
}

#[test]
fn polls_the_whole_access_gate_module_and_the_one_sealed_content_event() {
    assert_eq!(stream_filter(Stream::AccessGate, &packages()), ag_filter());
    assert_eq!(stream_filter(Stream::SealedContent, &packages()), seal_filter());
}

// ── The poll loop ───────────────────────────────────────────────────────────

#[tokio::test]
async fn stores_every_page_with_its_cursor_ascending_and_resumes_from_the_cursor() {
    let db = require_db!();
    let p = pages(vec![
        (
            ag_filter(),
            vec![
                vec![raw(&minted(&gate_a(), 1, 1))],
                vec![raw(&consumed(&gate_a(), 2)), raw(&minted(&gate_b(), 3, 1))],
            ],
        ),
        (seal_filter(), vec![vec![raw(&sealed(&gate_a(), 4, "x"))]]),
    ]);
    let (mut ing, _) = ingestor(&db, Fake::new(paged(p.clone()))).await;
    ing.poll().await;
    let s = ing.store();
    let cov = s.coverage(Stream::AccessGate).await.unwrap();
    assert_eq!(
        (cov.from_checkpoint.as_deref(), cov.latest_checkpoint.as_deref(), cov.events),
        (Some("1"), Some("3"), 3)
    );
    assert_eq!(s.coverage(Stream::SealedContent).await.unwrap().events, 1);
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("1"));
    assert!(ing.last_success_at() > 0);
    // Every request: ascending (the source contract), page size 50, the stream's filter.
    for c in ing.source().calls.borrow().iter() {
        assert_eq!(c.limit, 50);
    }

    // A later poll continues after the stored cursor; nothing is duplicated.
    p.borrow_mut().get_mut(&ag_filter()).unwrap().push(vec![raw(&minted(&gate_a(), 9, 1))]);
    ing.source().calls.borrow_mut().clear();
    ing.poll().await;
    let first = ing.source().calls.borrow()[0].clone();
    assert_eq!((first.event_type.as_str(), first.after.as_deref()), (ag_filter().as_str(), Some("1")));
    let cov = ing.store().coverage(Stream::AccessGate).await.unwrap();
    assert_eq!((cov.latest_checkpoint.as_deref(), cov.events), (Some("9"), 4));
    assert!(ing.store().verify_counters().await.unwrap().is_empty());
}

#[tokio::test]
async fn keeps_the_cursor_when_the_node_reports_no_position() {
    let db = require_db!();
    let (mut ing, _) = ingestor(&db, Fake::new(paged(pages(vec![])))).await;
    ing.store_mut().ingest(Stream::AccessGate, &[], Some("kept")).await.unwrap();
    ing.poll_stream(Stream::AccessGate).await.unwrap();
    assert_eq!(ing.store().cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("kept"));
}

#[tokio::test]
async fn bounds_every_full_node_request() {
    let db = require_db!();
    let mut fake = Fake::new(paged(pages(vec![])));
    fake.hang = true;
    let mut opts = IngestOptions::new(packages());
    opts.request_timeout = Duration::from_millis(20);
    let (mut ing, _) = ingestor_with(&db, fake, opts).await;
    let err = ing.poll_stream(Stream::AccessGate).await.unwrap_err();
    assert!(err.contains("timed out"), "{err}");
}

#[tokio::test]
async fn stops_at_the_page_budget_so_one_stream_cannot_starve_the_other() {
    let db = require_db!();
    let many = (1..=5).map(|i| vec![raw(&minted(&gate_a(), i, 1))]).collect();
    let mut opts = IngestOptions::new(packages());
    opts.max_pages_per_poll = 2;
    let (mut ing, _) = ingestor_with(&db, Fake::new(paged(pages(vec![(ag_filter(), many)]))), opts).await;
    ing.poll_stream(Stream::AccessGate).await.unwrap();
    assert_eq!(ing.store().coverage(Stream::AccessGate).await.unwrap().events, 2);
}

#[tokio::test]
async fn a_stored_row_is_the_nodes_event_byte_for_byte() {
    let db = require_db!();
    let f = consumed(&gate_a(), 3);
    let (mut ing, _) = ingestor(&db, Fake::new(paged(pages(vec![(ag_filter(), vec![vec![raw(&f)]])])))).await;
    ing.poll_stream(Stream::AccessGate).await.unwrap();
    let page = ing
        .store()
        .page(Stream::AccessGate, &sui_indexer_store::PageQuery { limit: 1, ..Default::default() })
        .await
        .unwrap();
    let row = &page.rows[0];
    assert_eq!(row.bcs, f.bcs);
    assert_eq!(row.event_type, f.event_type);
    assert_eq!(row.transaction_digest, f.transaction_digest);
    assert_eq!(row.sender, addr(&f.sender));
}

#[tokio::test]
async fn skips_and_counts_an_undecodable_event_keeps_its_page_mates_and_advances() {
    let db = require_db!();
    let mut bad = raw(&minted(&gate_a(), 2, 1));
    bad.json = Some(json!({ "nft_id": "0x1" }));
    let page = vec![raw(&minted(&gate_a(), 1, 1)), bad, raw(&minted(&gate_b(), 3, 1))];
    let (mut ing, logs) = ingestor(&db, Fake::new(paged(pages(vec![(ag_filter(), vec![page])])))).await;
    ing.poll().await;
    let s = ing.store();
    assert_eq!(s.coverage(Stream::AccessGate).await.unwrap().events, 2);
    assert_eq!(s.health(Stream::AccessGate).await.unwrap().undecodable, 1);
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("0")); // advanced past the page
    assert!(logs.borrow().iter().any(|l| l.contains("UNDECODABLE")));
}

#[tokio::test]
async fn skips_unrecognised_events_and_logs_how_many() {
    let db = require_db!();
    let mut foreign = raw(&minted(&gate_a(), 2, 1));
    foreign.event_type = format!("{}::access_gate::Unknown", kit::ag());
    let (mut ing, logs) =
        ingestor(&db, Fake::new(paged(pages(vec![(ag_filter(), vec![vec![raw(&minted(&gate_a(), 1, 1)), foreign]])]))))
            .await;
    ing.poll().await;
    assert_eq!(ing.store().coverage(Stream::AccessGate).await.unwrap().events, 1);
    assert_eq!(ing.store().health(Stream::AccessGate).await.unwrap().undecodable, 0);
    assert!(logs.borrow().iter().any(|l| l == "access-gate: skipped 1 unrecognised event(s)"));
}

#[tokio::test]
async fn polls_the_streams_independently_and_health_names_the_failing_one() {
    let db = require_db!();
    let ok = paged(pages(vec![(seal_filter(), vec![vec![raw(&sealed(&gate_a(), 4, "x"))]])]));
    let ag = ag_filter();
    let fake =
        Fake::new(Box::new(move |req| if req.event_type == ag { Err("node unavailable".into()) } else { ok(req) }));
    let (mut ing, _) = ingestor(&db, fake).await;
    ing.poll().await;
    assert_eq!(ing.store().coverage(Stream::SealedContent).await.unwrap().events, 1);
    let h = ing.health(Stream::AccessGate);
    assert_eq!((h.last_success_at, h.last_error.as_deref()), (0, Some("node unavailable")));
    assert!(h.last_attempt_at > 0);
    let h = ing.health(Stream::SealedContent);
    assert!(h.last_success_at > 0 && h.last_error.is_none());
    assert_eq!(ing.last_success_at(), 0); // the oldest stream's success: not healthy as a whole
    // The heartbeats the API reads.
    assert_eq!(
        ing.store().poll_state(Stream::AccessGate).await.unwrap().last_error.as_deref(),
        Some("node unavailable")
    );
    assert!(ing.store().poll_state(Stream::SealedContent).await.unwrap().last_success_at > 0);
    assert!(ing.store().heartbeat(ALIVE_KEY).await.unwrap().is_some());
}

#[tokio::test]
async fn restarts_from_the_nodes_earliest_data_when_the_cursor_fell_below_its_retention() {
    let db = require_db!();
    let ag = ag_filter();
    let fresh = raw(&minted(&gate_a(), 50, 1));
    let fake = Fake::new(Box::new(move |req| {
        if req.after.as_deref() == Some("stale-cursor") {
            return Err("requested data below earliest available; lowest available tx_seq is 4094238283".into());
        }
        if req.event_type != ag || req.after.is_some() {
            return Ok(SourcePage::default());
        }
        Ok(SourcePage { events: vec![fresh.clone()], has_next_page: false, end_cursor: Some("fresh".into()) })
    }));
    let (mut ing, logs) = ingestor(&db, fake).await;
    ing.store_mut()
        .ingest(Stream::AccessGate, &[minted(&gate_a(), 1, 1).indexed()], Some("stale-cursor"))
        .await
        .unwrap();
    ing.poll().await;
    let s = ing.store();
    assert_eq!(s.cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("fresh"));
    let cov = s.coverage(Stream::AccessGate).await.unwrap();
    assert_eq!((cov.events, cov.latest_checkpoint.as_deref()), (2, Some("50")));
    assert_eq!(s.health(Stream::AccessGate).await.unwrap().gaps, 1);
    assert!(s.meta("gap:access-gate").await.unwrap().unwrap().contains("below earliest available"));
    assert_eq!(ing.health(Stream::AccessGate).last_error, None);
    assert!(logs.borrow().iter().any(|l| l.contains("below the node's retention")));
    assert!(ing.source().calls.borrow().len() > 1);
}

#[tokio::test]
async fn does_not_swallow_other_failures_as_a_gap() {
    let db = require_db!();
    let (mut ing, _) = ingestor(&db, Fake::new(Box::new(|_| Err("503 from fullnode".into())))).await;
    ing.store_mut().ingest(Stream::AccessGate, &[], Some("c")).await.unwrap();
    ing.poll().await;
    assert_eq!(ing.store().cursor(Stream::AccessGate).await.unwrap().as_deref(), Some("c"));
    assert_eq!(ing.store().health(Stream::AccessGate).await.unwrap().gaps, 0);
    assert_eq!(ing.health(Stream::AccessGate).last_error.as_deref(), Some("503 from fullnode"));
}

#[tokio::test]
async fn a_pruned_message_without_a_stored_cursor_is_a_plain_failure() {
    let db = require_db!();
    let (mut ing, _) = ingestor(&db, Fake::new(Box::new(|_| Err("data has been pruned".into())))).await;
    ing.poll().await;
    assert_eq!(ing.store().health(Stream::AccessGate).await.unwrap().gaps, 0);
    assert_eq!(ing.health(Stream::AccessGate).last_error.as_deref(), Some("data has been pruned"));
}

#[test]
fn pruned_wording() {
    for m in ["requested data below earliest available", "Earliest Available checkpoint is 9", "has been pruned"] {
        assert!(is_pruned(m), "{m}");
    }
    assert!(!is_pruned("503 from fullnode"));
}

#[tokio::test]
async fn a_restart_continues_from_the_persisted_heartbeat() {
    let db = require_db!();
    {
        let store = db.store().await;
        let mut ing =
            Ingestor::new(store, Fake::new(paged(pages(vec![]))), IngestOptions::new(packages()), Box::new(|_| {}))
                .await;
        ing.poll().await;
    }
    let before = db.store().await.poll_state(Stream::AccessGate).await.unwrap().last_success_at;
    assert!(before > 0);
    let store = db.store().await;
    let mut ing = Ingestor::new(
        store,
        Fake::new(Box::new(|_| Err("down".into()))),
        IngestOptions::new(packages()),
        Box::new(|_| {}),
    )
    .await;
    ing.poll().await;
    let after = ing.store().poll_state(Stream::AccessGate).await.unwrap();
    assert_eq!((after.last_success_at, after.last_error.as_deref()), (before, Some("down")));
}

// ── Page assembly from stream frames (core.listEvents semantics) ───────────

fn ev(n: u64) -> RawEvent {
    RawEvent { transaction_digest: Some(format!("t{n}")), ..Default::default() }
}

fn f(event: Option<u64>, cursor: Option<u8>, end: Option<i32>) -> Frame {
    Frame { event: event.map(ev), cursor: cursor.map(|c| vec![c]), end: end.map(Some) }
}

fn b64(c: u8) -> Option<String> {
    use base64::Engine;
    Some(base64::engine::general_purpose::STANDARD.encode([c]))
}

fn build(limit: u32, frames: Vec<Frame>) -> SourcePage {
    let mut b = PageBuilder::new(limit);
    for fr in frames {
        b.push(fr);
    }
    b.finish()
}

#[test]
fn a_page_ending_at_the_ledger_tip_has_no_next_page_and_ends_at_its_last_event() {
    let p = build(50, vec![f(Some(1), Some(1), None), f(Some(2), Some(2), None), f(None, Some(9), Some(5))]);
    assert_eq!(p.events.len(), 2);
    assert!(!p.has_next_page);
    assert_eq!(p.end_cursor, b64(2));
}

#[test]
fn the_look_ahead_item_marks_a_next_page_and_is_not_returned() {
    let p = build(2, vec![f(Some(1), Some(1), None), f(Some(2), Some(2), None), f(Some(3), Some(3), Some(1))]);
    assert_eq!(p.events.len(), 2);
    assert!(p.has_next_page);
    assert_eq!(p.end_cursor, b64(2));
}

#[test]
fn a_scan_limit_resumes_from_the_scan_frontier() {
    let p = build(50, vec![f(Some(1), Some(1), None), f(None, Some(7), None), f(None, Some(8), Some(2))]);
    assert!(p.has_next_page);
    assert_eq!(p.end_cursor, b64(8));
    // Nothing matched before the scan limit: still a next page, from the frontier.
    let empty = build(50, vec![f(None, Some(4), Some(2))]);
    assert_eq!((empty.events.len(), empty.has_next_page, empty.end_cursor), (0, true, b64(4)));
}

#[test]
fn an_empty_page_at_the_tip_reports_no_position() {
    let p = build(50, vec![f(None, Some(4), Some(5))]);
    assert_eq!((p.events.len(), p.has_next_page, p.end_cursor), (0, false, None));
    assert_eq!(build(50, vec![]), SourcePage::default());
}

#[test]
fn item_limit_on_the_last_item_means_more() {
    let p = build(2, vec![f(Some(1), Some(1), None), f(Some(2), Some(2), Some(1))]);
    assert!(p.has_next_page);
    assert_eq!(p.end_cursor, b64(2));
}

proptest! {
    /// Whatever frames arrive: at most `limit` events, in arrival order, and the end cursor is a
    /// watermark the stream actually sent (never invented).
    #[test]
    fn page_assembly_invariants(limit in 1u32..8, frames in proptest::collection::vec((any::<bool>(), proptest::option::of(any::<u8>()), proptest::option::of(0i32..7)), 0..40)) {
        let mut n = 0;
        let fs: Vec<Frame> = frames.iter().map(|(has, c, end)| {
            n += 1;
            Frame { event: has.then(|| ev(n)), cursor: c.map(|c| vec![c]), end: end.map(Some) }
        }).collect();
        let sent: Vec<Option<String>> = fs.iter().filter_map(|fr| fr.cursor.as_ref().map(|c| b64(c[0]))).collect();
        let order: Vec<u64> = fs.iter().filter_map(|fr| fr.event.as_ref()).map(|e| e.transaction_digest.as_ref().unwrap()[1..].parse().unwrap()).collect();
        let p = build(limit, fs);
        prop_assert!(p.events.len() <= limit as usize);
        let got: Vec<u64> = p.events.iter().map(|e| e.transaction_digest.as_ref().unwrap()[1..].parse().unwrap()).collect();
        prop_assert_eq!(&got[..], &order[..got.len()]);
        if let Some(c) = &p.end_cursor {
            prop_assert!(sent.contains(&Some(c.clone())));
        }
        if order.len() > limit as usize {
            prop_assert!(p.has_next_page);
        }
    }
}

#[test]
fn the_grpc_request_is_core_list_events_ascending_with_one_look_ahead_item() {
    let r = grpc_request(&ListRequest { event_type: ag_filter(), after: Some("AQID".into()), limit: 50 }).unwrap();
    let o = r.options.unwrap();
    assert_eq!(o.limit, Some(51));
    assert_eq!(o.ordering, Some(0));
    assert_eq!(o.after.as_deref(), Some(&[1u8, 2, 3][..]));
    assert_eq!(o.before, None);
    let paths = r.read_mask.unwrap().paths;
    assert!(paths.iter().any(|p| p == "json") && paths.iter().any(|p| p == "contents"));
    let lit = &r.filter.unwrap().terms[0].literals[0];
    assert!(!lit.negated);
    match &lit.predicate {
        Some(sui_rpc::proto::sui::rpc::v2::event_literal::Predicate::EventType(t)) => {
            assert_eq!(t.event_type.as_deref(), Some(ag_filter().as_str()))
        }
        p => panic!("unexpected predicate {p:?}"),
    }
    assert!(grpc_request(&ListRequest { event_type: "x".into(), after: Some("!!".into()), limit: 50 }).is_err());
    assert!(
        grpc_request(&ListRequest { event_type: "x".into(), after: None, limit: 50 })
            .unwrap()
            .options
            .unwrap()
            .after
            .is_none()
    );
}

// ── The real client against a local LedgerService ──────────────────────────

mod stub {
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use sui_rpc::proto::sui::rpc::v2::ledger_service_server::{LedgerService, LedgerServiceServer};
    use sui_rpc::proto::sui::rpc::v2::{
        Bcs, Event, GetServiceInfoRequest, GetServiceInfoResponse, ListEventsRequest, ListEventsResponse, QueryEnd,
        Watermark,
    };
    use tokio_stream::Stream;

    type Frames = Box<dyn Fn(&ListEventsRequest) -> Result<Vec<ListEventsResponse>, tonic::Status> + Send + Sync>;

    pub struct Stub {
        pub requests: Arc<Mutex<Vec<ListEventsRequest>>>,
        pub frames: Frames,
        pub chain_id: String,
    }

    #[tonic::async_trait]
    impl LedgerService for Stub {
        async fn get_service_info(
            &self,
            _: tonic::Request<GetServiceInfoRequest>,
        ) -> Result<tonic::Response<GetServiceInfoResponse>, tonic::Status> {
            let mut r = GetServiceInfoResponse::default();
            r.chain_id = Some(self.chain_id.clone());
            Ok(tonic::Response::new(r))
        }

        async fn list_events(
            &self,
            request: tonic::Request<ListEventsRequest>,
        ) -> Result<
            tonic::Response<Pin<Box<dyn Stream<Item = Result<ListEventsResponse, tonic::Status>> + Send>>>,
            tonic::Status,
        > {
            let req = request.into_inner();
            self.requests.lock().unwrap().push(req.clone());
            let frames = (self.frames)(&req)?;
            Ok(tonic::Response::new(Box::pin(tokio_stream::iter(frames.into_iter().map(Ok)))))
        }
    }

    pub fn frame(event: Option<Event>, cursor: &[u8], end: Option<i32>) -> ListEventsResponse {
        let mut w = Watermark::default();
        w.cursor = Some(cursor.to_vec().into());
        let mut r = ListEventsResponse::default();
        r.event = event;
        r.watermark = Some(w);
        r.end = end.map(|reason| {
            let mut e = QueryEnd::default();
            e.reason = Some(reason);
            e
        });
        r
    }

    pub fn event(f: &sui_indexer_testkit::Fixture) -> Event {
        let mut bcs = Bcs::default();
        bcs.value = Some(f.bcs.clone().into());
        let mut e = Event::default();
        e.event_type = Some(f.event_type.clone());
        e.sender = Some(f.sender.clone());
        e.contents = Some(bcs);
        e.json = Some(Box::new(to_value(&f.json)));
        e.checkpoint = Some(f.checkpoint);
        e.transaction_digest = Some(f.transaction_digest.clone());
        e.event_index = Some(f.event_index);
        e
    }

    pub fn to_value(j: &serde_json::Value) -> prost_types::Value {
        use prost_types::value::Kind;
        let kind = match j {
            serde_json::Value::Null => Kind::NullValue(0),
            serde_json::Value::Bool(b) => Kind::BoolValue(*b),
            serde_json::Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap()),
            serde_json::Value::String(s) => Kind::StringValue(s.clone()),
            serde_json::Value::Array(a) => {
                Kind::ListValue(prost_types::ListValue { values: a.iter().map(to_value).collect() })
            }
            serde_json::Value::Object(o) => Kind::StructValue(prost_types::Struct {
                fields: o.iter().map(|(k, v)| (k.clone(), to_value(v))).collect(),
            }),
        };
        prost_types::Value { kind: Some(kind) }
    }

    /// Serve `stub` on a free loopback port; returns `http://127.0.0.1:<port>`.
    pub async fn serve(stub: Stub) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(LedgerServiceServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        url
    }
}

#[tokio::test]
async fn the_grpc_source_reads_list_events_frames_and_the_chain_identifier() {
    use std::sync::{Arc, Mutex};

    let db = require_db!();
    use crate::source::GrpcSource;

    let a = minted(&gate_a(), 7, 123);
    let b = consumed(&gate_a(), 8);
    let (ea, eb) = (stub::event(&a), stub::event(&b));
    let requests = Arc::new(Mutex::new(vec![]));
    let url = stub::serve(stub::Stub {
        requests: requests.clone(),
        chain_id: TESTNET_GENESIS.into(),
        frames: Box::new(move |req| {
            if req.options.as_ref().and_then(|o| o.after.as_ref()).is_some_and(|c| c.as_ref() == [0xee]) {
                return Err(tonic::Status::out_of_range(
                    "requested data below earliest available; lowest available tx_seq is 9",
                ));
            }
            Ok(vec![
                stub::frame(Some(ea.clone()), &[1, 1], None),
                stub::frame(None, &[1, 2], None),
                stub::frame(Some(eb.clone()), &[1, 3], None),
                stub::frame(None, &[1, 4], Some(2)), // scan limit: more to read from [1, 4]
            ])
        }),
    })
    .await;
    let source = GrpcSource::new(&url).unwrap();
    assert_eq!(source.chain_identifier().await.unwrap(), TESTNET_GENESIS);
    let page = source
        .list_events(ListRequest { event_type: ag_filter(), after: Some("AQA=".into()), limit: 50 })
        .await
        .unwrap();
    assert_eq!(page.events, vec![raw(&a), raw(&b)]);
    assert!(page.has_next_page);
    assert_eq!(page.end_cursor.as_deref(), Some("AQQ=")); // base64 of the frontier watermark [1, 4]
    let req = requests.lock().unwrap()[0].clone();
    let o = req.options.unwrap();
    assert_eq!((o.limit, o.ordering, o.after.as_deref()), (Some(51), Some(0), Some(&[1u8, 0][..])));

    let err = source
        .list_events(ListRequest { event_type: ag_filter(), after: Some("7g==".into()), limit: 50 })
        .await
        .unwrap_err();
    assert!(is_pruned(&err), "{err}");

    // End to end: the ingester stores what the gRPC source read.
    let mut real = Ingestor::new(db.store().await, source, IngestOptions::new(packages()), Box::new(|_| {})).await;
    real.poll_stream(Stream::AccessGate).await.unwrap();
    assert_eq!(real.store().coverage(Stream::AccessGate).await.unwrap().events, 2);
}

#[tokio::test]
async fn an_unreachable_node_is_an_error_not_a_hang() {
    use crate::source::GrpcSource;
    let source = GrpcSource::new("http://127.0.0.1:9").unwrap();
    let err = source.chain_identifier().await.unwrap_err();
    // The cause is named, not only "tcp connect error".
    assert!(err.to_lowercase().contains("refused"), "{err}");
    assert!(source.list_events(ListRequest { event_type: ag_filter(), after: None, limit: 50 }).await.is_err());
}

// ── Startup wiring (main) ──────────────────────────────────────────────────

fn cfg(db: &TestDb) -> sui_indexer_store::config::IngestConfig {
    let env = db.env(&db.owner, &[("ACCESS_GATE_ORIGINAL_ID", &kit::ag()), ("SEAL_ORIGINAL_ID", &kit::seal())]);
    sui_indexer_store::config::IngestConfig::load(&env).unwrap()
}

fn quiet() -> Logger {
    Box::new(|_| {})
}

/// The `bak_<ms>` schemas of the database (the backups a confirmed reindex makes).
async fn backup_schemas(db: &TestDb) -> Vec<String> {
    db.admin()
        .await
        .query("SELECT nspname::text FROM pg_namespace WHERE nspname LIKE 'bak\\_%' ORDER BY 1", &[])
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect()
}

#[tokio::test]
async fn start_refuses_a_node_of_another_chain() {
    let db = require_db!();
    let mut fake = Fake::new(paged(pages(vec![])));
    fake.chain = Ok(MAINNET_GENESIS.into());
    let err = start(&cfg(&db), fake, quiet()).await.err().unwrap();
    assert!(err.contains("serves chain 35834a8a, not testnet (4c78adac)"), "{err}");
    let mut fake = Fake::new(paged(pages(vec![])));
    fake.chain = Err("connection refused".into());
    let err = start(&cfg(&db), fake, quiet()).await.err().unwrap();
    assert!(err.contains("cannot read the chain identifier"), "{err}");
}

#[tokio::test]
async fn start_refuses_a_second_ingester_while_one_is_running() {
    let db = require_db!();
    let c = cfg(&db);
    let first = start(&c, Fake::new(paged(pages(vec![]))), quiet()).await.unwrap();
    // The second start fails fast on the writer lock (after its short grace) and never touches the data.
    let started = std::time::Instant::now();
    let err = start(&c, Fake::new(paged(pages(vec![]))), quiet()).await.err().unwrap();
    assert!(err.contains("cannot start the writer") && err.contains("writer lock"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(!first.store().is_closed());
}

#[tokio::test]
async fn start_refuses_a_package_change_until_confirmed_then_backs_up_and_reindexes() {
    let db = require_db!();
    let c = cfg(&db);
    let mut ing = start(&c, Fake::new(paged(pages(vec![]))), quiet()).await.unwrap();
    ing.store_mut().ingest(Stream::AccessGate, &[minted(&gate_a(), 1, 1).indexed()], Some("c1")).await.unwrap();
    let generation = ing.store().generation().to_owned();
    drop(ing);

    let mut changed = c.clone();
    changed.access_gate_original_id = addr("0x77");
    let err = start(&changed, Fake::new(paged(pages(vec![]))), quiet()).await.err().unwrap();
    assert!(err.starts_with("refusing to start: ") && err.contains("REINDEX_CONFIRM=access-gate"), "{err}");
    assert!(backup_schemas(&db).await.is_empty());

    changed.reindex_confirm = vec![Stream::AccessGate];
    let logs: Logs = Rc::default();
    let sink = logs.clone();
    let ing = start(&changed, Fake::new(paged(pages(vec![]))), Box::new(move |m| sink.borrow_mut().push(m.to_owned())))
        .await
        .unwrap();
    assert_ne!(ing.store().generation(), generation);
    assert_eq!(ing.store().coverage(Stream::AccessGate).await.unwrap().events, 0);
    let backups = backup_schemas(&db).await;
    assert_eq!(backups.len(), 1, "{backups:?}");
    let copied: i64 =
        ing.store().query(&format!("SELECT count(*) FROM {}.events", backups[0]), &[]).await.unwrap()[0].get(0);
    assert_eq!(copied, 1);
    assert!(logs.borrow().iter().any(|l| l.contains("package changed for access-gate") && l.contains(&backups[0])));
}

#[tokio::test]
async fn the_loop_polls_every_interval_and_stops_on_shutdown() {
    let db = require_db!();
    let p = pages(vec![(ag_filter(), vec![vec![raw(&minted(&gate_a(), 1, 1))]])]);
    let mut ing = start(&cfg(&db), Fake::new(paged(p)), quiet()).await.unwrap();
    // Real time (the database is real I/O): polls at 0, 150 ms, 300 ms ...; each poll reads both streams.
    let shutdown = tokio::time::sleep(Duration::from_millis(500));
    run_loop(&mut ing, Duration::from_millis(150), shutdown).await.unwrap();
    let calls = ing.source().calls.borrow().len();
    assert!((6..=10).contains(&calls), "{calls} requests in about four polls of two streams");
    assert_eq!(ing.store().coverage(Stream::AccessGate).await.unwrap().events, 1);
}

#[tokio::test]
async fn the_loop_stops_when_the_connection_carrying_the_writer_lock_is_lost() {
    let db = require_db!();
    let mut ing = start(&cfg(&db), Fake::new(paged(pages(vec![]))), quiet()).await.unwrap();
    // The database kills the writer's session (a failover, an admin's pg_terminate_backend).
    let admin = db.admin().await;
    admin
        .execute(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = 'sui-indexer-ingest' AND datname = current_database()",
            &[],
        )
        .await
        .unwrap();
    let outcome = run_loop(&mut ing, Duration::from_millis(50), std::future::pending()).await;
    assert!(outcome.unwrap_err().contains("writer lock"), "the loop must end with an error, not retry");
}

#[tokio::test]
async fn the_healthcheck_follows_the_liveness_heartbeat_and_the_database() {
    let db = require_db!();
    let c = cfg(&db);
    // Nothing exists yet: unhealthy (no table), not a hang.
    assert!(healthcheck(&c.database, 15_000, 0).await.is_err());
    let ing = start(&c, Fake::new(paged(pages(vec![]))), quiet()).await.unwrap();
    let alive: i64 = ing.store().heartbeat(ALIVE_KEY).await.unwrap().unwrap().parse().unwrap();
    assert!(healthcheck(&c.database, 15_000, alive + 105_000).await.is_ok());
    assert!(healthcheck(&c.database, 15_000, alive + 105_001).await.unwrap_err().contains("limit 105000 ms"));
    ing.store().execute_batch(&format!("DELETE FROM heartbeats WHERE key = '{ALIVE_KEY}'")).await.unwrap();
    assert!(healthcheck(&c.database, 15_000, alive).await.unwrap_err().contains("no liveness heartbeat"));
    // An unreachable database is unhealthy.
    let dead =
        DatabaseSettings::parse(&c.database.url_unredacted().replace(&format!(":{}/", c.database.port()), ":1/"), true)
            .unwrap();
    assert!(healthcheck(&dead, 15_000, alive).await.is_err());
}
