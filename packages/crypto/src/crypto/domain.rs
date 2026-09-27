//! The domain tags of Heartwood's own hashes: one registry for every structure Heartwood defines.
//!
//! Every hash of a structure that Heartwood defines (as opposed to the structures it shares with
//! the reference, whose hashes are fixed by the wire format) is a SHA-256 with a domain tag from
//! [`DomainTag`], so that no two kinds of hash can collide:
//!
//! `tagged_hash(tag, parts) = SHA-256(u8(len(tag)) || tag || parts[0] || parts[1] || ...)`
//!
//! The tag is ASCII and at most 255 bytes long. A new kind of hash adds a variant here and never
//! reuses a tag.

use crate::crypto::hash::sha256_concat;

/// A domain tag of the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DomainTag {
    /// A section of the canonical state digest.
    StateDigestSection,
    /// The state digest at one height: the hash of the section hashes.
    StateDigestHeight,
    /// The running digest over the height digests of a replay.
    StateDigestChain,
    /// A milestone entry that a node has applied, recorded so that a change to it after it was
    /// applied refuses the node's start.
    MilestoneEntry,
}

impl DomainTag {
    /// Every tag of the registry.
    pub const ALL: [DomainTag; 4] = [
        DomainTag::StateDigestSection,
        DomainTag::StateDigestHeight,
        DomainTag::StateDigestChain,
        DomainTag::MilestoneEntry,
    ];

    /// The tag text that the hash commits to.
    pub const fn as_str(self) -> &'static str {
        match self {
            DomainTag::StateDigestSection => "heartwood/csd1/section",
            DomainTag::StateDigestHeight => "heartwood/csd1/height",
            DomainTag::StateDigestChain => "heartwood/csd1/chain",
            DomainTag::MilestoneEntry => "heartwood/milestone-entry",
        }
    }
}

/// SHA-256 of the tag's length byte, the tag and `parts`, concatenated (see the module
/// documentation).
pub fn tagged_hash(tag: DomainTag, parts: &[&[u8]]) -> [u8; 32] {
    let text = tag.as_str().as_bytes();
    // Every tag of the registry is at most 255 bytes long (a unit test checks it).
    let length = [u8::try_from(text.len()).unwrap_or(u8::MAX)];
    sha256_concat(
        [length.as_slice(), text]
            .into_iter()
            .chain(parts.iter().copied()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::hash::sha256;
    use crate::utils::hex;

    #[test]
    fn tags_are_distinct_short_ascii() {
        for (i, tag) in DomainTag::ALL.iter().enumerate() {
            let text = tag.as_str();
            assert!(text.is_ascii() && text.len() <= 255, "{text}");
            assert!(
                DomainTag::ALL[..i]
                    .iter()
                    .all(|other| other.as_str() != text)
            );
        }
    }

    #[test]
    fn known_hashes() {
        let cases: [(DomainTag, &[&[u8]], &str); 4] = [
            (
                DomainTag::StateDigestSection,
                &[],
                "c55ad76cdad69ea70959b8662d3b333a758d577aa5e0e8c0916a40e68527a607",
            ),
            (
                DomainTag::StateDigestSection,
                &[b"#M\n"],
                "2b353b9b423b429dda42a5351569f606f6d9c2277d5596a99453239223f57f3a",
            ),
            (
                DomainTag::StateDigestHeight,
                &[&[0; 32], &[0; 32], &[0; 32], &[0; 32], &[0; 32]],
                "ecde0aeba97a0cd3bae77a01ab5cffbc792bd97337aa69c8b5643de422ea7adb",
            ),
            (
                DomainTag::StateDigestChain,
                &[&[0; 32], &core::array::from_fn::<u8, 32, _>(|i| i as u8)],
                "4ba59203f3afd565f45d00398bb6b038dc5b95ff5b73ebd6902430f6552bf432",
            ),
        ];
        for (tag, parts, expected) in cases {
            assert_eq!(hex::encode(&tagged_hash(tag, parts)), expected, "{tag:?}");
        }
    }

    #[test]
    fn the_tag_is_length_prefixed_and_parts_concatenate() {
        let tag = DomainTag::StateDigestHeight;
        let mut preimage = vec![21];
        preimage.extend_from_slice(b"heartwood/csd1/height");
        preimage.extend_from_slice(b"abc");
        assert_eq!(tagged_hash(tag, &[b"abc"]), sha256(&preimage));
        assert_eq!(tagged_hash(tag, &[b"a", b"", b"bc"]), sha256(&preimage));
        assert_ne!(
            tagged_hash(DomainTag::StateDigestSection, &[b"abc"]),
            tagged_hash(tag, &[b"abc"])
        );
    }
}
