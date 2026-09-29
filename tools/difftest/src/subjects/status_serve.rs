//! The served status, answered with `heartwood_p2p::status_server`:
//!
//! - `status.serve` `{tip, now, peerConfig, registered, forgerKeys}` -> `{hex, preimage}`: the
//!   getStatus answer's bytes and the status text a delegate signs (see
//!   `tools/oracle/lib/ops/relay.js` for the input).
//!
//! The relay's status server runs over a node whose tip, registered delegates and configuration
//! are the case's, with the wall clock at `now`. A forger with the keys of `forgerKeys` (in file
//! order, duplicates kept) holds the attestation session and signs every offer, as the reference
//! relay signs with the keys of its `delegates.json` that its forger lists; without a key there is
//! no session.

use std::net::IpAddr;
use std::sync::Arc;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{
    Block, BlockHeader, BlockId, KeyPair, Milestones, Network, PublicKeyBytes, SecretKey,
    TransactionId,
};
use heartwood_kernel::{ChainLink, Clock, ManualClock};
use heartwood_p2p::NodeServices;
use heartwood_p2p::attestation_session::AttestationSession;
use heartwood_p2p::errors::LookupError;
use heartwood_p2p::node_services::{Collation, ForgerEvent, NodeUnavailable, QueueState};
use heartwood_p2p::server::seams::SessionId;
use heartwood_p2p::status_server::StatusServer;
use heartwood_p2p::view::{BlockPing, ChainView, DelegateInfo};
use heartwood_p2p_wire::codecs::blocks::BlockListing;
use heartwood_p2p_wire::internal::{
    CurrentRound, StatusSignatures, StatusToSignRequest, StatusToSignResponse,
};
use heartwood_p2p_wire::status::{
    self, NetworkConfig, PeerConfig, PluginConfig, SignedState, StatusHeaderData, TokenConfig,
};
use serde_json::{Value, json};
use tokio::sync::watch;

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of the served status.
pub(super) const OPS: &[(&str, Run)] = &[("status.serve", serve)];

/// A boxed future, as the node's services return them.
type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// The relay: its tip, its registered delegates and its configuration.
struct Relay {
    tip: Arc<Block>,
    registered: Vec<PublicKeyBytes>,
    milestones: Milestones,
    network: Network,
    config: PeerConfig,
    queue: watch::Sender<QueueState>,
}

