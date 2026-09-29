//! The peer verifier (`checkState`, `checkStateFast`).
//!
//! - The claimed header: its height must be the state's height. Below our height it must be
//!   signed by a delegate of its round; at or above our height its own signature suffices
//!   (reproduced). The reference first compares the header with our own block at that height as
//!   JSON; a header decoded from the wire has its keys in another order than our block's, so that
//!   shortcut only saves work, and its answer is always the one of the checks it skips.
//! - A peer whose header is our block at that height is verified at once, with the common height
//!   its height.
//! - Otherwise the highest common block: an 8-ary search ([`crate::nsect::NSect`]) over
//!   `[1, min(theirs, ours)]`, each probe sending our ids at 9 heights; a reply naming an id we did
//!   not ask for, or at another height, ends the search.
//! - Then the peer's blocks from the common height + 1 to the end of that round (or its claimed
//!   height), fetched as headers, each rebuilt as `BlockFactory.fromData` rebuilds it, signed by
//!   its own key, at the height asked, by a delegate of that round, and each chaining to the one
//!   before it (the first to our common block), which the reference does not check. Nor does it
//!   check that the first differs from our block at its height: a peer on our chain could answer
//!   a probe with a low block and pass as forked there. Such a segment is refused
//!   ([`Refusal::OwnSegment`]), without a violation.
//! - The deadline is checked before every request.
//!
//! The reference ends a verification in one of three ways, and [`Outcome`] keeps them apart: the
//! chain verifies, it does not (`undefined`), or the verification throws. It throws when the
//! deadline has passed before a common-block probe, when a header of the fork segment cannot be
//! rebuilt as a block or its generator is no registered delegate, and when the node cannot read
//! its own chain; a deadline passed before a fetch of the fork segment is caught and does not
//! verify. A throw fails the ping and leaves the peer's last verification as it was;
//! [`StateCheck::check_state`] gives that with [`VerificationStopped`], which names what stopped
//! it. The fast verification throws only when the node cannot read its own chain.
//!
//! A bogus common-block reply and a header of the fork segment that fails its checks are protocol
//! violations that no honest peer commits; each is recorded on the peer.

use std::sync::{Arc, Mutex, PoisonError};

use heartwood_crypto::blocks::serialiser::{block_id, signing_hash};
use heartwood_crypto::crypto::sig::{self, SchemeId, Signature, SigningDomain};
use heartwood_crypto::{BlockHeader, BlockId, PublicKeyBytes};
use heartwood_kernel::{Clock, calculate_round};
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;

use crate::errors::VerificationStopped;
use crate::nsect::NSect;
use crate::peer::{FastVerification, PeerState, SharedPeer, Verification};
use crate::seams::{Deadline, StateCheck, VerifierLink};
use crate::view::{BlockLookup, ChainView};

/// The branching of the common-block search.
pub const N_ARY: u32 = 8;
/// The most headers one fetch of the fork segment asks for.
pub const MAX_HEADERS: u32 = 400;

/// How a full verification ended, as the reference's `checkState` ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The chain verified.
    Verified(Verification),
    /// The chain did not verify (the reference answers `undefined`).
    Refused(Refusal),
    /// The verification stopped (the reference throws).
    Stopped(VerificationStopped),
}

/// Why a chain did not verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The claimed header is not at the state's height, or is not signed as it must be.
    Header,
    /// The first probe found no common block, or a probe had a bogus reply before any match.
    NoCommonBlock,
    /// The deadline passed before a fetch of the fork segment, or the peer sent no header.
    NoHeaders,
    /// A header of the fork segment is not signed by its key, not at the height asked, or not
    /// signed by a delegate of the round.
    Segment,
    /// A header of the fork segment does not chain to the block before it; the reference does not
    /// check this.
    Unlinked,
    /// The fork segment's first header is our own block at its height, so the common block found
    /// is not the peer's highest one; the reference verifies the peer as forked there.
    OwnSegment,
}

/// The peer verifier, over the node's chain and stored blocks.
pub struct PeerVerifier {
    chain: Arc<dyn ChainView>,
    lookup: Arc<dyn BlockLookup>,
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

