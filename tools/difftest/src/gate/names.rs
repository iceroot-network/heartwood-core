//! The code names of the documented differences, as gate records count them.
//!
//! A gate record names the differences its run met by the variants' names in `subject.rs`
//! (`Documented::ReplayIndexOrder`, `ApiDifference::TieOrder`), never by the ids of any list
//! outside the repository. The report of a comparison prints each difference's reason text; the
//! recorder maps it back to the name here. [`name`] is an exhaustive `match`, so a variant added
//! to either enum stops compiling until it has a name, and [`ordinal`] with its test keeps
//! [`ALL`] complete.

use crate::subject::{ApiDifference, Documented};

/// Every documented difference, each once (an API difference under [`Documented::Api`]).
pub const ALL: [Documented; 36] = [
    Documented::PluginsPrototype,
    Documented::DeepGroups,
    Documented::WideAmount,
    Documented::RemovedType,
    Documented::ExtendedHeader,
    Documented::MultiSignature,
    Documented::NonUtf8Memo,
    Documented::VotesWithIndices,
    Documented::FeeKeyValueType,
    Documented::DecodeReason,
    Documented::FullPoolEviction,
    Documented::SavedStateUsername,
    Documented::RoundRowsAfterRevert,
    Documented::Locale,
    Documented::OnlineView,
    Documented::StrictSchema,
    Documented::UnlinkedForkSegment,
    Documented::OwnForkSegment,
    Documented::MissedRowsOfRefusedBlock,
    Documented::EscapedLogText,
    Documented::OverHeightOwnBlock,
    Documented::RoundWallets,
    Documented::ReplayIndexOrder,
    Documented::CliEnvParser,
    Documented::Api(ApiDifference::StoreFailure),
    Documented::Api(ApiDifference::HeightAboveTip),
    Documented::Api(ApiDifference::Semaphore),
    Documented::Api(ApiDifference::PeersIpFilter),
    Documented::Api(ApiDifference::TieOrder),
    Documented::Api(ApiDifference::BasePathLinks),
    Documented::Api(ApiDifference::RoundOrder),
    Documented::Api(ApiDifference::EstimatedCount),
    Documented::Api(ApiDifference::FeeTypes),
    Documented::Api(ApiDifference::SpecKeptOnly),
    Documented::Api(ApiDifference::AttributeOrder),
    Documented::Api(ApiDifference::QueryBounds),
];

/// The position of a difference in [`ALL`]. An exhaustive `match`: a variant without a
/// position stops the build, and the test below checks the list against it.
pub const fn ordinal(difference: Documented) -> usize {
    match difference {
        Documented::PluginsPrototype => 0,
        Documented::DeepGroups => 1,
        Documented::WideAmount => 2,
        Documented::RemovedType => 3,
        Documented::ExtendedHeader => 4,
        Documented::MultiSignature => 5,
        Documented::NonUtf8Memo => 6,
        Documented::VotesWithIndices => 7,
        Documented::FeeKeyValueType => 8,
        Documented::DecodeReason => 9,
        Documented::FullPoolEviction => 10,
        Documented::SavedStateUsername => 11,
        Documented::RoundRowsAfterRevert => 12,
        Documented::Locale => 13,
        Documented::OnlineView => 14,
        Documented::StrictSchema => 15,
        Documented::UnlinkedForkSegment => 16,
        Documented::OwnForkSegment => 17,
        Documented::MissedRowsOfRefusedBlock => 18,
        Documented::EscapedLogText => 19,
        Documented::OverHeightOwnBlock => 20,
        Documented::RoundWallets => 21,
        Documented::ReplayIndexOrder => 22,
        Documented::CliEnvParser => 23,
        Documented::Api(ApiDifference::StoreFailure) => 24,
        Documented::Api(ApiDifference::HeightAboveTip) => 25,
        Documented::Api(ApiDifference::Semaphore) => 26,
        Documented::Api(ApiDifference::PeersIpFilter) => 27,
        Documented::Api(ApiDifference::TieOrder) => 28,
        Documented::Api(ApiDifference::BasePathLinks) => 29,
        Documented::Api(ApiDifference::RoundOrder) => 30,
        Documented::Api(ApiDifference::EstimatedCount) => 31,
        Documented::Api(ApiDifference::FeeTypes) => 32,
        Documented::Api(ApiDifference::SpecKeptOnly) => 33,
        Documented::Api(ApiDifference::AttributeOrder) => 34,
        Documented::Api(ApiDifference::QueryBounds) => 35,
    }
}

