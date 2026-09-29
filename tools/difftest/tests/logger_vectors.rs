//! L0 over the logger's committed vectors (class V20-logger, `logger.format`), without an oracle.
//!
//! Every record is compared or a documented difference: a message with a character the logger
//! escapes is `Documented::EscapedLogText`, and its line must still be the reference's line with
//! the message escaped.

use std::path::Path;

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject;
use heartwood_difftest::vectors::VectorSet;

#[test]
fn the_logger_vectors_pass_l0() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/logger/tests/vectors");
    let set = VectorSet::load(&dir).expect("the logger's vectors load");
    let report = l0::run(&set, &subject::heartwood(None));
    assert!(report.is_success(), "{report}");
    assert!(report.is_complete(), "{report}");
    assert_eq!(
        report.compared() + report.documented(),
        set.record_count(),
        "{report}"
    );
    // Line breaks, colour codes and a line separator in a message, at five levels, four ways.
    assert_eq!(
        (report.compared(), report.documented()),
        (318, 60),
        "{report}"
    );
}
