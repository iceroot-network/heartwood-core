//! The reference's side of a lockstep chain: what the producer asks of it, and the oracle's
//! answers to those questions.

use serde_json::{Value, json};

use super::LockstepError;
use crate::levels::l2::call;
use crate::oracle::OracleServer;

/// A transaction the reference built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltTransaction {
    /// The transaction's id.
    pub id: String,
    /// Its bytes, in hex.
    pub hex: String,
}

/// What the reference answers to a block: its verdict and its digest line.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockAnswer {
    /// Whether the reference accepted the block.
    pub accepted: bool,
    /// The digest line of the state after the block, when it was accepted.
    pub line: Option<Value>,
    /// Why it refused the block, when it did.
    pub refusal: Option<String>,
}

/// The reference as the producer uses it: one chain session with a pool over it.
pub trait Reference {
    /// Start the session on the chain's four crypto files (`network`, `milestones`,
    /// `genesisBlock`, `exceptions`) with the pool's options; the digest line of the genesis
    /// state.
    fn open(&mut self, crypto: &Value, options: &Value) -> Result<Value, LockstepError>;

    /// The transactions of `specs` (each with its sender's passphrase, nonce and addresses),
    /// built by the reference's builders under the rules of `height`.
    fn build(
        &mut self,
        height: u32,
        specs: &[Value],
    ) -> Result<Vec<BuiltTransaction>, LockstepError>;

    /// The reference pool's verdict on `hexes`, submitted as one batch at `now` milliseconds:
    /// its processor's result as JSON.
    fn submit(&mut self, now: i64, hexes: &[String]) -> Result<Value, LockstepError>;

    /// Record `delegates` (public keys, in hex) as online in `round`.
    fn online(&mut self, round: u64, delegates: &[String]) -> Result<(), LockstepError>;

    /// Apply the block `hex` as a synced node does (`step` is `apply` or `reapply`), the online
    /// record expired at `expire_at` first when given, then the pool's steps after a block.
    fn apply(
        &mut self,
        hex: &str,
        step: &str,
        expire_at: Option<u64>,
    ) -> Result<BlockAnswer, LockstepError>;

    /// Revert `count` blocks and offer their transactions to the pool again; the revert lines.
    fn revert(&mut self, count: u32) -> Result<Vec<Value>, LockstepError>;

    /// End the session.
    fn close(&mut self) -> Result<(), LockstepError>;
}

/// The reference behind an oracle server.
pub struct OracleReference<'a> {
    server: &'a mut OracleServer,
    session: Value,
    height: u64,
}

impl<'a> OracleReference<'a> {
    /// The reference behind `server`, before its session is opened.
    pub fn new(server: &'a mut OracleServer) -> Self {
        Self {
            server,
            session: Value::Null,
            height: 1,
        }
    }

    fn ask(&mut self, op: &str, height: u64, args: &Value) -> Result<Value, LockstepError> {
        call(self.server, op, height, args).map_err(|e| LockstepError::Oracle(e.to_string()))
    }

    fn with_session(&self, mut args: Value) -> Value {
        args["session"] = self.session.clone();
        args
    }
}

impl Reference for OracleReference<'_> {
    fn open(&mut self, crypto: &Value, options: &Value) -> Result<Value, LockstepError> {
        let opened = self.ask(
            "lockstep.open",
            1,
            &json!({"crypto": crypto, "options": options}),
        )?;
        self.session = opened.get("session").cloned().unwrap_or(Value::Null);
        if self.session.is_null() {
            return Err(LockstepError::Oracle(
                "lockstep.open answered no session".into(),
            ));
        }
        Ok(opened.get("line").cloned().unwrap_or(Value::Null))
    }

    fn build(
        &mut self,
        height: u32,
        specs: &[Value],
    ) -> Result<Vec<BuiltTransaction>, LockstepError> {
        let args = self.with_session(json!({ "build": specs }));
        let built = self.ask("lockstep.build", u64::from(height), &args)?;
        let list = built
            .get("transactions")
            .and_then(Value::as_array)
            .ok_or_else(|| LockstepError::Oracle("lockstep.build answered no list".into()))?;
        list.iter()
            .map(|t| match (t["id"].as_str(), t["hex"].as_str()) {
                (Some(id), Some(hex)) => Ok(BuiltTransaction {
                    id: id.to_owned(),
                    hex: hex.to_owned(),
                }),
                _ => Err(LockstepError::Oracle(
                    "lockstep.build answered a transaction without id and hex".into(),
                )),
            })
            .collect()
    }

    fn submit(&mut self, now: i64, hexes: &[String]) -> Result<Value, LockstepError> {
        let args = self.with_session(json!({"transactions": hexes, "now": now}));
        self.ask("lockstep.submit", self.height, &args)
    }

    fn online(&mut self, round: u64, delegates: &[String]) -> Result<(), LockstepError> {
        let args = self.with_session(json!({"round": round, "delegates": delegates}));
        self.ask("lockstep.online", self.height, &args).map(drop)
    }

    fn apply(
        &mut self,
        hex: &str,
        step: &str,
        expire_at: Option<u64>,
    ) -> Result<BlockAnswer, LockstepError> {
        let mut args = self.with_session(json!({"hex": hex, "step": step}));
        if let Some(round) = expire_at {
            args["expireAt"] = json!(round);
        }
        let r = self.ask("lockstep.apply", self.height, &args)?;
        let accepted = r.get("accepted") == Some(&Value::Bool(true));
        Ok(BlockAnswer {
            accepted,
            line: r.get("line").cloned(),
            refusal: (!accepted).then(|| {
                format!(
                    "{}: {}",
                    r["stage"].as_str().unwrap_or("unknown stage"),
                    r["error"]
                )
            }),
        })
    }

    fn revert(&mut self, count: u32) -> Result<Vec<Value>, LockstepError> {
        let args = self.with_session(json!({ "count": count }));
        let r = self.ask("lockstep.revert", self.height, &args)?;
        Ok(r.get("lines")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    fn close(&mut self) -> Result<(), LockstepError> {
        if self.session.is_null() {
            return Ok(());
        }
        let args = self.with_session(json!({}));
        self.ask("lockstep.close", self.height, &args)?;
        self.session = Value::Null;
        Ok(())
    }
}
