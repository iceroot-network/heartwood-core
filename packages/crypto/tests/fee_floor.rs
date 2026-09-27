//! The dynamic fee floor (vector class V19): the six types over memo lengths, recipients, votes of
//! 1 to 53 entries and resignation types; fee tables switched off, with `minFee` 0 and negative,
//! and without a type's key; and milestone files whose fee table or revoke delay has a value of
//! the wrong type, which Heartwood refuses at load. The `fee.minimum` records of V06 are checked
//! with the other records of that class.

#[macro_use]
mod common;
mod blocks_common;

use blocks_common::{check_devnet, fee_minimum, milestones};

#[test]
fn v19_fee_floor() {
    let file = vector_file!("V19-fee-floor");
    check_devnet(&file);
    let (mut compared, mut refused_files) = (0, 0);
    for record in &file.records {
        let label = record.label(&file.class);
        assert_eq!(record.op, "fee.minimum", "{label}");
        match milestones(record) {
            Ok(milestones) => {
                let expected = record
                    .expected
                    .as_ref()
                    .unwrap_or_else(|class| panic!("{label}: the oracle refused with {class}"));
                let actual = fee_minimum(record, &milestones)
                    .unwrap_or_else(|error| panic!("{label}: {error}"));
                assert_eq!(&actual, expected, "{label}");
                compared += 1;
            }
            // A fee or revoke-delay value the reference loads, and coerces or fails on when it
            // checks a fee: Heartwood refuses the file.
            Err(error) => {
                assert!(
                    error.contains("dynamicFees")
                        || error.contains("blocksToRevokeDelegateResignation"),
                    "{label}: {error}"
                );
                refused_files += 1;
            }
        }
    }
    assert_eq!((compared, refused_files), (204, 43));
}
