//! `seed-fixture <db> [poll-age-ms]`: create a database holding the [`seed`] fixtures and fresh
//! heartbeats, for the CI client-contract job (the real client packages against the `api` binary).

use sui_indexer_store::time::now_ms;
use sui_indexer_store::{PollState, Store, Stream};
use sui_indexer_testkit::seed;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: seed-fixture <db> [poll-age-ms]");
    let age: i64 = args.next().map_or(0, |a| a.parse().expect("poll-age-ms is an integer"));
    let mut store = Store::open(&path).expect("open the database");
    store
        .bind_packages(
            &[(Stream::AccessGate, sui_indexer_testkit::ag()), (Stream::SealedContent, sui_indexer_testkit::seal())],
            &[],
            None,
        )
        .expect("bind the fixture packages");
    seed(&store);
    let at = now_ms() - age;
    for s in Stream::ALL {
        store
            .record_poll(s, &PollState { last_success_at: at, last_error: None, last_attempt_at: at })
            .expect("heartbeat");
    }
    println!("seeded {path} (generation {})", store.generation());
}