impl ChainView for Relay {
    fn last_block(&self) -> Arc<Block> {
        Arc::clone(&self.tip)
    }
    fn last_blocks(&self) -> Vec<Arc<Block>> {
        vec![self.last_block()]
    }
    fn milestones(&self) -> &Milestones {
        &self.milestones
    }
    fn network(&self) -> &Network {
        &self.network
    }
    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo> {
        let rank = self.registered.iter().position(|known| known == key)?;
        let username = format!("delegate_{rank}");
        let rank = u32::try_from(rank).ok()?.checked_add(1);
        Some(DelegateInfo { username, rank })
    }
    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        self.registered.clone()
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

/// The relay's services as the status server reads them; nothing else is asked of them.
struct Services(Arc<Relay>);

impl NodeServices for Services {
    fn is_booted(&self) -> bool {
        true
    }
    fn first_synced_ms(&self) -> Option<i64> {
        None
    }
    fn queue_state(&self) -> watch::Receiver<QueueState> {
        self.0.queue.subscribe()
    }
    fn chain(&self) -> Arc<dyn ChainView> {
        self.0.clone()
    }
    fn last_downloaded_block(&self) -> ChainLink {
        ChainLink::of(&self.0.tip)
    }
    fn ping_block(&self, _: &BlockHeader) -> bool {
        false
    }
    fn handle_incoming_block(&self, _: Block, _: bool, _: IpAddr) {}
    fn delegate_rank(&self, _: &str) -> Option<u32> {
        None
    }
    fn already_forged(&self, _: u32, _: &str) -> Result<bool, LookupError> {
        Ok(false)
    }
    fn active_keys_next_round(&self) -> Result<Vec<PublicKeyBytes>, LookupError> {
        Ok(Vec::new())
    }
    fn blocks_for_download(
        &self,
        _: u32,
        _: u32,
        _: bool,
    ) -> Result<Vec<BlockListing>, LookupError> {
        Ok(Vec::new())
    }
    fn common_blocks(&self, _: &[BlockId], _: usize) -> Result<Vec<BlockHeader>, LookupError> {
        Ok(Vec::new())
    }
    fn pool_size(&self) -> usize {
        0
    }
    fn collate(
        &self,
        _: bool,
        _: Vec<TransactionId>,
    ) -> BoxFuture<'_, Result<Collation, NodeUnavailable>> {
        Box::pin(async { Err(NodeUnavailable) })
    }
    fn post_transactions(
        &self,
        _: Vec<Vec<u8>>,
    ) -> BoxFuture<'_, Result<Vec<TransactionId>, NodeUnavailable>> {
        Box::pin(async { Err(NodeUnavailable) })
    }
    fn current_round(&self, _: i64) -> Result<CurrentRound, LookupError> {
        Err(LookupError("no round".into()))
    }
    fn slot_number(&self, _: Option<i64>, _: i64) -> i64 {
        0
    }
    fn force_wakeup(&self) {}
    fn forger_event(&self, _: ForgerEvent) {}
    fn peer_config(&self) -> PeerConfig {
        self.0.config.clone()
    }
}

/// A status configuration as the oracle's input writes it.
fn peer_config(config: &Value) -> Outcome<PeerConfig> {
    let network = &config["network"];
    let text = |value: &Value, name: &str| {
        (value.as_str().map(str::to_owned)).ok_or_else(|| failed(format!("peerConfig.{name}")))
    };
    let plugins = config["plugins"]
        .as_object()
        .ok_or_else(|| failed("peerConfig.plugins"))?;
    let mut listed = Vec::new();
    for (name, plugin) in plugins {
        let port = plugin["port"].as_u64().and_then(|p| u32::try_from(p).ok());
        let plugin = PluginConfig {
            enabled: plugin["enabled"].as_bool() == Some(true),
            port: port.ok_or_else(|| failed(format!("the port of {name}")))?,
            estimate_total_count: plugin.get("estimateTotalCount").and_then(Value::as_bool),
        };
        listed.push((name.clone(), plugin));
    }
    let version = network["version"]
        .as_u64()
        .and_then(|v| u8::try_from(v).ok());
    Ok(PeerConfig {
        version: text(&config["version"], "version")?,
        network: NetworkConfig {
            version: version.ok_or_else(|| failed("peerConfig.network.version"))?,
            name: text(&network["name"], "network.name")?,
            nethash: text(&network["nethash"], "network.nethash")?,
            explorer: text(&network["explorer"], "network.explorer")?,
            token: TokenConfig {
                name: text(&network["token"]["name"], "token.name")?,
                symbol: text(&network["token"]["symbol"], "token.symbol")?,
            },
        },
        plugins: listed,
    })
}

/// Run `future` to its end on a runtime of this thread.
fn block_on<F: std::future::Future>(future: F) -> Outcome<F::Output> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| failed(format!("no tokio runtime: {e}")))?;
    Ok(runtime.block_on(future))
}

