//! The forger's decisions and the internal routes' bodies:
//!
//! - `forger.decide` `{round, networkState, status, slots, timeLeftInMs, privateKey,
//!   firstAttempt, pool?, fail?, milestones?}` -> `{action, reason, lines}`: one attempt of
//!   `heartwood_forger::forger_service` ([`ForgerService::forge_attempt`]) over a relay that
//!   answers from the input and a signer that records, with the forger's clock leaving
//!   `timeLeftInMs` in the round's slot, and the lines it logs (see
//!   `tools/oracle/lib/ops/forger-decide.js` for the input). An over-height header by the
//!   delegate's own key that is not an empty block and whose signature verifies refuses where the
//!   reference forges: `Documented::OverHeightOwnBlock`.
//! - `internal.networkState` `{status, lastBlock?, lastSlotNumber?, quorumDetails?}` or `{body}`
//!   -> `{json, read, activeDelegates}`: the relay's network state written with
//!   `heartwood_p2p_wire::internal`, and the forger's reading of it;
//! - `internal.currentRound` `{json}` -> `{read}`: the forger's reading of a reference relay's
//!   round.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::{hash, slots};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockId, KeyPair, Milestones, PublicKeyBytes, SecretKey};
use heartwood_forger::errors::{RelayError, SignError};
use heartwood_forger::forger_service::{AttemptEnd, ForgerParts, ForgerService};
use heartwood_forger::options::Options;
use heartwood_forger::seams::{
    BlockSigner, Deadline, ForgeInputs, ForgerEvent, ForgingTransactions, RelayLink, Signed,
    SignedOutcome,
};
use heartwood_kernel::{Clock, ManualClock};
use heartwood_p2p_wire::InternalBodyError;
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::internal::{
    self, CurrentRound, DelegateWallet, InternalBody, NetworkState, NetworkStateStatus,
};
use heartwood_p2p_wire::messages::peer::{GetStatusResponse, State, StatusHeader};
use heartwood_p2p_wire::status::ReceivedStatus;
use serde_json::{Value, json};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use super::{Env, Outcome, Run, failed, text};
use crate::subject::{Case, Documented, SubjectError, Verdict};

/// The operations of the forger's decisions and the internal bodies.
pub(super) const OPS: &[(&str, Run)] = &[
    ("forger.decide", decide),
    ("internal.networkState", network_state),
    ("internal.currentRound", current_round),
];

/// The calls of an attempt, as the oracle's input names them, and their routes.
const CALLS: [(&str, &str); 7] = [
    ("networkState", "p2p.internal.getNetworkState"),
    ("roundSlot", "p2p.internal.getSlotNumber"),
    (
        "transactions",
        "p2p.transactions.getUnconfirmedTransactions",
    ),
    ("status", "p2p.peer.getStatus"),
    ("currentSlot", "p2p.internal.getSlotNumber"),
    ("lastBlockSlot", "p2p.internal.getSlotNumber"),
    ("blockSlot", "p2p.internal.getSlotNumber"),
];
const SLOT_CALLS: [&str; 4] = ["roundSlot", "currentSlot", "lastBlockSlot", "blockSlot"];

/// A relay whose answers are a case's input.
struct Scripted {
    round: CurrentRound,
    state: Result<NetworkState, RelayError>,
    status: ReceivedStatus,
    slots: Mutex<VecDeque<(&'static str, i64)>>,
    pool: ForgingTransactions,
    fail: Option<(String, String)>,
}

impl Scripted {
    fn failing(&self, call: &str) -> Result<(), RelayError> {
        let Some((failed_call, message)) = &self.fail else {
            return Ok(());
        };
        let route = CALLS
            .iter()
            .find(|(name, _)| name == failed_call)
            .map(|(_, route)| *route);
        match (failed_call == call, route) {
            (true, Some(route)) => Err(RelayError::Communication {
                endpoint: format!("127.0.0.1:4002<{route}>"),
                message: message.clone(),
            }),
            _ => Ok(()),
        }
    }
}

impl RelayLink for Scripted {
    async fn get_round(&self) -> Result<CurrentRound, RelayError> {
        Ok(self.round.clone())
    }

