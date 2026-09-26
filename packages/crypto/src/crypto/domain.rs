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

/// A domain tag of the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DomainTag {
    /// A section of the canonical state digest.
    StateDigestSection,
    /// The state digest at one height: the hash of the section hashes.
    StateDigestHeight,
    /// The running digest over the height digests of a replay.
    StateDigestChain,
}

impl DomainTag {
    /// Every tag of the registry.
    pub const ALL: [DomainTag; 3] = [
        DomainTag::StateDigestSection,
        DomainTag::StateDigestHeight,
        DomainTag::StateDigestChain,
    ];

    /// The tag text that the hash commits to.
    pub const fn as_str(self) -> &'static str {
        match self {
            DomainTag::StateDigestSection => "heartwood/csd1/section",
            DomainTag::StateDigestHeight => "heartwood/csd1/height",
            DomainTag::StateDigestChain => "heartwood/csd1/chain",
        }
    }
}

/// SHA-256 of the tag's length byte, the tag and `parts`, concatenated (see the module
/// documentation).
pub fn tagged_hash(tag: DomainTag, parts: &[&[u8]]) -> [u8; 32] {
    let _ = (tag, parts);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