    /// The full verification of `claimed`, the state `peer` reported, with the requests made
    /// through `link` before `deadline`.
    pub async fn verify<L: VerifierLink>(
        &self,
        link: &L,
        peer: &SharedPeer,
        claimed: &PeerState,
        deadline: Deadline,
    ) -> Outcome {
        if !self.header_verifies(claimed) {
            return Outcome::Refused(Refusal::Header);
        }
        let theirs = claimed.height;
        let ours = self.our_height();
        let verified = |common| {
            Outcome::Verified(Verification {
                my_height: ours,
                their_height: theirs,
                highest_common_height: common,
            })
        };
        match self.have_their_block(claimed, ours) {
            Err(stop) => return Outcome::Stopped(stop),
            Ok(true) => return verified(theirs),
            Ok(false) => {}
        }
        let common = match self
            .highest_common(link, peer, theirs.min(ours), deadline)
            .await
        {
            Err(stop) => return Outcome::Stopped(stop),
            Ok(None) => return Outcome::Refused(Refusal::NoCommonBlock),
            Ok(Some(common)) => common,
        };
        match self
            .verify_segment(link, peer, common, theirs, deadline)
            .await
        {
            Ok(()) => verified(common.0),
            Err(end) => end,
        }
    }

    /// The fast verification of `claimed`: the header only, and whether it is our block at its
    /// height; [`VerificationStopped::Lookup`] when our block cannot be read, where the reference
    /// throws.
    pub fn verify_fast(
        &self,
        claimed: &PeerState,
    ) -> Result<Option<FastVerification>, VerificationStopped> {
        if !self.header_verifies(claimed) {
            return Ok(None);
        }
        let ours = self.our_height();
        let have = self.have_their_block(claimed, ours)?;
        Ok(Some(FastVerification {
            my_height: ours,
            their_height: claimed.height,
            highest_common_height: have.then_some(claimed.height),
        }))
    }

    fn our_height(&self) -> u32 {
        self.chain.last_block().height()
    }

    /// Record a protocol violation of `peer` now.
    fn violation(&self, peer: &SharedPeer) {
        let now = self.clock.now_ms();
        peer.with(|peer| peer.add_violation(now));
    }

    /// `checkStateHeader`: every error in it is caught and does not verify.
    fn header_verifies(&self, claimed: &PeerState) -> bool {
        let header = &claimed.header;
        let Some(height) = header.height.filter(|&height| height == claimed.height) else {
            return false;
        };
        let Some(rebuilt) = rebuild(&status_fields(claimed)) else {
            return false;
        };
        if height < self.our_height() {
            let round = calculate_round(height, self.chain.milestones());
            let Ok(delegates) = self.lookup.round_delegate_keys(round) else {
                return false;
            };
            self.header_passes(&rebuilt, height, &delegates) == Ok(true)
        } else {
            signed(&rebuilt)
        }
    }

    /// `weHavePeersHighestBlock`: whether the claimed header is our block at its height.
    fn have_their_block(
        &self,
        claimed: &PeerState,
        ours: u32,
    ) -> Result<bool, VerificationStopped> {
        if claimed.height > ours {
            return Ok(false);
        }
        let blocks = self
            .lookup
            .blocks_by_height(&[claimed.height])
            .map_err(|_| VerificationStopped::Lookup)?;
        let [block] = blocks.as_slice() else {
            return Err(VerificationStopped::Lookup);
        };
        Ok(claimed.header_id() == Some(block_id(block).to_hex().as_str()))
    }

    /// The highest common block, and our id at its height, by the n-ary search over `[1, high]`.
    async fn highest_common<L: VerifierLink>(
        &self,
        link: &L,
        peer: &SharedPeer,
        high: u32,
        deadline: Deadline,
    ) -> Result<Option<(u32, BlockId)>, VerificationStopped> {
        let last_match = Mutex::new(None);
        let height = NSect::new(N_ARY)
            .find(1, high, |heights| {
                self.probe(link, peer, heights, deadline, &last_match)
            })
            .await?;
        let id = last_match
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(height.zip(id))
    }

    /// One probe: our ids at `heights`, and the peer's highest common block among them, when its
    /// reply names one of them at its height. The id of a match goes to `last_match`.
    async fn probe<L: VerifierLink>(
        &self,
        link: &L,
        peer: &SharedPeer,
        heights: Vec<u32>,
        deadline: Deadline,
        last_match: &Mutex<Option<BlockId>>,
    ) -> Result<Option<u32>, VerificationStopped> {
        let headers = self
            .lookup
            .blocks_by_height(&heights)
            .map_err(|_| VerificationStopped::Lookup)?;
        let ours: Vec<(u32, BlockId)> = headers
            .iter()
            .map(|header| (header.height, block_id(header)))
            .collect();
        let complete = ours.len() == heights.len()
            && heights
                .iter()
                .all(|height| ours.iter().any(|(ours, _)| ours == height));
        if !complete {
            return Err(VerificationStopped::Lookup);
        }
        let remaining = deadline
            .remaining()
            .map_err(|_| VerificationStopped::Deadline)?;
        let ids: Vec<BlockId> = ours.iter().map(|(_, id)| *id).collect();
        let Some(common) = link.common_blocks(peer, &ids, remaining).await else {
            return Ok(None);
        };
        // The reply's id is looked up as text among our ids, which are lowercase.
        match ours.iter().find(|(_, id)| id.to_hex() == common.id) {
            Some((height, id)) if *height == common.height => {
                *last_match.lock().unwrap_or_else(PoisonError::into_inner) = Some(*id);
                Ok(Some(common.height))
            }
            _ => {
                self.violation(peer);
                Ok(None)
            }
        }
    }

