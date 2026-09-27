//! The random samples of peers: the reference's shuffle, then a draw round-robin across subnets
//! (/24 for IPv4, /48 for IPv6), so that one subnet cannot fill a sample. On a network whose peers
//! share one subnet the draw is the reference's shuffle.
//!
//! The generator is seeded from the operating system; the `difftest` feature adds a fixed seed.

use std::net::IpAddr;
use std::sync::Mutex;

use rand::rngs::StdRng;

/// The random source of the samples.
#[derive(Debug)]
pub struct Sampler {
    #[expect(dead_code, reason = "the sampler's methods read it")]
    rng: Mutex<StdRng>,
}

impl Sampler {
    /// A sampler seeded from the operating system.
    pub fn from_os() -> Sampler {
        todo!("seed from the operating system")
    }

    /// A sampler with a fixed seed, for tests.
    #[cfg(feature = "difftest")]
    pub fn seeded(seed: u64) -> Sampler {
        let _ = seed;
        todo!("a fixed seed")
    }

    /// `items` in a random order (the reference's `shuffle`).
    pub fn shuffle<T>(&self, items: Vec<T>) -> Vec<T> {
        let _ = items;
        todo!("shuffle")
    }

    /// At most `count` of `items`: shuffled, then drawn round-robin across the subnets of
    /// `address`.
    pub fn diverse<T>(
        &self,
        items: Vec<T>,
        count: usize,
        address: impl Fn(&T) -> IpAddr,
    ) -> Vec<T> {
        let _ = (items, count, address);
        todo!("the subnet-diverse draw")
    }
}
