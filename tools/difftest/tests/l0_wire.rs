//! L0 over the committed golden vectors of `heartwood-p2p-wire`, without an oracle: the wire codec
//! (class V13), where every record is compared and equal to the reference's result, none is
//! skipped, and the only documented differences are the records that assert them on both sides;
//! and the exchange (class V24: the requests the client writes as a reference server reads them,
//! and the replies' checks), under the devnet's rules, with nothing skipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject::{self, Documented};
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{ClassDir, MANIFEST, VectorSet};

/// The vector class of the wire codec; the directory also holds the exchange class.
const CLASS: &str = "V13-p2p-wire";

/// The pinned counts of an L0 run over the committed wire vectors: the records compared (all
/// equal to the reference's result), and for each documented difference the records that assert
/// it. They change only on purpose: a new subject, a new documented difference, or new vectors.
/// `None` while the class is not installed.
const PINNED: Option<(usize, [(Documented, usize); 3])> = Some((
    1179,
    [
        (Documented::PluginsPrototype, 1),
        (Documented::DeepGroups, 2),
        (Documented::WideAmount, 11),
    ],
));

fn vector_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/p2p-wire/tests/vectors")
}

#[test]
fn the_committed_wire_vectors_pass_l0() {
    let dir = vector_dir();
    if PINNED.is_none() && !dir.join(MANIFEST).exists() {
        // Nothing is installed and nothing is pinned.
        return;
    }
    let mut set = VectorSet::load(&dir).expect("the committed wire vectors load");
    // The directory also holds the exchange class, which its own tests compare.
    set.files.retain(|file| file.meta.class == CLASS);
    assert_eq!(set.files.len(), 1, "{CLASS} in {}", dir.display());
    // The class is self-contained: its records carry their own configuration, so no devnet.
    let report = l0::run(&set, &subject::heartwood(None));
    let Some((compared, documented)) = PINNED else {
        panic!("the wire vectors are installed: pin the counts of this run in PINNED\n{report}");
    };
    assert!(report.is_success(), "{report}");
    assert!(report.without_subject().is_empty(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
    assert_eq!(
        report.compared() + report.documented(),
        set.record_count(),
        "{report}"
    );
    let documented: BTreeMap<&str, usize> = documented
        .iter()
        .map(|&(difference, records)| (difference.reason(), records))
        .collect();
    assert_eq!(report.documented_by_reason(), documented, "{report}");
    assert_eq!(report.compared(), compared, "{report}");
}

/// The exchange class, which the directory holds beside the codec's.
const EXCHANGE: &str = "V24-p2p-exchange";

#[test]
fn the_committed_exchange_vectors_pass_l0() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = ClassDir::from_env(
        &workspace,
        "WIRE_VECTORS",
        "packages/p2p-wire/tests/vectors",
    );
    let Some(mut set) = dir.load(EXCHANGE).expect("the exchange class is installed") else {
        eprintln!(
            "{EXCHANGE} is not in {}, which WIRE_VECTORS names",
            dir.path().display()
        );
        return;
    };
    set.files.retain(|file| file.meta.class == EXCHANGE);
    // The requests' version check and the replies' limits need the devnet's rules.
    let crypto = VectorSet::load(&workspace.join("packages/crypto/tests/vectors"))
        .expect("the crypto vectors load");
    let devnet = Devnet::from_vectors(&crypto)
        .expect("one devnet configuration")
        .expect("the crypto vectors have a genesis class");
    let report = l0::run(&set, &subject::heartwood(Some(devnet)));
    eprintln!("{report}");
    assert!(report.is_success(), "{report}");
    assert!(report.is_complete(), "{report}");
}