    /// `verifyPeerBlocks` from the height after `common` to the end of its round or `claimed`,
    /// with the headers of each height checked in order and fetched when first needed.
    async fn verify_segment<L: VerifierLink>(
        &self,
        link: &L,
        peer: &SharedPeer,
        (common, common_id): (u32, BlockId),
        claimed: u32,
        deadline: Deadline,
    ) -> Result<(), Outcome> {
        let Some(start) = common.checked_add(1) else {
            return Ok(());
        };
        let round = calculate_round(start, self.chain.milestones());
        let delegates = self
            .lookup
            .round_delegate_keys(round)
            .map_err(|_| Outcome::Stopped(VerificationStopped::Lookup))?;
        let last_in_round = round
            .round_height
            .saturating_add(round.max_delegates)
            .saturating_sub(1);
        let end = u32::try_from(last_in_round.min(u64::from(claimed))).unwrap_or(claimed);
        // The headers from `start`, by position in the replies, as the reference keeps them.
        let mut fetched: Vec<ListedBlock> = Vec::new();
        let mut previous = common_id;
        for height in start..=end {
            let index = usize::try_from(height.saturating_sub(start)).unwrap_or(usize::MAX);
            if fetched.len() <= index {
                if deadline.remaining().is_err() {
                    return Err(Outcome::Refused(Refusal::NoHeaders));
                }
                let limit = end
                    .saturating_sub(height)
                    .saturating_add(1)
                    .clamp(1, MAX_HEADERS);
                let headers = link.headers(peer, height.saturating_sub(1), limit).await;
                if headers.is_empty() {
                    return Err(Outcome::Refused(Refusal::NoHeaders));
                }
                fetched.extend(headers.into_iter().map(|block| block.header));
            }
            let Some(listed) = fetched.get(index) else {
                return Err(Outcome::Refused(Refusal::NoHeaders));
            };
            let outcome = match rebuild(&listed_fields(listed)) {
                None => Outcome::Stopped(VerificationStopped::Header),
                Some(rebuilt) => match self.header_passes(&rebuilt, height, &delegates) {
                    Err(stop) => Outcome::Stopped(stop),
                    Ok(false) => Outcome::Refused(Refusal::Segment),
                    Ok(true) if BlockId::from_array(rebuilt.previous_block) != previous => {
                        Outcome::Refused(Refusal::Unlinked)
                    }
                    Ok(true) => {
                        previous = block_id(&rebuilt);
                        if height == start && self.is_ours(height, previous)? {
                            return Err(Outcome::Refused(Refusal::OwnSegment));
                        }
                        continue;
                    }
                },
            };
            self.violation(peer);
            return Err(outcome);
        }
        Ok(())
    }

    /// Whether `id` is the id of our block at `height`, which is none above our tip. Only a peer
    /// that answered a common-block probe with a block below its highest common one shows us our
    /// own block as the first of its fork segment: a peer on our chain that claims a fork, or an
    /// honest peer whose later probe failed, so it is refused without a violation.
    fn is_ours(&self, height: u32, id: BlockId) -> Result<bool, Outcome> {
        if height > self.our_height() {
            return Ok(false);
        }
        let stopped = Outcome::Stopped(VerificationStopped::Lookup);
        let ours = self
            .lookup
            .blocks_by_height(&[height])
            .map_err(|_| stopped)?;
        Ok(ours.first().is_some_and(|ours| block_id(ours) == id))
    }

    /// `verifyPeerBlock` after the rebuild: the generator must be a registered delegate (the
    /// reference throws otherwise), then the signature, the height asked and the round's
    /// delegates.
    fn header_passes(
        &self,
        header: &BlockHeader,
        expected_height: u32,
        delegates: &[PublicKeyBytes],
    ) -> Result<bool, VerificationStopped> {
        // The reference names the generator only for a version 0 block, and requires a name.
        if header.version != 0
            || self
                .chain
                .delegate_by_key(&header.generator_public_key)
                .is_none()
        {
            return Err(VerificationStopped::Header);
        }
        Ok(signed(header)
            && header.height == expected_height
            && delegates.contains(&header.generator_public_key))
    }
}

