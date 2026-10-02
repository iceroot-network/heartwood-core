//! Recorded pool sessions (`tests/fixtures/pool/sessions.jsonl`): the minimal cases of P10's seeded
//! runs that the pool judge's rules did not cover, each with the reference's answer, recorded with
//! the oracle of this tree. Every session runs through Heartwood's pool and is judged with the
//! pool's rules (`levels::pool`); each must come out as the documented difference it names.
//!
//! - `onlineView`: after a change of the online record, a vote or a revoke that one side refused as
//!   offline (or both, naming different delegates), in a step that also holds an entry of a
//!   removed type or one that does not decode;
//! - `fullPoolEviction`: a re-add whose full pool evicted a transaction and then refused the
//!   re-added one, which Heartwood refused without evicting.

use std::path::Path;

use heartwood_difftest::levels::pool::judge;
use heartwood_difftest::report::Outcome;
use heartwood_difftest::subject::{self, Case, Documented, Verdict};
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::VectorSet;
use serde_json::Value;

#[test]
fn recorded_pool_sessions_are_documented_differences() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crypto = VectorSet::load(&root.join("../../packages/crypto/tests/vectors"))
        .expect("the crypto vectors load");
    let devnet = Devnet::from_vectors(&crypto)
        .expect("one devnet configuration")
        .expect("a genesis class");
    let registry = subject::heartwood(Some(devnet));
    let pool = registry.get("pool.run").expect("the pool's subject");
    let text = std::fs::read_to_string(root.join("tests/fixtures/pool/sessions.jsonl"))
        .expect("the recorded sessions");
    let mut seen = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let session: Value = serde_json::from_str(line).expect("one session per line");
        let difference = match session["documented"].as_str() {
            Some("onlineView") => Documented::OnlineView,
            Some("fullPoolEviction") => Documented::FullPoolEviction,
            other => panic!("session {i}: no documented difference named {other:?}"),
        };
        let case = Case {
            op: "pool.run",
            network: "devnet",
            height: session["height"].as_u64().expect("a rule height"),
            input: &session["input"],
        };
        let Ok(Verdict::Output(ours)) = pool.run(&case) else {
            panic!("session {i}: Heartwood's pool gave no answer");
        };
        assert_eq!(
            judge(&session["input"], &session["expected"], &ours),
            Outcome::Documented(difference.reason().to_owned()),
            "session {i}"
        );
        seen.push(difference);
    }
    let count = |d: Documented| seen.iter().filter(|s| **s == d).count();
    assert_eq!(
        (
            count(Documented::OnlineView),
            count(Documented::FullPoolEviction)
        ),
        (9, 1)
    );
}
