//! The network description and the milestones.
//!
//! [`Network`] is `network.json`. [`Milestones`] is `milestones.json`, sorted, merged and checked
//! once at load; [`Milestones::at`] gives the [`Params`] in force at a height. There is no global
//! height: every rule takes its height's parameters explicitly.

pub mod config;
pub mod network;

pub use self::config::{Donation, DynamicFees, DynamicReward, Milestones, Params};
pub use self::network::Network;
