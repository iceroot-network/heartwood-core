//! The block signer: the delegates' keys, the signing record and the block behind one call. It
//! assembles and signs only after the loop's checks, and the bytes are those the reference signs
//! for the same inputs and aux.

use heartwood_crypto::{Aux, Milestones, PublicKeyBytes};

use crate::delegate::Delegate;
use crate::errors::SignError;
use crate::seams::{BlockSigner, ForgeInputs, SignedOutcome};
use crate::signing_record::SigningRecord;

/// The production [`BlockSigner`].
pub struct Signer {
    #[expect(dead_code, reason = "the signer reads it")]
    delegates: Vec<Delegate>,
    #[expect(dead_code, reason = "the signer reads it")]
    record: SigningRecord,
    #[expect(dead_code, reason = "the signer reads it")]
    aux: Aux,
    #[expect(dead_code, reason = "the signer reads it")]
    milestones: Milestones,
}

impl Signer {
    /// A signer for `delegates`, through `record`, with the block signatures' `aux` and the
    /// forger's milestones.
    pub fn new(
        delegates: Vec<Delegate>,
        record: SigningRecord,
        aux: Aux,
        milestones: Milestones,
    ) -> Signer {
        Signer {
            delegates,
            record,
            aux,
            milestones,
        }
    }
}

impl BlockSigner for Signer {
    fn sign_or_resend(
        &mut self,
        key: &PublicKeyBytes,
        inputs: &ForgeInputs<'_>,
        own_slot: i64,
        slot: i64,
    ) -> Result<SignedOutcome, SignError> {
        let _ = (key, inputs, own_slot, slot);
        todo!("the block signer")
    }
}
