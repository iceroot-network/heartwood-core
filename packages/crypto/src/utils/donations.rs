//! Donations taken from the block reward.

use crate::identities::Address;
use crate::managers::Params;

/// The donation paid to each recipient of the merged milestone `params` out of `reward`, in the
/// order of the milestone file: `floor(reward × basisPoints / 10000)` each. The remainder stays
/// with the forger.
pub fn calculate_donations(reward: u64, params: &Params) -> Vec<(Address, u64)> {
    params
        .donations()
        .iter()
        .map(|donation| (*donation.address(), share(reward, donation.basis_points())))
        .collect()
}

/// `floor(amount × basis_points / 10000)`, computed without overflow. The result is at most
/// `amount` for up to 10,000 basis points, which the milestone loader guarantees.
pub(crate) fn share(amount: u64, basis_points: u16) -> u64 {
    let exact = u128::from(amount) * u128::from(basis_points) / 10_000;
    u64::try_from(exact).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn shares_round_down() {
        assert_eq!(share(0, 500), 0);
        assert_eq!(share(19, 500), 0);
        assert_eq!(share(20, 500), 1);
        assert_eq!(share(200_000_000, 500), 10_000_000);
        assert_eq!(share(1_000_026, 9_000), 900_023);
        assert_eq!(share(7_500_411_055, 9_000), 6_750_369_949);
        assert_eq!(share(u64::MAX, 9_000), 16_602_069_666_338_596_453);
        assert_eq!(share(u64::MAX, 10_000), u64::MAX);
        assert_eq!(share(u64::MAX, u16::MAX), u64::MAX);
    }

    proptest! {
        #[test]
        fn a_share_never_exceeds_the_amount(amount in any::<u64>(), basis_points in 0_u16..=10_000) {
            let part = share(amount, basis_points);
            prop_assert!(part <= amount);
            prop_assert_eq!(
                u128::from(part),
                u128::from(amount) * u128::from(basis_points) / 10_000
            );
        }
    }
}
