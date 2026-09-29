//! The chunk cache: downloaded chunks above a chunk that failed, kept for a later download, at
//! most 100 and at most 256 MiB, the oldest dropped first. A chunk is used once and then removed.
//!
//! As in the reference, a key may be cached twice (a read takes the first), and removing a key
//! removes every chunk cached under it.

use std::collections::VecDeque;

use heartwood_crypto::Transaction;

use crate::download::DownloadedBlock;

/// The most chunks the cache keeps.
pub const MAX_CHUNKS: usize = 100;
/// The most memory the cached chunks may hold, as [`chunk_bytes`] counts it. The reference keeps
/// 100 chunks of any size, and a reply of 20 MiB holds about four times as much once read, so
/// peers that sent large chunks above a missing one filled gigabytes.
pub const MAX_BYTES: usize = 256 * 1024 * 1024;

/// What a block holds besides its transactions, as [`chunk_bytes`] counts it.
const BLOCK_BYTES: usize = 1024;
/// What a transaction holds besides its bytes once read, as [`chunk_bytes`] counts it.
const TRANSACTION_BYTES: usize = 512;

/// About the memory `blocks` hold: each block's header, and each transaction's bytes and decoded
/// form.
pub fn chunk_bytes(blocks: &[DownloadedBlock]) -> usize {
    let transaction = |t: &Transaction| t.bytes().len().saturating_add(TRANSACTION_BYTES);
    let block = |b: &DownloadedBlock| {
        let transactions = b.transactions.iter().flatten().map(transaction);
        transactions.fold(BLOCK_BYTES, usize::saturating_add)
    };
    blocks.iter().map(block).fold(0, usize::saturating_add)
}

/// The chunk cache, keyed by the chunk's height range as the reference writes it
/// (`[<from>, <to>]`).
#[derive(Debug, Clone)]
pub struct ChunkCache {
    chunks: VecDeque<(String, Vec<DownloadedBlock>, usize)>,
    bytes: usize,
    budget: usize,
}

impl Default for ChunkCache {
    fn default() -> ChunkCache {
        ChunkCache::with_budget(MAX_BYTES)
    }
}

impl ChunkCache {
    /// An empty cache.
    pub fn new() -> ChunkCache {
        ChunkCache::default()
    }

    /// An empty cache whose chunks hold at most `budget` bytes, as [`chunk_bytes`] counts them.
    pub fn with_budget(budget: usize) -> ChunkCache {
        ChunkCache {
            chunks: VecDeque::new(),
            bytes: 0,
            budget,
        }
    }

    /// Whether a chunk is cached under `key`.
    pub fn has(&self, key: &str) -> bool {
        self.chunks.iter().any(|(cached, ..)| cached == key)
    }

    /// The first chunk cached under `key`.
    pub fn get(&self, key: &str) -> Option<&[DownloadedBlock]> {
        self.chunks
            .iter()
            .find(|(cached, ..)| cached == key)
            .map(|(_, blocks, _)| blocks.as_slice())
    }

    /// Cache `blocks` under `key`, dropping the oldest chunks beyond [`MAX_CHUNKS`] or the byte
    /// budget ([`MAX_BYTES`]); a chunk larger than the budget is not kept.
    pub fn set(&mut self, key: String, blocks: Vec<DownloadedBlock>) {
        let size = chunk_bytes(&blocks);
        self.bytes = self.bytes.saturating_add(size);
        self.chunks.push_back((key, blocks, size));
        while self.chunks.len() > MAX_CHUNKS || self.bytes > self.budget {
            let Some((_, _, size)) = self.chunks.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(size);
        }
    }

    /// Remove every chunk cached under `key`.
    pub fn remove(&mut self, key: &str) {
        let removed = self
            .chunks
            .iter()
            .filter(|(cached, ..)| cached == key)
            .map(|(.., size)| *size)
            .fold(0, usize::saturating_add);
        self.bytes = self.bytes.saturating_sub(removed);
        self.chunks.retain(|(cached, ..)| cached != key);
    }

    /// Take the first chunk cached under `key` and remove every chunk cached under it, as the
    /// download does with a chunk it finds here.
    pub fn take(&mut self, key: &str) -> Option<Vec<DownloadedBlock>> {
        let index = self.chunks.iter().position(|(cached, ..)| cached == key)?;
        let (_, blocks, size) = self.chunks.remove(index)?;
        self.bytes = self.bytes.saturating_sub(size);
        self.remove(key);
        Some(blocks)
    }

    /// The keys, oldest first.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.chunks.iter().map(|(key, ..)| key.as_str())
    }

    /// How many chunks are cached.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Whether no chunk is cached.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// The bytes the cached chunks hold, as [`chunk_bytes`] counts them.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_read_first_removed_all_and_capped() {
        let mut cache = ChunkCache::new();
        assert!(!cache.has("[1, 400]"));
        assert!(cache.get("[1, 400]").is_none());
        cache.set("[1, 400]".into(), Vec::new());
        cache.set("[401, 800]".into(), Vec::new());
        cache.set("[1, 400]".into(), Vec::new());
        assert!(cache.has("[1, 400]"));
        assert_eq!(cache.get("[401, 800]").map(<[_]>::len), Some(0));
        cache.remove("[1, 400]");
        assert_eq!(cache.keys().collect::<Vec<_>>(), ["[401, 800]"]);
        assert!(cache.take("[401, 800]").is_some());
        assert!(cache.is_empty());
        for n in 0..=MAX_CHUNKS {
            cache.set(format!("[{n}]"), Vec::new());
        }
        assert_eq!(cache.len(), MAX_CHUNKS);
        assert!(!cache.has("[0]"));
        assert!(cache.has("[100]"));
    }

    #[test]
    fn the_oldest_chunks_go_beyond_the_byte_budget() {
        let block = |height| {
            let mut block = DownloadedBlock {
                header: Default::default(),
                transactions: None,
                ip: std::net::IpAddr::from([192, 0, 2, 1]),
            };
            block.header.header.height = Some(height);
            block
        };
        let chunk = |from: u32| (from..from + 4).map(block).collect::<Vec<_>>();
        assert_eq!(chunk_bytes(&chunk(1)), 4 * BLOCK_BYTES);
        let mut cache = ChunkCache::with_budget(10 * BLOCK_BYTES);
        cache.set("[1, 4]".into(), chunk(1));
        cache.set("[5, 8]".into(), chunk(5));
        assert_eq!(cache.bytes(), 8 * BLOCK_BYTES);
        cache.set("[9, 12]".into(), chunk(9));
        assert_eq!(cache.keys().collect::<Vec<_>>(), ["[5, 8]", "[9, 12]"]);
        assert_eq!(cache.bytes(), 8 * BLOCK_BYTES);
        assert!(cache.take("[5, 8]").is_some());
        assert_eq!(cache.bytes(), 4 * BLOCK_BYTES);
        cache.remove("[9, 12]");
        assert_eq!((cache.bytes(), cache.len()), (0, 0));
        // A chunk larger than the whole budget is not kept.
        cache.set("[1, 12]".into(), (1..=12).map(block).collect());
        assert!(cache.is_empty());
    }
}
