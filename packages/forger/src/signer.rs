//! The block signer: the delegates' keys, the signing record and the block behind one call. It
//! assembles and signs only after the loop's checks, and the bytes are those the reference signs
//! for the same inputs and aux.
//!
//! A new block is recorded before it is returned, so it never leaves the forger unrecorded. The
//! recorded slot gives back the recorded bytes, whatever the inputs are now; a slot below the
//! recorded one, or more than one away from the forger's own slot, is refused with a line.

use heartwood_crypto::{Aux, Milestones, PublicKeyBytes};
use heartwood_kernel::{info, warning};

use crate::delegate::Delegate;
use crate::errors::SignError;
use crate::seams::{BlockSigner, ForgeInputs, Signed, SignedOutcome};
use crate::signing_record::{Decision, SigningRecord};

/// The production [`BlockSigner`].
pub struct Signer {
    delegates: Vec<Delegate>,
    record: SigningRecord,
    aux: Aux,
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

    /// The signing record.
    pub fn record(&self) -> &SigningRecord {
        &self.record
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
        let delegate = (self.delegates.iter())
            .find(|delegate| delegate.public_key() == key)
            .ok_or_else(|| SignError::UnknownKey {
                public_key: key.to_hex(),
            })?;
        let name = key.to_hex();
        match self.record.decide(key, slot, own_slot) {
            Decision::OutOfWindow { own_slot } => {
                warning!(
                    "Delegate {name} will not sign slot {slot}: this forger's clock is in slot {own_slot}"
                );
                Err(SignError::OutOfWindow { slot, own_slot })
            }
            Decision::Refuse { recorded_slot } => {
                warning!(
                    "Delegate {name} will not sign slot {slot}: it has signed slot {recorded_slot}"
                );
                Err(SignError::Refused {
                    slot,
                    recorded_slot,
                })
            }
            Decision::Resend(signed) => {
                info!(
                    "Delegate {name} sends block {} at height {} again, as signed for slot {slot}",
                    signed.id, signed.height
                );
                Ok(SignedOutcome::Resent(signed))
            }
            Decision::SignNew => {
                let block = delegate.forge(inputs, self.aux, &self.milestones)?;
                let signed = Signed {
                    height: block.height(),
                    slot,
                    id: *block.id(),
                    bytes: block.encode(),
                };
                self.record.record(key, signed.clone())?;
                Ok(SignedOutcome::New(signed))
            }
        }
    }
}
