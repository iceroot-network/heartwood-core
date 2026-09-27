//! The peer verifier (`checkState`, `checkStateFast`).
//!
//! - The claimed header: its height must be the state's height. A header equal to our own block at
//!   that height (as JSON) passes. Below our height it must be signed by a delegate of its round;
//!   at or above our height its own signature suffices (reproduced).
//! - A peer whose header is our block at that height is verified at once, with the common height
//!   its height.
//! - Otherwise the highest common block: an 8-ary search ([`crate::nsect::NSect`]) over
//!   `[1, min(theirs, ours)]`, each probe sending our ids at 9 heights; a reply naming an id we did
//!   not ask for, or at another height, fails the verification.
//! - Then the peer's blocks from the common height + 1 to the end of that round (or its claimed
//!   height), fetched as headers, each signed by its own key, at the height asked, by a delegate
//!   of that round, and each chaining to the one before it (previous id, height + 1), which the
//!   reference does not check.
//! - The deadline is checked before every request.

use std::sync::Arc;

use heartwood_kernel::Clock;

use crate::errors::PastDeadline;
use crate::peer::{FastVerification, PeerState, SharedPeer, Verification};
use crate::seams::{Deadline, StateCheck, VerifierLink};
use crate::view::{BlockLookup, ChainView};

/// The branching of the common-block search.
pub const N_ARY: u32 = 8;

/// The peer verifier, over the node's chain and stored blocks.
pub struct PeerVerifier {
    #[expect(dead_code, reason = "the verifier's methods read it")]
    chain: Arc<dyn ChainView>,
    #[expect(dead_code, reason = "the verifier's methods read it")]
    lookup: Arc<dyn BlockLookup>,
    #[expect(dead_code, reason = "the verifier's methods read it")]
    clock: Arc<dyn Clock>,
}

impl PeerVerifier {
    /// A verifier over `chain` and `lookup`; `clock` gives the time of the violations it records.
    pub fn new(
        chain: Arc<dyn ChainView>,
        lookup: Arc<dyn BlockLookup>,
        clock: Arc<dyn Clock>,
    ) -> PeerVerifier {
        PeerVerifier {
            chain,
            lookup,
            clock,
        }
    }
}

impl StateCheck for PeerVerifier {
    async fn check_state<L: VerifierLink>(
        &self,
        link: &L,
        peer: &SharedPeer,
        claimed: &PeerState,
        deadline: Deadline,
    ) -> Result<Option<Verification>, PastDeadline> {
        let _ = (link, peer, claimed, deadline);
        todo!("checkState")
    }

    async fn check_state_fast(
        &self,
        peer: &SharedPeer,
        claimed: &PeerState,
    ) -> Option<FastVerification> {
        let _ = (peer, claimed);
        todo!("checkStateFast")
    }
}
