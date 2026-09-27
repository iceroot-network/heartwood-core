//! The random samples of peers: the reference's shuffle, then a draw round-robin across subnets
//! (/24 for IPv4, /48 for IPv6), so that one subnet cannot fill a sample. On a network whose peers
//! share one subnet the draw is the reference's shuffle.
//!
//! The generator is seeded from the operating system; the `difftest` feature adds a fixed seed and
//! a scripted sampler whose shuffles are given orders, so a harness can replay the order the
//! reference drew.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};

use indexmap::IndexMap;
use rand::SeedableRng;
use rand::rand_core::{OsError, OsRng};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;

/// Where the orders of the shuffles come from.
#[derive(Debug)]
enum Source {
    /// A generator.
    Random(Box<StdRng>),
    /// Given orders, one per shuffle, each listing the input indices in their new order, and how
    /// many shuffles found none that fitted.
    #[cfg(feature = "difftest")]
    Scripted(VecDeque<Vec<usize>>, usize),
}

/// The random source of the samples. A clone is a handle to the same source.
#[derive(Debug, Clone)]
pub struct Sampler {
    source: Arc<Mutex<Source>>,
}

/// The subnet a sample spreads over: the first three bytes of an IPv4 address, the first three
/// groups of an IPv6 one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Subnet {
    V4([u8; 3]),
    V6([u16; 3]),
}

fn subnet(ip: IpAddr) -> Subnet {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            Subnet::V4([a, b, c])
        }
        IpAddr::V6(ip) => {
            let [a, b, c, ..] = ip.segments();
            Subnet::V6([a, b, c])
        }
    }
}

impl Sampler {
    /// A sampler seeded from the operating system's random source.
    pub fn from_os() -> Result<Sampler, OsError> {
        let rng = StdRng::try_from_rng(&mut OsRng)?;
        Ok(Sampler::with(Source::Random(Box::new(rng))))
    }

    /// A sampler with a fixed seed, for tests.
    #[cfg(feature = "difftest")]
    pub fn seeded(seed: u64) -> Sampler {
        Sampler::with(Source::Random(Box::new(StdRng::seed_from_u64(seed))))
    }

    /// A sampler whose shuffles take `orders` in turn, each the input indices in their new order,
    /// for a harness that replays the reference's draws. A shuffle with no order left, or whose
    /// order is not a permutation of its input, keeps the input's order and counts as a misfit.
    #[cfg(feature = "difftest")]
    pub fn scripted(orders: Vec<Vec<usize>>) -> Sampler {
        Sampler::with(Source::Scripted(orders.into(), 0))
    }

    /// How many scripted orders are left unused, and how many shuffles found no fitting order.
    #[cfg(feature = "difftest")]
    pub fn script_left(&self) -> (usize, usize) {
        match &*self.source.lock().unwrap_or_else(PoisonError::into_inner) {
            Source::Scripted(orders, misfits) => (orders.len(), *misfits),
            Source::Random(_) => (0, 0),
        }
    }

    fn with(source: Source) -> Sampler {
        Sampler {
            source: Arc::new(Mutex::new(source)),
        }
    }

    /// `items` in a random order (the reference's `shuffle`).
    pub fn shuffle<T>(&self, mut items: Vec<T>) -> Vec<T> {
        match &mut *self.source.lock().unwrap_or_else(PoisonError::into_inner) {
            Source::Random(rng) => items.shuffle(rng.as_mut()),
            #[cfg(feature = "difftest")]
            Source::Scripted(orders, misfits) => {
                let order = orders.pop_front().unwrap_or_default();
                let mut sorted = order.clone();
                sorted.sort_unstable();
                if sorted.into_iter().eq(0..items.len()) {
                    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
                    return order
                        .iter()
                        .filter_map(|&i| slots.get_mut(i)?.take())
                        .collect();
                }
                *misfits = misfits.saturating_add(1);
            }
        }
        items
    }

