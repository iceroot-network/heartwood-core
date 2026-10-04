//! The L1 runner's memory over a run. A property's run keeps no case's input until a case fails,
//! so its memory grows only by each case's report line: P13's inputs carry a whole corpus chain
//! each (hundreds of kilobytes in memory), and a runner that kept every input grew by that much
//! per case, past 30 GB in 70,000 cases.
//!
//! The oracle is a stand-in (`sh` and `head`) that answers every request with a null result, and
//! no subject is registered, so every case passes as having no subject and the run measures the
//! runner alone. The resident size is read from `/proc/self/status` (Linux) by a thread that
//! samples it while the run goes on; the test is alone in its binary, so no other test allocates
//! beside it.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use heartwood_difftest::levels::l1::{self, L1Options, Property};
use heartwood_difftest::oracle::{Mode, Oracle};
use heartwood_difftest::store::ScratchDir;
use heartwood_difftest::strategies::transactions::TxContext;
use heartwood_difftest::subject::Registry;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::VectorSet;

/// The stand-in for `oracle-server.js`, run by `sh`: each request line is answered, in order, with
/// a null result and the next id (the runner numbers its requests from 1). `head` reads exactly
/// one line, however long, since the runner sends the next request only after the answer.
const STAND_IN: &str = r#"n=0
while line=$(head -n 1) && [ -n "$line" ]; do
    n=$((n + 1))
    printf '{"id":%d,"ok":true,"result":null}\n' "$n"
done
"#;

/// The cases of the measured run.
const CASES: u32 = 320;
/// The most the resident size may grow over the measured run, in kB. Kept inputs take far more:
/// 320 P13 inputs are over 100 MB.
const MOST_GROWTH_KB: u64 = 32 * 1024;

fn context() -> Arc<TxContext> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto/tests/vectors");
    let set = VectorSet::load(&dir).expect("the committed vectors load");
    let devnet = Devnet::from_vectors(&set)
        .unwrap()
        .expect("a genesis class");
    let text = l1::devnet_milestones(&set).expect("the devnet milestone file");
    Arc::new(TxContext::new(devnet.network.clone(), text).expect("the milestones load"))
}

/// The process's resident size, in kB.
fn resident_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("/proc/self/status");
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("a VmRSS line")
}

/// Sets its flag when dropped, so that the sampler stops when the measured run ends, by a panic
/// too.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// The largest resident size, in kB, that a thread sampling every 5 ms sees while `f` runs.
fn largest_resident_kb<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let sampler = scope.spawn(|| {
            let mut largest = resident_kb();
            while !done.load(Ordering::Relaxed) {
                largest = largest.max(resident_kb());
                std::thread::sleep(Duration::from_millis(5));
            }
            largest.max(resident_kb())
        });
        let value = {
            let _stop = StopOnDrop(&done);
            f()
        };
        (value, sampler.join().expect("the sampler"))
    })
}

#[test]
fn a_long_processor_run_keeps_no_passed_input() {
    let dir = ScratchDir::new("l1-memory").unwrap();
    std::fs::write(dir.path().join("oracle-server.js"), STAND_IN).unwrap();
    let oracle = Oracle {
        mode: Mode::Local {
            solar_dir: dir.path().to_path_buf(),
        },
        docker: Vec::new(),
        node: "sh".into(),
        scripts: dir.path().to_path_buf(),
    };
    let mut server = oracle.server().unwrap();
    let registry = Registry::new();
    let context = context();
    let options = |cases| L1Options {
        properties: vec![Property::Processor],
        cases,
        seed: 1,
        regressions: None,
        min_reach: 0,
        decision_reach: 0,
        served_reach: 0,
    };
    // A first short run loads the corpora and brings the runner to its steady state.
    l1::run(&mut server, &registry, &context, &options(16)).unwrap();
    let before = resident_kb();
    let (run, largest) =
        largest_resident_kb(|| l1::run(&mut server, &registry, &context, &options(CASES)).unwrap());
    let cases = run.report.results.iter().filter(|r| r.line > 0).count();
    assert_eq!(cases, CASES as usize, "{}", run.report);
    let grown = largest.saturating_sub(before);
    let measured =
        format!("the resident size grew by {grown} kB over {CASES} cases (from {before} kB)");
    eprintln!("{measured}");
    assert!(grown < MOST_GROWTH_KB, "{measured}");
}
