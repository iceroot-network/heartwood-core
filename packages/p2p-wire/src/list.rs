//! The length-prefixed lists of the block and transaction codecs: each item is a `u32`
//! big-endian length and then the item's bytes.
//!
//! The reference's decoding loop runs while the offset is below the length minus 4, reads a
//! length, takes the bytes after it (cut silently at the end of the buffer), and stops once it
//! holds more items than its cap. So a final zero-length item and 1 to 3 trailing bytes are
//! dropped, a buffer of 4 bytes or fewer is an empty list, and a list can hold one item more than
//! its cap.
//!
//! The exclude list of a getUnconfirmedTransactions request is not length-prefixed: it is the
//! 32-byte ids one after the other ([`decode_exclude`]).

#[cfg(test)]
mod tests;

/// The cap on the blocks of a getBlocks response.
pub const MAX_BLOCKS: usize = 400;
/// The cap on the transactions of one block of a getBlocks response.
pub const MAX_BLOCK_TRANSACTIONS: usize = 500;
/// The cap on the transactions of a postTransactions request and a getUnconfirmedTransactions
/// response.
pub const MAX_TRANSACTIONS: usize = 1_000;
/// The cap on the ids of a getUnconfirmedTransactions request's exclude list.
pub const MAX_EXCLUSIONS: usize = 15_000;

/// The length of an id in the exclude list.
const ID_BYTES: usize = 32;

/// The list of `items`: each with its length as a `u32` big-endian before it.
///
/// Every item must be shorter than 4 GiB, which the payload limits of every route keep far out of
/// reach; a longer item's length would be written as `u32::MAX`.
pub fn encode<T: AsRef<[u8]>>(items: &[T]) -> Vec<u8> {
    let size = items.iter().fold(0usize, |size, item| {
        size.saturating_add(4).saturating_add(item.as_ref().len())
    });
    let mut bytes = Vec::with_capacity(size);
    for item in items {
        let item = item.as_ref();
        let length = u32::try_from(item.len()).unwrap_or(u32::MAX);
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(item);
    }
    bytes
}

/// The items of the list in `bytes`, read as the reference reads them: at most `cap + 1` items.
pub fn decode(bytes: &[u8], cap: usize) -> Vec<Vec<u8>> {
    items(bytes, cap).into_iter().map(<[u8]>::to_vec).collect()
}

/// [`decode`], with the items borrowed from `bytes`.
pub(crate) fn items(bytes: &[u8], cap: usize) -> Vec<&[u8]> {
    let mut items = Vec::new();
    let mut rest = bytes;
    // The reference's `offset < length - 4`: more than the four bytes of a length remain.
    while let Some((length, after)) = rest.split_first_chunk::<4>()
        && !after.is_empty()
    {
        let length = usize::try_from(u32::from_be_bytes(*length)).unwrap_or(usize::MAX);
        let (item, next) = after.split_at(length.min(after.len()));
        items.push(item);
        rest = next;
        if items.len() > cap {
            break;
        }
    }
    items
}

/// The ids of an exclude list: consecutive 32-byte ids, none unless the length is a multiple of
/// 32, and at most [`MAX_EXCLUSIONS`] + 1.
pub fn decode_exclude(bytes: &[u8]) -> Vec<[u8; 32]> {
    if !bytes.len().is_multiple_of(ID_BYTES) {
        return Vec::new();
    }
    let (ids, _) = bytes.as_chunks::<ID_BYTES>();
    ids.iter()
        .take(MAX_EXCLUSIONS.saturating_add(1))
        .copied()
        .collect()
}
