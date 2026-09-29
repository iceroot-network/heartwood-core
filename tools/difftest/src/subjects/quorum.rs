//! The network state a relay gives its forger, answered with `heartwood_p2p::network_state`:
//!
//! - `quorum.analyze` `{tip, delegates, active, peers, declared, forgerKeys, now, coldStart,
//!   minimumDelegateReach?}` -> `{json, canForge, peerKeys}`: the getNetworkState answer's text,
//!   `canForge` at the case's milestone, and each peer's keys once the delegates on this node are
//!   taken from it (see `tools/oracle/lib/ops/relay.js` for the input).
//!
//! The delegates on this node are the attested set a forger's session gives: the declared keys
//! (`declared`, private keys in file order, duplicates kept) that the forger lists (`forgerKeys`)
//! and that are registered delegates, as a reference relay signs with the keys of its
//! `delegates.json` that its forger lists; the analysis then keeps those of the round's set.

use std::collections::HashMap;
use std::sync::Arc;

use heartwood_crypto::{Block, KeyPair, Milestones, Network, PublicKeyBytes, SecretKey};
use heartwood_p2p::network_state::{AnalyzeInput, analyze};
use heartwood_p2p::peer::{FastVerification, Peer, PeerState};
use heartwood_p2p::view::{BlockPing, ChainView, DelegateInfo};
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::internal;
use heartwood_p2p_wire::messages::peer::StatusHeader;
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed, text};
use crate::subject::{Case, Verdict};

/// The operations of the network state.
pub(super) const OPS: &[(&str, Run)] = &[("quorum.analyze", analyze_case)];

/// The relay's chain as a case gives it: its tip and its registered delegates.
struct Registered<'a> {
    tip: Arc<Block>,
    delegates: HashMap<PublicKeyBytes, DelegateInfo>,
    milestones: &'a Milestones,
    network: &'a Network,
}

impl ChainView for Registered<'_> {
    fn last_block(&self) -> Arc<Block> {
        Arc::clone(&self.tip)
    }
    fn last_blocks(&self) -> Vec<Arc<Block>> {
        vec![self.last_block()]
    }
    fn milestones(&self) -> &Milestones {
        self.milestones
    }
    fn network(&self) -> &Network {
        self.network
    }
    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo> {
        self.delegates.get(key).cloned()
    }
    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        self.delegates.keys().copied().collect()
    }
    fn is_synced(&self) -> bool {
        true
    }
    fn is_idle(&self) -> bool {
        true
    }
    fn block_ping(&self) -> Option<BlockPing> {
        None
    }
}

fn list<'a>(value: &'a Value, name: &str) -> Outcome<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| failed(format!("input.{name} is not a list")))
}

fn public_key(value: &Value) -> Outcome<PublicKeyBytes> {
    let text = value
        .as_str()
        .ok_or_else(|| failed("a public key is not text"))?;
    PublicKeyBytes::from_hex(text).map_err(|e| failed(format!("the public key {text}: {e}")))
}

fn public_keys(value: &Value, name: &str) -> Outcome<Vec<PublicKeyBytes>> {
    list(value, name)?.iter().map(public_key).collect()
}

fn number<T: TryFrom<u64>>(value: &Value) -> Option<T> {
    value.as_u64().and_then(|n| T::try_from(n).ok())
}

/// A peer's state header: the fields it has, its amounts as text.
fn header(value: &Value) -> Outcome<(StatusHeader, [Amount; 3])> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let count = |key: &str| value.get(key).and_then(number::<u32>);
    let amount = |key: &str| {
        let text = value.get(key).and_then(Value::as_str).unwrap_or_default();
        Amount::parse(text).map_err(|e| failed(format!("header.{key}: {e:?}")))
    };
    let header = StatusHeader {
        id: text("id"),
        version: count("version"),
        timestamp: count("timestamp"),
        previous_block: text("previousBlock"),
        height: count("height"),
        number_of_transactions: count("numberOfTransactions"),
        total_amount: text("totalAmount"),
        total_fee: text("totalFee"),
        reward: text("reward"),
        payload_length: count("payloadLength"),
        payload_hash: text("payloadHash"),
        generator_public_key: text("generatorPublicKey"),
        block_signature: text("blockSignature"),
        ..StatusHeader::default()
    };
    Ok((
        header,
        [
            amount("totalAmount")?,
            amount("totalFee")?,
            amount("reward")?,
        ],
    ))
}

