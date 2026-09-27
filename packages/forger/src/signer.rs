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
        let delegate = (self.delegates.iter()).find(|delegate| delegate.public_key() == key);
        let unknown = || SignError::UnknownKey {
            public_key: key.to_hex(),
        };
        let delegate = delegate.ok_or_else(unknown)?;
        let name = key.to_hex();
        let refused = match self.record.decide(key, slot, own_slot) {
            Decision::OutOfWindow { own_slot } => SignError::OutOfWindow { slot, own_slot },
            Decision::Refuse { recorded_slot } => SignError::Refused {
                slot,
                recorded_slot,
            },
            Decision::Resend(signed) => {
                let (id, height) = (&signed.id, signed.height);
                info!("Delegate {name} sends block {id} at height {height} of slot {slot} again");
                return Ok(SignedOutcome::Resent(signed));
            }
            Decision::SignNew => {
                let block = delegate.forge(inputs, self.aux, &self.milestones)?;
                let (height, id, bytes) = (block.height(), *block.id(), block.encode());
                let signed = Signed {
                    height,
                    slot,
                    id,
                    bytes,
                };
                self.record.record(key, signed.clone())?;
                return Ok(SignedOutcome::New(signed));
            }
        };
        warning!("Delegate {name} will not sign: {refused}");
        Err(refused)
    }
}
