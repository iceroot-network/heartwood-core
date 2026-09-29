//! `heartwood-forger`: the forger of Heartwood Core.
//!
//! A forger holds its delegates' keys and nothing else of a node: it opens no listening socket and
//! has no store and no API. Its relays drive it, as the reference's forger is driven: at every slot
//! it reads the round from the selected relay, and when one of its delegates is the slot's forger it
//! asks for the network state, the pool's transactions and the relay's status, runs the reference's
//! checks, signs the block and posts it back to the relay.
//!
//! - [`forger_service`]: the slot loop (`checkSlot`), the attempt loop of a slot (`forgeNewBlock`)
//!   with its checks and 2,000 ms margin, the forging quorum's reading (`isForgingAllowed`), the
//!   round's delegate lines, the error counter and the shutdown.
//! - [`client`] and [`deadlines`]: the relay client: one nes session per configured host, host
//!   selection and failover as the reference's, each call with its codec and its deadline.
//! - [`delegate`]: a delegate's key and the block it forges from the relay's inputs, byte for byte
//!   the reference's.
//! - [`keys`]: `delegates.json`, its permission check and its keys, wiped when dropped.
//! - [`signing_record`] and [`signer`]: the persisted per-key record of what was signed, so that a
//!   slot is signed once and a retry re-sends the same bytes, and the block signer over it.
//! - [`attestation`]: the attester, which signs the status its relay offers, over its own session.
//! - [`options`]: the component's options with the reference's schema and defaults.
//! - [`seams`]: the traits between the parts: the relay link, the attestation link and the block
//!   signer, and the small types they share; [`errors`]: the typed errors.
//!
//! Every relay answer is untrusted input: it never causes a panic, and arithmetic on it is checked.
//!
//! Features: `difftest` adds the crash test's `forger-child` binary and the kernel's manual clock,
//! test seams that only test code enables; a release build never has them.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

pub mod attestation;
pub mod client;
pub mod deadlines;
pub mod delegate;
pub mod errors;
pub mod forger_service;
pub mod keys;
pub mod options;
pub mod seams;
pub mod signer;
pub mod signing_record;

use heartwood_kernel::errors::BootError;
use heartwood_kernel::lifecycle::Component;

pub use crate::attestation::Attester;
pub use crate::client::{Client, ClientOptions};
pub use crate::delegate::Delegate;
pub use crate::errors::{
    AttestError, ForgeError, KeysError, OptionsError, RecordError, RelayError, SignError,
};
pub use crate::forger_service::{ForgerParts, ForgerService};
pub use crate::keys::{DelegateKeys, load_keys};
pub use crate::options::{Options, RelayHost};
pub use crate::seams::{
    AttestLink, BlockSigner, Deadline, ForgeInputs, ForgerEvent, ForgingTransactions, RelayLink,
    ShutdownSignal, ShutdownTrigger, Signed, SignedOutcome,
};
pub use crate::signer::Signer;
pub use crate::signing_record::{Decision, SigningRecord};

/// The component's package name, as `app.json` lists it. The forger role requires it: a forger
/// whose component fails to boot exits, where the reference's idles.
pub const PACKAGE: &str = "@solar-network/forger";

/// The forger component of a forger process: its service, booted and disposed by the process's
/// lifecycle. The loop itself runs on [`ForgerService::run`], which the process's composition root
/// starts after the boot.
pub struct Forger<L, S> {
    service: ForgerService<L, S>,
}

impl<L, S> Forger<L, S> {
    /// The component over `service`.
    pub fn new(service: ForgerService<L, S>) -> Forger<L, S> {
        Forger { service }
    }

    /// The service, to run its loop.
    pub fn service(&self) -> &ForgerService<L, S> {
        &self.service
    }
}

impl<L, S> Component for Forger<L, S>
where
    L: RelayLink + AttestLink + 'static,
    S: BlockSigner + 'static,
{
    const PACKAGE: &'static str = PACKAGE;
    const REQUIRED: bool = true;

    async fn boot(&mut self) -> Result<(), BootError> {
        self.service.boot().await
    }

    async fn dispose(&mut self) {
        self.service.dispose().await;
    }
}
