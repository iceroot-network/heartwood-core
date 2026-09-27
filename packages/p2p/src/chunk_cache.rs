//! The chunk cache: downloaded chunks above a chunk that failed, kept for a later download, at
//! most 100, the oldest dropped first. A chunk is used once and then removed.
//!
//! As in the reference, a key may be cached twice (a read takes the first), and removing a key
//! removes every chunk cached under it.

use std::collections::VecDeque;

use crate::download::DownloadedBlock;

/// The most chunks the cache keeps.
pub const MAX_CHUNKS: usize = 100;

/// The chunk cache, keyed by the chunk's height range as the reference writes it
/// (`[<from>, <to>]`).
#[derive(Debug, Clone, Default)]
pub struct ChunkCache {
    chunks: VecDeque<(String, Vec<DownloadedBlock>)>,
}

impl ChunkCache {
    /// An empty cache.
    pub fn new() -> ChunkCache {
        ChunkCache::default()
    }

    /// Whether a chunk is cached under `key`.
    pub fn has(&self, key: &str) -> bool {
        self.chunks.iter().any(|(cached, _)| cached == key)
    }

    /// The first chunk cached under `key`.
    pub fn get(&self, key: &str) -> Option<&[DownloadedBlock]> {
        self.chunks
            .iter()
            .find(|(cached, _)| cached == key)
            .map(|(_, blocks)| blocks.as_slice())
    }

    /// Cache `blocks` under `key`, dropping the oldest chunk beyond [`MAX_CHUNKS`].
    pub fn set(&mut self, key: String, blocks: Vec<DownloadedBlock>) {
        self.chunks.push_back((key, blocks));
        if self.chunks.len() > MAX_CHUNKS {
            self.chunks.pop_front();
        }
    }

    /// Remove every chunk cached under `key`.
    pub fn remove(&mut self, key: &str) {
        self.chunks.retain(|(cached, _)| cached != key);
    }

    /// Take the first chunk cached under `key` and remove every chunk cached under it, as the
    /// download does with a chunk it finds here.
    pub fn take(&mut self, key: &str) -> Option<Vec<DownloadedBlock>> {
        let index = self.chunks.iter().position(|(cached, _)| cached == key)?;
        let (_, blocks) = self.chunks.remove(index)?;
        self.remove(key);
        Some(blocks)
    }

    /// The keys, oldest first.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.chunks.iter().map(|(key, _)| key.as_str())
    }

    /// How many chunks are cached.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Whether no chunk is cached.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
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
}