    async fn get_slot_number(&self, _: Option<i64>, _: Deadline) -> Result<i64, RelayError> {
        let next = self
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        let (call, slot) = next.unwrap_or(("", 0));
        self.failing(call)?;
        Ok(slot)
    }

    async fn get_network_state(&self, _: bool, _: Deadline) -> Result<NetworkState, RelayError> {
        self.failing("networkState")?;
        self.state.clone()
    }

    async fn get_status(&self, _: Deadline) -> Result<ReceivedStatus, RelayError> {
        self.failing("status")?;
        Ok(self.status.clone())
    }

    async fn get_transactions(
        &self,
        _: Deadline,
    ) -> Result<Option<ForgingTransactions>, RelayError> {
        self.failing("transactions")?;
        Ok(Some(self.pool.clone()))
    }

    async fn broadcast_block(&self, _: &[u8], _: Deadline) {}

    async fn sync_with_network(&self) -> Result<(), RelayError> {
        Ok(())
    }

    async fn emit_event(&self, _: ForgerEvent) {}

    fn dispose(&self) {}
}

/// A signer that gives the block's height for its inputs, and nothing to post.
struct Unsigned;

impl BlockSigner for Unsigned {
    fn sign_or_resend(
        &mut self,
        _: &PublicKeyBytes,
        inputs: &ForgeInputs<'_>,
        _: i64,
        slot: i64,
    ) -> Result<SignedOutcome, SignError> {
        Ok(SignedOutcome::New(Signed {
            height: inputs.previous_height.saturating_add(1),
            slot,
            id: BlockId::from_array([0; 32]),
            bytes: Vec::new(),
        }))
    }
}

/// The lines logged while it is installed: the reference's level and the message.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<(String, String)>>>);

#[derive(Default)]
struct LineFields {
    severity: String,
    message: String,
}

impl Visit for LineFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "severity" => self.severity = value.to_owned(),
            "message" => self.message = value.to_owned(),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for Lines {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = LineFields::default();
        event.record(&mut fields);
        let mut lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        lines.push((fields.severity, fields.message));
    }
}