    /// At most `count` of `items`: shuffled, then drawn round-robin across the subnets of
    /// `address`.
    pub fn diverse<T>(
        &self,
        items: Vec<T>,
        count: usize,
        address: impl Fn(&T) -> IpAddr,
    ) -> Vec<T> {
        round_robin(self.shuffle(items), count, address)
    }
}

/// At most `count` of `items`, drawn round-robin across their subnets: the first item of each
/// subnet in the order the subnets first appear, then the second of each, and so on. Items of one
/// subnet keep their order, so on a single subnet this is the first `count` items.
pub fn round_robin<T>(items: Vec<T>, count: usize, address: impl Fn(&T) -> IpAddr) -> Vec<T> {
    let mut groups: IndexMap<Subnet, VecDeque<T>> = IndexMap::new();
    for item in items {
        groups
            .entry(subnet(address(&item)))
            .or_default()
            .push_back(item);
    }
    let mut drawn = Vec::new();
    while drawn.len() < count && !groups.is_empty() {
        groups.retain(|_, group| {
            if drawn.len() < count
                && let Some(item) = group.pop_front()
            {
                drawn.push(item);
            }
            !group.is_empty()
        });
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn one_subnet_draws_the_shuffled_order() {
        let items: Vec<IpAddr> = (1..=9).map(|n| ip(&format!("192.0.2.{n}"))).collect();
        assert_eq!(round_robin(items.clone(), 4, |a| *a), items[..4]);
        assert_eq!(round_robin(items.clone(), 20, |a| *a), items);
        assert!(round_robin(items, 0, |a| *a).is_empty());
    }

    #[test]
    fn several_subnets_are_drawn_in_turn() {
        let items: Vec<IpAddr> = [
            "192.0.2.1",
            "192.0.2.2",
            "192.0.2.3",
            "198.51.100.1",
            "192.0.2.4",
            "2001:db8:1::1",
            "2001:db8:1:ff::2",
            "2001:db8:2::1",
        ]
        .into_iter()
        .map(ip)
        .collect();
        let drawn = round_robin(items, 6, |a| *a);
        let expected: Vec<IpAddr> = [
            "192.0.2.1",
            "198.51.100.1",
            "2001:db8:1::1",
            "2001:db8:2::1",
            "192.0.2.2",
            "2001:db8:1:ff::2",
        ]
        .into_iter()
        .map(ip)
        .collect();
        assert_eq!(drawn, expected);
    }

    #[test]
    fn a_shuffle_keeps_every_item() {
        let sampler = Sampler::from_os().unwrap();
        let mut shuffled = sampler.shuffle((0..50).collect::<Vec<u32>>());
        shuffled.sort_unstable();
        assert_eq!(shuffled, (0..50).collect::<Vec<u32>>());
        let sample = sampler.diverse((0..50).collect::<Vec<u8>>(), 8, |n| {
            IpAddr::from([198, 51, 100, *n])
        });
        assert_eq!(sample.len(), 8);
    }

    #[cfg(feature = "difftest")]
    #[test]
    fn a_scripted_sampler_replays_its_orders() {
        let sampler = Sampler::scripted(vec![vec![2, 0, 1], vec![0, 0], vec![]]);
        assert_eq!(sampler.shuffle(vec!['a', 'b', 'c']), ['c', 'a', 'b']);
        // Not a permutation: the input's order, and a misfit.
        assert_eq!(sampler.shuffle(vec!['a', 'b']), ['a', 'b']);
        assert_eq!(sampler.shuffle(Vec::<char>::new()), Vec::<char>::new());
        assert_eq!(sampler.script_left(), (0, 1));
        // Exhausted: the input's order.
        assert_eq!(sampler.shuffle(vec![1, 2]), [1, 2]);
        assert_eq!(sampler.script_left(), (0, 2));
        let a = Sampler::seeded(7).shuffle((0..20).collect::<Vec<u8>>());
        assert_eq!(a, Sampler::seeded(7).shuffle((0..20).collect::<Vec<u8>>()));
    }
}
