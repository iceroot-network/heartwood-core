//! Wallet fixtures: the input of the state operations of L1 (`state.applyTransaction`,
//! `state.digest`) and of the `state_apply` fuzz target.
//!
//! A fixture is a list of wallets in the shape of the digest's `#W` records, without the values
//! both sides derive (`vb`, `vc` and `rk`: vote balances and voter counts come from the voters,
//! and a fixture has no ranks), plus `resignedAt`, the height of a resigned delegate's latest
//! resignation. The oracle builds the reference's wallets and history from the same JSON.

use heartwood_crypto::BlockId;
use heartwood_crypto::identities::{Address, PublicKey, PublicKeyBytes};
use heartwood_crypto::transactions::types::Votes;
use heartwood_state::wallets::{Resignation, WalletRepository};
use serde_json::Value;

/// A wallet fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    /// The wallets, in the order they are created.
    pub wallets: Vec<FixtureWallet>,
}

/// One wallet of a fixture (`a`, `pk`, `bal`, `n`, `spk`, and the delegate and vote keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureWallet {
    /// `a`.
    pub address: Address,
    /// `pk`.
    pub public_key: Option<PublicKey>,
    /// `bal`.
    pub balance: u64,
    /// `n`.
    pub nonce: u64,
    /// `spk`.
    pub second_public_key: Option<PublicKeyBytes>,
    /// The delegate keys, when `u` is present.
    pub delegate: Option<FixtureDelegate>,
    /// `v`, in the order they are kept.
    pub votes: Votes,
}

/// The delegate record of a fixture wallet (`u`, `rs`, `pb`, `ff`, `fr`, `bf`, `dn`, `lb`, and
/// `resignedAt`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureDelegate {
    /// `u`.
    pub username: String,
    /// `rs`.
    pub resigned: Option<Resignation>,
    /// `resignedAt`.
    pub resigned_at: Option<u32>,
    /// `pb`.
    pub produced_blocks: u64,
    /// `ff`.
    pub forged_fees: u64,
    /// `fr`.
    pub forged_rewards: u64,
    /// `bf`.
    pub burned_fees: u64,
    /// `dn`.
    pub donations: u64,
    /// `lb`.
    pub last_block: Option<BlockId>,
}

impl Fixture {
    /// The fixture in `value`, the JSON the oracle reads; a message when it is malformed.
    pub fn from_json(value: &Value) -> Result<Fixture, String> {
        let _ = value;
        todo!()
    }

    /// The fixture as the JSON the oracle reads.
    pub fn to_json(&self) -> Value {
        todo!()
    }

    /// The fixture as a wallet repository of the network with address byte `network`: the
    /// wallets, then the vote balances and voter counts built from the voters. A message when the
    /// fixture cannot be built (for example, a vote for a name no fixture delegate has).
    pub fn to_wallets(&self, network: u8) -> Result<WalletRepository, String> {
        let _ = network;
        todo!()
    }
}
