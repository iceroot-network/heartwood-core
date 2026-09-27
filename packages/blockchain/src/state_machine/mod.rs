//! The reference's blockchain state machine: its states and events ([`machine`]), and the actions
//! its states run on entry ([`actions`]).
//!
//! An event that neither the current state nor its parent lists is ignored, as the reference's
//! machine ignores it.

pub mod actions;
pub mod machine;

pub use self::actions::Action;
pub use self::machine::{MachineEvent, MachineState, SyncState, Transition, transition};