fn decide(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let milestones = env.milestones(case)?.into_owned();
    let read = |key: &str| input.get(key).cloned().unwrap_or(Value::Null);
    let round = CurrentRound::from_json(&read("round"))
        .map_err(|error| failed(format!("input.round: {error}")))?;
    let state =
        NetworkState::from_json(&read("networkState")).map_err(|error| RelayError::Communication {
            endpoint: "127.0.0.1:4002<p2p.internal.getNetworkState>".into(),
            message: error.to_string(),
        });
    let secret = SecretKey::from_hex(text(input, "privateKey")?)
        .map_err(|error| failed(format!("input.privateKey: {error}")))?;
    let key = KeyPair::from_secret_key(secret)
        .public_key()
        .to_compressed();
    let slots: Vec<i64> = (read("slots").as_array())
        .map(|slots| slots.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    let fail = read("fail");
    let fail = (fail.get("call").and_then(Value::as_str))
        .zip(fail.get("message").and_then(Value::as_str))
        .map(|(call, message)| (call.to_owned(), message.to_owned()));
    let pool = read("pool");
    let transactions = (pool.get("transactions").and_then(Value::as_array))
        .map(|list| {
            list.iter()
                .filter_map(|t| t.as_str().and_then(|t| hex::decode(t).ok()))
                .collect()
        })
        .unwrap_or_default();
    let link = Scripted {
        state,
        status: status(&read("status")),
        slots: Mutex::new(SLOT_CALLS.into_iter().zip(slots).collect()),
        pool: ForgingTransactions {
            pool_size: pool
                .get("poolSize")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            transactions,
        },
        fail,
        round: round.clone(),
    };
    let first = milestones.at(1);
    let epoch =
        slots::epoch_unix_ms(first.epoch()).ok_or_else(|| failed("the epoch does not parse"))?;
    let time_left = input["timeLeftInMs"]
        .as_i64()
        .ok_or_else(|| failed("input.timeLeftInMs"))?;
    let block_ms =
        i64::try_from(milestones.at(round.last_block.height).block_time()).unwrap_or(0) * 1_000;
    let now = epoch + round.timestamp * 1_000 + block_ms - time_left;
    let parts = ForgerParts {
        link: Arc::new(link),
        signer: Unsigned,
        delegates: vec![key],
        options: Options::from_json(&Value::Null, &|_| None).map_err(|e| failed(e.to_string()))?,
        milestones: milestones.clone(),
        clock: Arc::new(ManualClock::new(now)) as Arc<dyn Clock>,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let lines = Lines::default();
    let first_attempt = input["firstAttempt"] == Value::Bool(true);
    let end = runtime.block_on(async {
        let mut service = ForgerService::new(parts);
        service.boot().await.map_err(|e| failed(e.to_string()))?;
        let _capture =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(lines.clone()));
        Ok::<AttemptEnd, SubjectError>(service.forge_attempt(&key, first_attempt, &round).await)
    })?;
    let lines = lines
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let action = match end {
        AttemptEnd::Forged => "forged",
        AttemptEnd::Stopped => "stop",
        AttemptEnd::Refused => "refused",
        AttemptEnd::Late => "late",
        AttemptEnd::SameSlot => "sameSlot",
        AttemptEnd::Errored => "error",
    };
    let reason = (end == AttemptEnd::Refused).then(|| {
        let status = NetworkState::from_json(&read("networkState")).map(|state| state.status);
        match status {
            Ok(NetworkStateStatus::Unknown) => "unknown",
            Ok(NetworkStateStatus::ColdStart) => "coldStart",
            Ok(NetworkStateStatus::BelowMinimumDelegates) => "belowMinimum",
            _ if lines
                .iter()
                .any(|(_, line)| line.contains("already forged a block")) =>
            {
                "alreadyForged"
            }
            _ => "quorum",
        }
    });
    if reason == Some("alreadyForged") {
        let own = own_headers(&read("networkState"), &key.to_hex(), &milestones);
        // The reference refuses too when one of them is an empty block; else it forges.
        if !own.iter().any(|empty| *empty) {
            let difference = Documented::OverHeightOwnBlock;
            return Ok(Verdict::RefusedByDesign {
                difference,
                holds: !own.is_empty(),
            });
        }
    }
    let lines: Vec<Value> = lines
        .into_iter()
        .map(|(level, line)| json!([level, line]))
        .collect();
    Ok(Verdict::Output(
        json!({ "action": action, "reason": reason, "lines": lines }),
    ))
}

/// For each over-height header by `key` whose signature verifies, whether its block is empty.
fn own_headers(state: &Value, key: &str, milestones: &Milestones) -> Vec<bool> {
    let headers = state["quorumDetails"]["overHeightBlockHeaders"].as_object();
    let own = headers.into_iter().flat_map(|headers| headers.values());
    own.filter(|header| header["generatorPublicKey"].as_str() == Some(key))
        .filter_map(|header| factory::from_json(header, milestones).ok())
        .filter(|block| block.verify_signature())
        .map(|block| block.header().number_of_transactions == 0)
        .collect()
}

/// The status header of the input: its timestamp and generator when given.
fn status(header: &Value) -> ReceivedStatus {
    let message = GetStatusResponse {
        state: Some(State {
            header: Some(StatusHeader {
                timestamp: header["timestamp"]
                    .as_u64()
                    .and_then(|t| u32::try_from(t).ok()),
                generator_public_key: header["generatorPublicKey"].as_str().map(str::to_owned),
                ..StatusHeader::default()
            }),
            ..State::default()
        }),
        ..GetStatusResponse::default()
    };
    ReceivedStatus {
        message,
        total_amount: Amount::from(0_u64),
        total_fee: Amount::from(0_u64),
        reward: Amount::from(0_u64),
    }
}

fn network_state(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let milestones = env.milestones(case)?;
    let json = match input.get("body") {
        Some(body) => body
            .as_str()
            .ok_or_else(|| failed("input.body is not text"))?
            .to_owned(),
        None => {
            // The relay's state: a tip made as the oracle makes it, and the counts given.
            let details = input.get("quorumDetails").cloned().unwrap_or(json!({}));
            let mut state = NetworkState::from_json(
                &json!({ "status": input["status"], "quorumDetails": details }),
            )
            .map_err(|e| failed(format!("input: {e}")))?;
            if let Some(tip) = input.get("lastBlock") {
                let block = tip_block(tip, &milestones)?;
                state.node_height = Some(block.0);
                state.last_block_id = Some(block.1);
                state.last_generator = tip["username"].as_str().map(str::to_owned);
                state.last_slot_number = input["lastSlotNumber"].as_i64();
            }
            String::from_utf8_lossy(&internal::encode(&state)).into_owned()
        }
    };
    let state: NetworkState = match internal::decode(json.as_bytes()) {
        Ok(state) => state,
        Err(error) => return Ok(unread(error)),
    };
    let quorum = state.quorum();
    let quorum = if quorum.fract() == 0.0 {
        json!(quorum as i64)
    } else {
        json!(quorum)
    };
    let height = super::height(case)?;
    let active = u32::try_from(milestones.at(height).active_delegates()).unwrap_or(u32::MAX);
    let headers: Vec<&Value> = state
        .quorum_details
        .over_height_block_headers
        .values()
        .collect();
    let read = json!({
        "status": state.status as u8,
        "nodeHeight": state.node_height,
        "lastBlockId": state.last_block_id,
        "lastGenerator": state.last_generator,
        "lastSlotNumber": state.last_slot_number,
        "quorum": quorum,
        "canForge": state.can_forge(active),
        "overHeightBlockHeaders": headers,
    });
    Ok(Verdict::Output(
        json!({ "json": json, "read": read, "activeDelegates": active }),
    ))
}

/// The height and id of the block the oracle makes for a tip: an empty block of the passphrase's
/// key, with aux bytes of 7.
fn tip_block(tip: &Value, milestones: &Milestones) -> Outcome<(u32, String)> {
    let keys =
        KeyPair::from_passphrase(text(tip, "passphrase")?).map_err(|e| failed(e.to_string()))?;
    let number = |key: &str| {
        (tip[key].as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| failed(format!("lastBlock.{key}")))
    };
    let draft = BlockDraft {
        version: 0,
        timestamp: number("timestamp")?,
        height: number("height")?,
        previous_block: [0; 32],
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward: 0,
        payload_length: 0,
        payload_hash: hash::sha256(b""),
    };
    let block = factory::make(draft, Vec::new(), &keys, Aux::fixed([7; 32]), milestones)
        .map_err(|e| failed(e.to_string()))?;
    Ok((block.height(), block.id().to_hex()))
}

fn current_round(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let round: CurrentRound = match internal::decode(text(case.input, "json")?.as_bytes()) {
        Ok(round) => round,
        Err(error) => return Ok(unread(error)),
    };
    let pairs = |list: &[DelegateWallet]| -> Vec<Value> {
        list.iter()
            .map(|w| json!([w.public_key, w.username]))
            .collect()
    };
    let key = |wallet: &Option<DelegateWallet>| wallet.as_ref().map(|w| w.public_key.clone());
    let read = json!({
        "timestamp": round.timestamp,
        "reward": round.reward,
        "lastBlockHeight": round.last_block.height,
        "currentForger": key(&round.current_forger),
        "nextForger": key(&round.next_forger),
        "delegates": pairs(&round.delegates),
        "allDelegates": pairs(&round.all_delegates),
    });
    Ok(Verdict::Output(json!({ "read": read })))
}

/// A body the forger does not read: the class of the reference's reader where it has one.
fn unread(error: InternalBodyError) -> Verdict {
    Verdict::Rejected {
        class: Some(error.js_class()),
        detail: error.to_string(),
    }
}
