//! The chunk cache: downloaded chunks above a chunk that failed, kept for a later download, at
//! most 100, the oldest dropped first. A chunk is used once and then removed.

use std::collections::VecDeque;

use crate::download::DownloadedBlock;

/// The most chunks the cache keeps.
pub const MAX_CHUNKS: usize = 100;

/// The chunk cache, keyed by the chunk's height range as the reference writes it
/// (`[<from>, <to>]`).
#[derive(Debug, Clone, Default)]
pub struct ChunkCache {
    #[expect(dead_code, reason = "the cache's methods read it")]
    chunks: VecDeque<(String, Vec<DownloadedBlock>)>,
}

impl ChunkCache {
    /// An empty cache.
    pub fn new() -> ChunkCache {
        ChunkCache::default()
    }

    /// Whether a chunk is cached under `key`.
    pub fn has(&self, key: &str) -> bool {
        let _ = key;
        todo!("the cache's lookup")
    }

    /// The first chunk cached under `key`.
    pub fn get(&self, key: &str) -> Option<&[DownloadedBlock]> {
        let _ = key;
        todo!("the cache's read")
    }

    /// Cache `blocks` under `key`, dropping the oldest chunk beyond [`MAX_CHUNKS`].
    pub fn set(&mut self, key: String, blocks: Vec<DownloadedBlock>) {
        let _ = (key, blocks);
        todo!("the cache's write")
    }

    /// Remove every chunk cached under `key`.
    pub fn remove(&mut self, key: &str) {
        let _ = key;
        todo!("the cache's removal")
    }
}
