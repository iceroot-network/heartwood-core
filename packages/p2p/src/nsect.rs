//! The kernel's `NSect`: an n-ary search for the highest height that satisfies a probe, over
//! `[low, high]`. Each probe asks about several heights at once and answers the highest of them
//! that matched; the verifier searches with n = 8, so each probe asks about 9 heights.

use std::future::Future;

/// An n-ary search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NSect {
    n: u32,
}

impl NSect {
    /// A search that splits a range into `n` parts per probe.
    pub fn new(n: u32) -> NSect {
        NSect { n }
    }

    /// How many parts each probe splits the range into.
    pub fn n(&self) -> u32 {
        self.n
    }

    /// The highest height in `[low, high]` that `probe` matches. `probe` gets the heights to ask
    /// about and answers the highest that matched, or `None` to stop the search, which then
    /// answers `None`; its error stops the search too.
    pub async fn find<F, Fut, E>(&self, low: u32, high: u32, probe: F) -> Result<Option<u32>, E>
    where
        F: FnMut(Vec<u32>) -> Fut,
        Fut: Future<Output = Result<Option<u32>, E>>,
    {
        let _ = (low, high, probe);
        todo!("the n-ary search")
    }
}
