//! The reference's blockchain state machine: its states and events ([`machine`]), and the actions
//! its states run on entry ([`actions`]).
//!
//! An event that neither the current state nor its parent lists is ignored, as the reference's
//! machine ignores it.

/// An enum of unit variants, each with the name the reference's machine gives it (`name`).
macro_rules! named {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $variant:ident = $text:literal,)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($(#[$vdoc])* $variant,)* }

        impl $name {
            /// The name, as the reference's machine writes it.
            pub fn name(self) -> &'static str {
                match self { $($name::$variant => $text,)* }
            }
        }
    };
}

pub mod actions;
pub mod machine;

pub use self::actions::Action;
pub use self::machine::{MachineEvent, MachineState, SyncState, Transition, transition};