/// The code name of a difference: `Documented::X`, or `ApiDifference::X` for an API one.
pub const fn name(difference: Documented) -> &'static str {
    match difference {
        Documented::PluginsPrototype => "Documented::PluginsPrototype",
        Documented::DeepGroups => "Documented::DeepGroups",
        Documented::WideAmount => "Documented::WideAmount",
        Documented::RemovedType => "Documented::RemovedType",
        Documented::ExtendedHeader => "Documented::ExtendedHeader",
        Documented::MultiSignature => "Documented::MultiSignature",
        Documented::NonUtf8Memo => "Documented::NonUtf8Memo",
        Documented::VotesWithIndices => "Documented::VotesWithIndices",
        Documented::FeeKeyValueType => "Documented::FeeKeyValueType",
        Documented::DecodeReason => "Documented::DecodeReason",
        Documented::FullPoolEviction => "Documented::FullPoolEviction",
        Documented::SavedStateUsername => "Documented::SavedStateUsername",
        Documented::RoundRowsAfterRevert => "Documented::RoundRowsAfterRevert",
        Documented::Locale => "Documented::Locale",
        Documented::OnlineView => "Documented::OnlineView",
        Documented::StrictSchema => "Documented::StrictSchema",
        Documented::UnlinkedForkSegment => "Documented::UnlinkedForkSegment",
        Documented::OwnForkSegment => "Documented::OwnForkSegment",
        Documented::MissedRowsOfRefusedBlock => "Documented::MissedRowsOfRefusedBlock",
        Documented::EscapedLogText => "Documented::EscapedLogText",
        Documented::OverHeightOwnBlock => "Documented::OverHeightOwnBlock",
        Documented::RoundWallets => "Documented::RoundWallets",
        Documented::ReplayIndexOrder => "Documented::ReplayIndexOrder",
        Documented::CliEnvParser => "Documented::CliEnvParser",
        Documented::Api(api) => api_name(api),
    }
}

/// The code name of an API difference.
pub const fn api_name(difference: ApiDifference) -> &'static str {
    match difference {
        ApiDifference::StoreFailure => "ApiDifference::StoreFailure",
        ApiDifference::HeightAboveTip => "ApiDifference::HeightAboveTip",
        ApiDifference::Semaphore => "ApiDifference::Semaphore",
        ApiDifference::PeersIpFilter => "ApiDifference::PeersIpFilter",
        ApiDifference::TieOrder => "ApiDifference::TieOrder",
        ApiDifference::BasePathLinks => "ApiDifference::BasePathLinks",
        ApiDifference::RoundOrder => "ApiDifference::RoundOrder",
        ApiDifference::EstimatedCount => "ApiDifference::EstimatedCount",
        ApiDifference::FeeTypes => "ApiDifference::FeeTypes",
        ApiDifference::SpecKeptOnly => "ApiDifference::SpecKeptOnly",
        ApiDifference::AttributeOrder => "ApiDifference::AttributeOrder",
        ApiDifference::QueryBounds => "ApiDifference::QueryBounds",
    }
}

/// The documented differences whose reason is a named text of the harness, not a variant: the
/// code name (the constant's path) and the text.
pub const NAMED_REASONS: [(&str, &str); 2] = [
    ("levels::BOUND_KEY", crate::levels::BOUND_KEY),
    (
        "synthetic_rollback::OLDER_SAVED_STATE",
        crate::synthetic_rollback::OLDER_SAVED_STATE,
    ),
];

/// What a hit is counted under when a report prints a documented difference whose reason has no
/// code name: this prefix and the reason, which no register entry claims.
pub const UNNAMED: &str = "unnamed: ";

/// The code name of the difference whose reason text a report printed.
pub fn name_of_reason(reason: &str) -> Option<&'static str> {
    ALL.iter()
        .find(|d| d.reason() == reason)
        .map(|d| name(*d))
        .or_else(|| {
            NAMED_REASONS
                .iter()
                .find(|(_, text)| *text == reason)
                .map(|(name, _)| *name)
        })
}

/// Every code name: [`ALL`]'s in its order, then [`NAMED_REASONS`]'.
pub fn all_names() -> Vec<&'static str> {
    ALL.iter()
        .map(|d| name(*d))
        .chain(NAMED_REASONS.iter().map(|(name, _)| *name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_list_holds_every_difference_at_its_ordinal() {
        for (index, difference) in ALL.iter().enumerate() {
            assert_eq!(ordinal(*difference), index, "{}", name(*difference));
        }
        // No variant beyond the list: the highest ordinal is the last position.
        let highest = ALL.iter().map(|d| ordinal(*d)).max().unwrap_or(0);
        assert_eq!(highest + 1, ALL.len());
    }

    #[test]
    fn names_and_reasons_are_unique() {
        let names: BTreeSet<_> = ALL.iter().map(|d| name(*d)).collect();
        assert_eq!(names.len(), ALL.len());
        let reasons: BTreeSet<_> = ALL.iter().map(|d| d.reason()).collect();
        assert_eq!(reasons.len(), ALL.len());
    }

    #[test]
    fn a_reason_maps_back_to_its_name() {
        for difference in ALL {
            assert_eq!(name_of_reason(difference.reason()), Some(name(difference)));
        }
        for (name, text) in NAMED_REASONS {
            assert_eq!(name_of_reason(text), Some(name));
            assert!(all_names().contains(&name));
            assert!(ALL.iter().all(|d| d.reason() != text));
        }
        assert_eq!(name_of_reason("not a reason"), None);
    }
}
