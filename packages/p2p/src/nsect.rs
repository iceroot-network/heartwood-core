//! The kernel's `NSect`: an n-ary search for the highest height that satisfies a probe, over
//! `[low, high]`. Each probe asks about several heights at once and answers the highest of them
//! that matched; the verifier searches with n = 8, so each probe asks about 9 heights.
//!
//! The search is the reference's, step for step, so the probes a peer sees are its probes:
//!
//! - a probe of `[low, high]` asks about `low + round((high - low) × i / n)` for i from 0 to n,
//!   rounded half up, each height once;
//! - an answer ends the search when the range had at most n + 1 heights, when it is the highest
//!   height asked, or when the next height asked is the one after it; otherwise the search goes on
//!   between the answer and the next height asked;
//! - a probe that answers nothing ends the search with the last answer before it, which is
//!   nothing only when the first probe answered nothing.

use std::future::Future;

/// An n-ary search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NSect {
    n: u32,
}

impl NSect {
    /// A search that splits a range into `n` parts per probe. With `n` = 0 each probe asks about
    /// the lowest height only.
    pub fn new(n: u32) -> NSect {
        NSect { n }
    }

    /// How many parts each probe splits the range into.
    pub fn n(&self) -> u32 {
        self.n
    }

    /// The heights one probe of `[low, high]` asks about, in ascending order, each once. Empty
    /// when `low` is above `high`, a range the reference refuses.
    pub fn probes(&self, low: u32, high: u32) -> Vec<u32> {
        let Some(diff) = high.checked_sub(low) else {
            return Vec::new();
        };
        let n = u64::from(self.n);
        let mut heights: Vec<u32> = Vec::new();
        for i in 0..=n {
            // round(diff × i / n), half up: (2 × diff × i + n) / (2n). Every product fits 64 bits.
            let offset = u64::from(diff)
                .saturating_mul(i)
                .saturating_mul(2)
                .saturating_add(n)
                .checked_div(n.saturating_mul(2))
                .unwrap_or(0);
            let height = u64::from(low).saturating_add(offset);
            // The offset never exceeds diff, so the height is at most `high`.
            let height = u32::try_from(height).unwrap_or(high);
            if heights.last() != Some(&height) {
                heights.push(height);
            }
        }
        heights
    }

    /// The highest height in `[low, high]` that `probe` matches. `probe` gets the heights to ask
    /// about and answers the highest that matched, or `None` to stop the search, which then
    /// answers the last height matched before (`None` when it is the first probe); its error stops
    /// the search too. `probe` must answer one of the heights it was given: another answer ends
    /// the search with that answer. `None`, without a probe, when `low` is above `high`.
    pub async fn find<F, Fut, E>(&self, low: u32, high: u32, mut probe: F) -> Result<Option<u32>, E>
    where
        F: FnMut(Vec<u32>) -> Fut,
        Fut: Future<Output = Result<Option<u32>, E>>,
    {
        let (mut low, mut high) = (low, high);
        let mut highest = None;
        loop {
            let heights = self.probes(low, high);
            if heights.is_empty() {
                break;
            }
            let Some(matched) = probe(heights.clone()).await? else {
                break;
            };
            highest = Some(matched);
            // The range was narrow enough that every height in it was asked about.
            if u64::from(low).saturating_add(u64::from(self.n)) >= u64::from(high) {
                break;
            }
            // The highest height asked matched: nothing above it is left to ask about.
            if heights.last() == Some(&matched) {
                break;
            }
            let next = heights
                .iter()
                .position(|&height| height == matched)
                .and_then(|index| heights.get(index.saturating_add(1)));
            let Some(&next) = next else {
                break;
            };
            // The height after the match was asked about and did not match.
            if matched.checked_add(1) == Some(next) {
                break;
            }
            low = matched.saturating_add(1);
            high = next.saturating_sub(1);
        }
        Ok(highest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    /// The probes of a search for `common` over `[low, high]`, answered as a peer that has every
    /// height up to `common` answers, and the result.
    fn search(n: u32, low: u32, high: u32, common: u32) -> (Vec<Vec<u32>>, Option<u32>) {
        let mut asked = Vec::new();
        let result = futures_util::FutureExt::now_or_never(NSect::new(n).find(
            low,
            high,
            |heights: Vec<u32>| {
                let answer = heights.iter().rev().find(|&&h| h <= common).copied();
                asked.push(heights);
                std::future::ready(Ok::<_, Infallible>(answer))
            },
        ))
        .expect("the probes are ready at once")
        .expect("no probe fails");
        (asked, result)
    }

    #[test]
    fn the_probes_split_the_range_as_the_reference_rounds() {
        let nsect = NSect::new(8);
        assert_eq!(nsect.probes(1, 81), vec![1, 11, 21, 31, 41, 51, 61, 71, 81]);
        // diff 4: 0, 0.5, 1, 1.5, ... rounded half up, each height once.
        assert_eq!(nsect.probes(1, 5), vec![1, 2, 3, 4, 5]);
        assert_eq!(nsect.probes(7, 7), vec![7]);
        // diff 11: 1.375 rounds to 1, 2.75 to 3, 5.5 to 6 (half up).
        assert_eq!(nsect.probes(1, 12), vec![1, 2, 4, 5, 7, 8, 9, 11, 12]);
        assert!(nsect.probes(5, 4).is_empty());
        let wide = nsect.probes(1, u32::MAX);
        assert_eq!(wide.first(), Some(&1));
        assert_eq!(wide.last(), Some(&u32::MAX));
        assert_eq!(NSect::new(0).probes(3, 9), vec![3]);
    }

    #[test]
    fn a_search_finds_the_highest_matching_height() {
        for high in 1..=40 {
            for common in 1..=high {
                assert_eq!(
                    search(8, 1, high, common).1,
                    Some(common),
                    "[1, {high}] {common}"
                );
            }
        }
        let (asked, result) = search(8, 1, 10_000, 7_777);
        assert_eq!(result, Some(7_777));
        assert!(asked.len() <= 6, "{asked:?}");
        assert_eq!(
            asked[0],
            vec![1, 1251, 2501, 3751, 5001, 6250, 7500, 8750, 10000]
        );
        assert_eq!(asked[1][0], 7501);
    }

    #[test]
    fn a_search_without_a_first_match_finds_nothing() {
        assert_eq!(
            search(8, 5, 100, 4),
            (vec![NSect::new(8).probes(5, 100)], None)
        );
    }

    #[test]
    fn a_probe_that_stops_keeps_the_last_match() {
        let mut calls = 0;
        let result = futures_util::FutureExt::now_or_never(NSect::new(8).find(
            1,
            100,
            |heights: Vec<u32>| {
                calls += 1;
                let answer = (calls == 1).then(|| heights[4]);
                std::future::ready(Ok::<_, Infallible>(answer))
            },
        ))
        .expect("ready")
        .expect("no error");
        assert_eq!(result, Some(51));
        assert_eq!(calls, 2);
    }

    #[test]
    fn a_probe_error_stops_the_search() {
        let result = futures_util::FutureExt::now_or_never(NSect::new(8).find(
            1,
            100,
            |_heights: Vec<u32>| std::future::ready(Err::<Option<u32>, _>("stopped")),
        ))
        .expect("ready");
        assert_eq!(result, Err("stopped"));
    }
}