/// A peer of a case: its address, its keys, its state and its fast verification.
fn peer(value: &Value) -> Outcome<Peer> {
    let address = text(value, "ip")?;
    let ip = address
        .parse()
        .map_err(|_| failed(format!("{address} is not an address")))?;
    let mut peer = Peer::new(ip, 4002);
    peer.public_keys = public_keys(&value["publicKeys"], "peers.publicKeys")?;
    if let Some(state) = value["state"].as_object() {
        let (header, [total_amount, total_fee, reward]) = header(&state["header"])?;
        let height = number(&state["height"]).ok_or_else(|| failed("a state height"))?;
        let slot = state["currentSlot"]
            .as_i64()
            .ok_or_else(|| failed("a state slot"))?;
        peer.state = Some(PeerState {
            height,
            forging_allowed: state["forgingAllowed"].as_bool() == Some(true),
            current_slot: slot,
            header,
            total_amount,
            total_fee,
            reward,
        });
    }
    if let Some(fast) = value["fastVerification"].as_object() {
        let height = |key: &str| number(&fast[key]).ok_or_else(|| failed(format!("fast.{key}")));
        peer.fast_verification = Some(FastVerification {
            my_height: height("myHeight")?,
            their_height: height("theirHeight")?,
            highest_common_height: fast.get("highestCommonHeight").and_then(number),
        });
    }
    Ok(peer)
}

fn analyze_case(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let devnet = env.devnet()?;
    let milestones = env.milestones(case)?;
    let bytes = super::hex_bytes(&input["tip"], "hex")?.ok_or_else(|| failed("tip.hex"))?;
    let tip = Block::decode(&bytes, &milestones).map_err(|e| failed(format!("the tip: {e}")))?;
    let mut delegates = HashMap::new();
    for delegate in list(&input["delegates"], "delegates")? {
        let info = DelegateInfo {
            username: text(delegate, "username")?.to_owned(),
            rank: number(&delegate["rank"]),
        };
        delegates.insert(public_key(&delegate["publicKey"])?, info);
    }
    let chain = Registered {
        tip: Arc::new(tip),
        delegates,
        milestones: &milestones,
        network: &devnet.network,
    };
    let listed = public_keys(&input["forgerKeys"], "forgerKeys")?;
    let mut attested = Vec::new();
    for secret in list(&input["declared"], "declared")? {
        let secret = secret.as_str().ok_or_else(|| failed("a declared key"))?;
        let secret = SecretKey::from_hex(secret).map_err(|e| failed(format!("{e}")))?;
        let key = KeyPair::from_secret_key(secret)
            .public_key()
            .to_compressed();
        if listed.contains(&key) && chain.delegate_by_key(&key).is_some() {
            attested.push(key);
        }
    }
    let peers: Vec<Peer> = list(&input["peers"], "peers")?
        .iter()
        .map(peer)
        .collect::<Outcome<_>>()?;
    let now = input["now"].as_i64().ok_or_else(|| failed("input.now"))?;
    let analysis = analyze(&AnalyzeInput {
        chain: &chain,
        peers: &peers,
        attested: &attested,
        active_next_round: &public_keys(&input["active"], "active")?,
        port: 4002,
        now_ms: now,
        cold_start: input["coldStart"].as_bool() == Some(true),
        minimum_delegate_reach: input.get("minimumDelegateReach").and_then(number),
    });
    let active = milestones.at(super::height(case)?).active_delegates();
    let active = u32::try_from(active).unwrap_or(u32::MAX);
    let peer_keys: Vec<Vec<String>> = peers
        .iter()
        .map(|peer| {
            let kept = (peer.public_keys.iter()).filter(|key| !analysis.on_this_node.contains(key));
            kept.map(PublicKeyBytes::to_hex).collect()
        })
        .collect();
    let json = String::from_utf8_lossy(&internal::encode(&analysis.state)).into_owned();
    Ok(Verdict::Output(json!({
        "json": json,
        "canForge": analysis.state.can_forge(active),
        "peerKeys": peer_keys,
    })))
}