/// A forger on the session of `status` that signs every offer with `keys`.
async fn sign_offers(session: Arc<AttestationSession>, keys: Vec<KeyPair>) {
    let declared = keys
        .iter()
        .map(|key| key.public_key().to_compressed().to_hex());
    let request = StatusToSignRequest {
        public_keys: declared.collect(),
    };
    let forger = IpAddr::from([127, 0, 0, 1]);
    let id = SessionId(1);
    while let Ok(answer) = session.status_to_sign(id, forger, request.clone()).await {
        let StatusToSignResponse::Offer {
            status,
            public_keys,
            challenge,
        } = answer
        else {
            continue;
        };
        let proof = status::session_proof(&challenge).unwrap_or_default();
        let (mut signatures, mut proofs) = (Vec::new(), Vec::new());
        for offered in &public_keys {
            let key = keys
                .iter()
                .find(|key| key.public_key().to_compressed().to_hex() == *offered);
            let sign = |text: &[u8]| key.and_then(|key| status::attest(text, key).ok());
            signatures.extend(sign(status.as_bytes()).map(|signed| signed.signature.to_hex()));
            proofs.extend(sign(&proof).map(|signed| signed.signature.to_hex()));
        }
        let offer = hex::encode(&sha256(status.as_bytes()));
        let posted = StatusSignatures {
            offer,
            signatures,
            proofs,
        };
        let _ = session.post_signatures(id, posted);
    }
}

fn serve(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let devnet = env.devnet()?;
    let milestones = env.milestones(case)?.into_owned();
    let bytes = super::hex_bytes(&input["tip"], "hex")?.ok_or_else(|| failed("tip.hex"))?;
    let tip = Block::decode(&bytes, &milestones).map_err(|e| failed(format!("the tip: {e}")))?;
    let key = |value: &Value| {
        let text = value.as_str().ok_or_else(|| failed("a key is not text"))?;
        PublicKeyBytes::from_hex(text).map_err(|e| failed(format!("{text}: {e}")))
    };
    let registered = (input["registered"].as_array())
        .ok_or_else(|| failed("input.registered"))?
        .iter()
        .map(key)
        .collect::<Outcome<_>>()?;
    let mut keys = Vec::new();
    for secret in input["forgerKeys"]
        .as_array()
        .ok_or_else(|| failed("input.forgerKeys"))?
    {
        let secret = secret
            .as_str()
            .ok_or_else(|| failed("a forger key is not text"))?;
        let secret = SecretKey::from_hex(secret).map_err(|e| failed(format!("{e}")))?;
        keys.push(KeyPair::from_secret_key(secret));
    }
    let now = input["now"].as_i64().ok_or_else(|| failed("input.now"))?;
    let config = peer_config(&input["peerConfig"])?;
    let relay = Arc::new(Relay {
        tip: Arc::new(tip),
        registered,
        milestones,
        network: devnet.network.clone(),
        config,
        queue: watch::channel(QueueState::default()).0,
    });
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(now));
    let node: Arc<dyn NodeServices> = Arc::new(Services(Arc::clone(&relay)));
    let session = Arc::new(AttestationSession::new(
        Arc::clone(&node),
        Arc::clone(&clock),
    ));
    let server = StatusServer::new(node, Arc::clone(&session), clock);
    let served = block_on(async {
        let forger =
            (!keys.is_empty()).then(|| tokio::spawn(sign_offers(Arc::clone(&session), keys)));
        tokio::task::yield_now().await;
        let served = server.get_status().await;
        forger.inspect(tokio::task::JoinHandle::abort);
        served
    })?;
    let served = served.map_err(|refusal| failed(format!("getStatus refused: {refusal:?}")))?;
    // The text a delegate signs, from the same tip, slot and configuration.
    let first = relay.milestones.at(1);
    let epoch = slots::epoch_unix_ms(first.epoch()).unwrap_or_default();
    let slot = slots::slot_info(slots::epoch_time(now, epoch), first.block_time());
    let state = SignedState {
        height: relay.tip.height(),
        forging_allowed: slot.forging_status,
        current_slot: slot.slot_number,
        header: StatusHeaderData::from_block(&relay.tip),
    };
    let preimage = status::signing_preimage(&state, &relay.config);
    Ok(Verdict::Output(json!({
        "hex": hex::encode(&served),
        "preimage": String::from_utf8_lossy(&preimage),
    })))
}
