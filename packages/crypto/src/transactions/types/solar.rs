//! Assets of type group 2: burn and vote.

use crate::errors::{DecodeError, EncodeError};
use crate::transactions::codec;
use crate::transactions::types::{Asset, Votes};
use crate::utils::bytes::{Reader, Writer};
use crate::utils::sort_votes::insert_as_object_key;

/// The largest vote asset in bytes: `1 + Σ (3 + name bytes)`.
pub const MAX_VOTE_ASSET_BYTES: usize = 1024;

/// A vote name that the reference's vote object cannot hold: assigning it sets the object's
/// prototype instead, which a number leaves unchanged, so the entry is lost.
const PROTOTYPE_KEY: &str = "__proto__";

/// Write a burn asset: the amount (`u64`).
pub(crate) fn write_burn(amount: u64, out: &mut Writer) {
    out.u64_le(amount);
}

/// Read a burn asset.
pub(crate) fn read_burn(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    Ok(Asset::Burn {
        amount: reader.u64_le()?,
    })
}

/// The size of the vote asset of `votes`: `1 + Σ (3 + name bytes)`.
pub fn vote_asset_size(votes: &Votes) -> usize {
    votes.keys().fold(1, |size, name| {
        size.saturating_add(3).saturating_add(name.len())
    })
}

/// Write a vote asset: the vote count (`u8`), then per vote the name length (`u8`), the name and
/// the basis points (`u16`). Refused over [`MAX_VOTE_ASSET_BYTES`].
///
/// As in the reference, the length byte counts the name's UTF-16 code units, not its bytes. The
/// two agree for every delegate name; they differ only for a name the rules remove, where the
/// difference shows in the id.
pub(crate) fn write_vote(votes: &Votes, out: &mut Writer) -> Result<(), EncodeError> {
    let size = vote_asset_size(votes);
    if size > MAX_VOTE_ASSET_BYTES {
        return Err(EncodeError::VoteAssetTooLarge { size });
    }
    let count = u8::try_from(votes.len()).map_err(|_| EncodeError::OutOfRange {
        field: "asset.votes",
    })?;
    out.u8(count);
    for (name, &basis_points) in votes {
        let length =
            u8::try_from(name.encode_utf16().count()).map_err(|_| EncodeError::OutOfRange {
                field: "asset.votes",
            })?;
        out.u8(length);
        out.bytes(name.as_bytes());
        out.u16_le(basis_points);
    }
    Ok(())
}

/// Read a vote asset. Refused over [`MAX_VOTE_ASSET_BYTES`].
///
/// The votes are kept as the reference's vote object keeps them: names read with every
/// ill-formed UTF-8 sequence replaced, a repeated name keeping its first place and its last share,
/// array-index names first, and `__proto__` lost.
pub(crate) fn read_vote(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    let start = reader.offset();
    let count = reader.u8()?;
    let mut votes = Votes::new();
    for _ in 0..count {
        let length = usize::from(reader.u8()?);
        let name = codec::lossy_utf8(reader.bytes(length)?);
        let basis_points = reader.u16_le()?;
        if name != PROTOTYPE_KEY {
            insert_as_object_key(&mut votes, name, basis_points);
        }
    }
    let size = reader.offset() - start;
    if size > MAX_VOTE_ASSET_BYTES {
        return Err(DecodeError::VoteAssetTooLarge { size });
    }
    Ok(Asset::Vote { votes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(bytes: &[u8]) -> Result<Votes, DecodeError> {
        match read_vote(&mut Reader::new(bytes))? {
            Asset::Vote { votes } => Ok(votes),
            _ => unreachable!(),
        }
    }

    fn entry(name: &[u8], basis_points: u16) -> Vec<u8> {
        let mut out = vec![u8::try_from(name.len()).unwrap()];
        out.extend_from_slice(name);
        out.extend_from_slice(&basis_points.to_le_bytes());
        out
    }

    #[test]
    fn object_semantics() {
        let mut bytes = vec![4];
        bytes.extend(entry(b"b", 1000));
        bytes.extend(entry(b"__proto__", 1000));
        bytes.extend(entry(b"7", 2000));
        bytes.extend(entry(b"b", 7000));
        let votes = read(&bytes).unwrap();
        assert_eq!(
            votes.into_iter().collect::<Vec<_>>(),
            [("7".to_owned(), 2000), ("b".to_owned(), 7000)]
        );
        let votes = read(&[1, 1, 0xff, 0x10, 0x27]).unwrap();
        assert_eq!(votes["\u{fffd}"], 10_000);
    }

    #[test]
    fn utf16_length_byte() {
        let votes: Votes = [("\u{fffd}".to_owned(), 10_000)].into_iter().collect();
        let mut out = Writer::new();
        write_vote(&votes, &mut out).unwrap();
        assert_eq!(out.into_vec(), [1, 1, 0xef, 0xbf, 0xbd, 0x10, 0x27]);
    }

    #[test]
    fn size_limit_both_ways() {
        // 44 names of 20 bytes and one of 8 give exactly 1,024 bytes; one of 9 gives 1,025.
        let mut votes: Votes = (0..44).map(|i| (format!("{i:019}a"), 222)).collect();
        votes.insert("abcdefgh".to_owned(), 232);
        assert_eq!(vote_asset_size(&votes), 1024);
        let mut out = Writer::new();
        write_vote(&votes, &mut out).unwrap();
        let bytes = out.into_vec();
        assert_eq!(bytes.len(), 1024);
        assert_eq!(read(&bytes).unwrap(), votes);

        votes.shift_remove("abcdefgh");
        votes.insert("abcdefghi".to_owned(), 232);
        assert_eq!(
            write_vote(&votes, &mut Writer::new()),
            Err(EncodeError::VoteAssetTooLarge { size: 1025 })
        );
        let mut bytes = vec![45];
        for (name, &bp) in &votes {
            bytes.extend(entry(name.as_bytes(), bp));
        }
        assert_eq!(
            read(&bytes),
            Err(DecodeError::VoteAssetTooLarge { size: 1025 })
        );
    }
}
