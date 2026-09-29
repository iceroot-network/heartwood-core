//! The checks, one file per area. Each function reads only its
//! [`crate::doctor::registry::CheckInput`] (the parsed view and the run's options) and gives its
//! findings. The rules: facts first, then fixed advice by key with typed values; `unknown` is never
//! a pass; a finding whose basis is the log alone never gives disruptive advice as a fail (another
//! source must agree).

pub mod clock;
pub mod config;
pub mod disk;
pub mod forger;
pub mod host;
pub mod net;
pub mod onboarding;
pub mod peers;
pub mod process;
pub mod quorum;
pub mod slots;
pub mod sync;
pub mod version;