impl StateCheck for PeerVerifier {
    async fn check_state<L: VerifierLink>(
        &self,
        link: &L,
        peer: &SharedPeer,
        claimed: &PeerState,
        deadline: Deadline,
    ) -> Result<Option<Verification>, VerificationStopped> {
        match self.verify(link, peer, claimed, deadline).await {
            Outcome::Verified(verification) => Ok(Some(verification)),
            Outcome::Refused(_) => Ok(None),
            Outcome::Stopped(stop) => Err(stop),
        }
    }

    async fn check_state_fast(
        &self,
        _peer: &SharedPeer,
        claimed: &PeerState,
    ) -> Result<Option<FastVerification>, VerificationStopped> {
        self.verify_fast(claimed)
    }
}

/// The fields of a header as the reply check left them, for [`rebuild`].
struct Fields<'a> {
    version: Option<u32>,
    timestamp: Option<u32>,
    height: Option<u32>,
    previous_block: Option<&'a str>,
    number_of_transactions: Option<u32>,
    amounts: [Amount; 3],
    payload_length: Option<u32>,
    payload_hash: Option<&'a str>,
    generator_public_key: Option<&'a str>,
    block_signature: Option<&'a str>,
}

/// The [`Fields`] of a status or listed header (both have the reply's field names) and its
/// amounts.
macro_rules! fields {
    ($header:expr, $amounts:expr) => {
        Fields {
            version: $header.version,
            timestamp: $header.timestamp,
            height: $header.height,
            previous_block: $header.previous_block.as_deref(),
            number_of_transactions: $header.number_of_transactions,
            amounts: $amounts,
            payload_length: $header.payload_length,
            payload_hash: $header.payload_hash.as_deref(),
            generator_public_key: $header.generator_public_key.as_deref(),
            block_signature: $header.block_signature.as_deref(),
        }
    };
}

fn status_fields(claimed: &PeerState) -> Fields<'_> {
    fields!(
        claimed.header,
        [claimed.total_amount, claimed.total_fee, claimed.reward]
    )
}

fn listed_fields(listed: &ListedBlock) -> Fields<'_> {
    fields!(
        listed.header,
        [listed.total_amount, listed.total_fee, listed.reward]
    )
}

/// The header `BlockFactory.fromData` makes of a header without transactions, or `None` where it
/// throws. It writes the fields out and reads them back: an absent version, transaction count or
/// payload length is written as 0; each hex field is read as Node reads hex (pairs of digits, an
/// odd last digit dropped) and must then have its length, and an absent one throws; an amount
/// must fit 64 bits. The header's own id is not read: the block's id is computed.
fn rebuild(fields: &Fields<'_>) -> Option<BlockHeader> {
    let [total_amount, total_fee, reward] = fields
        .amounts
        .map(|amount| u64::try_from(amount.value()).ok());
    Some(BlockHeader {
        version: fields.version.unwrap_or(0),
        timestamp: fields.timestamp?,
        height: fields.height?,
        previous_block: node_hex(fields.previous_block?)?,
        number_of_transactions: fields.number_of_transactions.unwrap_or(0),
        total_amount: total_amount?,
        total_fee: total_fee?,
        reward: reward?,
        payload_length: fields.payload_length.unwrap_or(0),
        payload_hash: node_hex(fields.payload_hash?)?,
        generator_public_key: PublicKeyBytes::from_array(node_hex(fields.generator_public_key?)?),
        block_signature: Signature::from_array(node_hex(fields.block_signature?)?),
    })
}

/// The `N` bytes of `text` as Node's `Buffer.from(text, "hex")` reads them, when it reads `N`:
/// pairs of digits of either case up to the first pair that is not one.
fn node_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let mut bytes = Vec::with_capacity(N);
    let digit = |byte: u8| char::from(byte).to_digit(16);
    for &[high, low] in text.as_bytes().as_chunks::<2>().0 {
        let (Some(high), Some(low)) = (digit(high), digit(low)) else {
            break;
        };
        bytes.push(u8::try_from(high.wrapping_shl(4) | low).ok()?);
    }
    bytes.try_into().ok()
}

/// Whether the header's signature is a valid BIP340 signature by its generator key.
fn signed(header: &BlockHeader) -> bool {
    sig::verify(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &signing_hash(header),
        &header.block_signature,
        header.generator_public_key.as_bytes(),
    )
}
