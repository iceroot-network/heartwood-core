//! The length-prefixed lists of the block and transaction codecs: each item is a `u32`
//! big-endian length and then the item's bytes.
//!
//! The reference's decoding loop runs while the offset is below the length minus 4, reads a
//! length, takes the bytes after it (cut silently at the end of the buffer), and stops once it
//! holds more items than its cap. So a final zero-length item and 1 to 3 trailing bytes are
//! dropped, a buffer of 4 bytes or fewer is an empty list, and a list can hold one item more than
//! its cap.

/// The cap on the blocks of a getBlocks response.
pub const MAX_BLOCKS: usize = 400;
/// The cap on the transactions of one block of a getBlocks response.
pub const MAX_BLOCK_TRANSACTIONS: usize = 500;
/// The cap on the transactions of a postTransactions request and a getUnconfirmedTransactions
/// response.
pub const MAX_TRANSACTIONS: usize = 1_000;
/// The cap on the ids of a getUnconfirmedTransactions request's exclude list.
pub const MAX_EXCLUSIONS: usize = 15_000;

/// The list of `items`: each with its length as a `u32` big-endian before it.
pub fn encode<T: AsRef<[u8]>>(items: &[T]) -> Vec<u8> {
    let _ = items;
    todo!()
}

/// The items of the list in `bytes`, read as the reference reads them: at most `cap + 1` items.
pub fn decode(bytes: &[u8], cap: usize) -> Vec<Vec<u8>> {
    let _ = (bytes, cap);
    todo!()
}

/// The ids of an exclude list: consecutive 32-byte ids, none unless the length is a multiple of
/// 32, and at most [`MAX_EXCLUSIONS`] + 1.
pub fn decode_exclude(bytes: &[u8]) -> Vec<[u8; 32]> {
    let _ = bytes;
    todo!()
}
