//! The signing record and the block signer: the record's decisions, its file and its write
//! order, and the signer's refusals.
//!
//! A slot is signed once per key: a retry, a restart or a crash between the record's write and the
//! block's send gives back the recorded bytes, whatever the inputs are now. A slot below the
//! recorded one, or more than one away from the forger's own slot, is refused and nothing is
//! recorded.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockId, KeyPair, Milestones, Network, PublicKeyBytes};
use heartwood_forger::signing_record::{self, Decision};
use heartwood_forger::{
    BlockSigner, Delegate, ForgeInputs, RecordError, SignError, Signed, SignedOutcome, Signer,
    SigningRecord,
};
use serde_json::Value;

const NETHASH: &str = "c9b03ab996ef3ac216a2ac53eaee71118cbf7995fa44449a7fb7f94bbe18bcca";

/// A data directory of its own for each test, removed when dropped.
struct Data(PathBuf);

impl Data {
    fn new(name: &str) -> Data {
        let dir = std::env::temp_dir().join(format!(
            "heartwood-forger-record-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        Data(dir)
    }

    fn record(&self) -> PathBuf {
        signing_record::path(&self.0)
    }

    fn open(&self) -> Result<SigningRecord, RecordError> {
        SigningRecord::open(&self.record(), NETHASH)
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The committed devnet's milestones.
fn milestones() -> Milestones {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto");
    let read = |name: &str| fs::read_to_string(root.join("tests/vectors").join(name)).unwrap();
    let meta: Value =
        serde_json::from_str(read("V02-transactions.jsonl").lines().next().unwrap()).unwrap();
    let digest = meta["network"]["milestonesSha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let genesis = read("V10-genesis.jsonl");
    let files = (genesis.lines().skip(1))
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["output"]["files"].clone())
        .find(|files| {
            let text = files["crypto/milestones.json"].as_str().unwrap();
            hex::encode(&sha256(text.as_bytes())) == digest
        })
        .unwrap();
    let network = Network::load(files["crypto/network.json"].as_str().unwrap()).unwrap();
    Milestones::load(files["crypto/milestones.json"].as_str().unwrap(), &network).unwrap()
}

fn key_pair(passphrase: &str) -> Arc<KeyPair> {
    Arc::new(KeyPair::from_passphrase(passphrase).unwrap())
}

fn signed(slot: i64, byte: u8) -> Signed {
    Signed {
        height: 10,
        slot,
        id: BlockId::from_array([byte; 32]),
        bytes: vec![byte; 3],
    }
}

/// The inputs of a block after `previous`.
fn inputs(previous: u8, reward: u64) -> ForgeInputs<'static> {
    ForgeInputs {
        transactions: &[],
        previous_id: BlockId::from_array([previous; 32]),
        previous_height: 9,
        timestamp: 80,
        reward,
    }
}

/// A signer of `keys` over the record in `data`, with a fresh random aux for every signature.
fn signer(data: &Data, keys: &[Arc<KeyPair>]) -> Signer {
    let delegates = keys.iter().map(|k| Delegate::new(k.clone(), 90)).collect();
    Signer::new(delegates, data.open().unwrap(), Aux::random(), milestones())
}

#[test]
fn the_record_decides_by_the_window_first_then_by_the_recorded_slot() {
    let data = Data::new("decisions");
    let mut record = data.open().unwrap();
    let key = key_pair("delegate a").public_key().to_compressed();
    let other = key_pair("delegate b").public_key().to_compressed();
    assert_eq!(record.decide(&key, 10, 10), Decision::SignNew);
    assert_eq!(record.decide(&key, 11, 10), Decision::SignNew);
    assert_eq!(record.decide(&key, 9, 10), Decision::SignNew);
    assert_eq!(
        record.decide(&key, 12, 10),
        Decision::OutOfWindow { own_slot: 10 }
    );
    assert_eq!(
        record.decide(&key, 8, 10),
        Decision::OutOfWindow { own_slot: 10 }
    );
    record.record(&key, signed(10, 1)).unwrap();
    assert_eq!(record.decide(&key, 10, 10), Decision::Resend(signed(10, 1)));
    assert_eq!(
        record.decide(&key, 9, 10),
        Decision::Refuse { recorded_slot: 10 }
    );
    assert_eq!(record.decide(&key, 11, 11), Decision::SignNew);
    // The window comes first, even for the recorded slot.
    assert_eq!(
        record.decide(&key, 10, 12),
        Decision::OutOfWindow { own_slot: 12 }
    );
    // Each key has its own entry.
    assert_eq!(record.decide(&other, 9, 10), Decision::SignNew);
    assert_eq!(record.last(&key), Some(&signed(10, 1)));
    assert_eq!(record.last(&other), None);
}

#[test]
fn the_record_survives_a_restart_in_a_private_file() {
    let data = Data::new("reopen");
    let key = key_pair("delegate a").public_key().to_compressed();
    let mut record = data.open().unwrap();
    record.record(&key, signed(10, 1)).unwrap();
    record.record(&key, signed(12, 2)).unwrap();
    drop(record);
    let record = data.open().unwrap();
    assert_eq!(record.last(&key), Some(&signed(12, 2)));
    let mode = |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&data.record()), 0o600);
    assert_eq!(mode(data.record().parent().unwrap()), 0o700);
    assert_eq!(
        data.record(),
        data.0.join("forger").join("signing-record.json")
    );
}

#[test]
fn a_torn_temporary_file_is_ignored() {
    let data = Data::new("torn");
    let key = key_pair("delegate a").public_key().to_compressed();
    data.open().unwrap().record(&key, signed(10, 1)).unwrap();
    // A write killed before its rename leaves its temporary file, perhaps cut short.
    let temporary = data.record().with_extension("json.tmp");
    fs::write(&temporary, r#"{"format":"heartwood-signing-rec"#).unwrap();
    let record = data.open().unwrap();
    assert_eq!(record.last(&key), Some(&signed(10, 1)));
    assert!(!temporary.exists());
}

#[test]
fn a_record_of_another_network_or_not_a_record_stops_the_start() {
    let data = Data::new("refused");
    data.open().unwrap();
    let error = SigningRecord::open(&data.record(), &"00".repeat(32)).unwrap_err();
    assert!(matches!(error, RecordError::Network { .. }), "{error:?}");
    for text in [
        "not json",
        r#"{"format":"other"}"#,
        r#"{"format":"heartwood-signing-record/1","nethash":"NETHASH"}"#,
        r#"{"format":"heartwood-signing-record/1","nethash":"NETHASH","keys":{"02":{}}}"#,
    ] {
        fs::write(data.record(), text.replace("NETHASH", NETHASH)).unwrap();
        let error = data.open().unwrap_err();
        assert!(
            matches!(error, RecordError::Corrupt { .. }),
            "{text}: {error:?}"
        );
    }
}

#[test]
fn a_slot_is_signed_once_and_a_retry_sends_the_same_bytes() {
    let data = Data::new("once");
    let keys = key_pair("delegate a");
    let key = keys.public_key().to_compressed();
    let mut signer = signer(&data, &[keys]);
    let first = signer.sign_or_resend(&key, &inputs(1, 5), 20, 20).unwrap();
    let SignedOutcome::New(block) = &first else {
        panic!("{first:?}")
    };
    assert_eq!((block.height, block.slot), (10, 20));
    // The tip moved and the aux is random: a new signature would differ, the record sends the
    // same bytes.
    let again = signer.sign_or_resend(&key, &inputs(2, 6), 20, 20).unwrap();
    assert_eq!(again, SignedOutcome::Resent(block.clone()));
    assert_eq!(signer.record().last(&key), Some(block));
    let next = signer.sign_or_resend(&key, &inputs(2, 6), 21, 21).unwrap();
    assert!(matches!(next, SignedOutcome::New(ref b) if b.bytes != block.bytes));
    assert_eq!(
        signer.sign_or_resend(&key, &inputs(3, 7), 21, 20),
        Err(SignError::Refused {
            slot: 20,
            recorded_slot: 21
        })
    );
}

#[test]
fn a_crash_between_the_write_and_the_send_resends_the_recorded_block() {
    let data = Data::new("crash");
    let keys = key_pair("delegate a");
    let key = keys.public_key().to_compressed();
    let mut before = signer(&data, std::slice::from_ref(&keys));
    let SignedOutcome::New(recorded) = before.sign_or_resend(&key, &inputs(1, 5), 30, 30).unwrap()
    else {
        panic!("a new block")
    };
    // The process dies before the block leaves; a new one starts in the same slot, where the
    // relay now names another tip and reward.
    drop(before);
    let mut after = signer(&data, &[keys]);
    assert_eq!(
        after.sign_or_resend(&key, &inputs(9, 99), 30, 30),
        Ok(SignedOutcome::Resent(recorded))
    );
}

#[test]
fn a_slot_two_ahead_of_the_forgers_clock_is_refused_and_nothing_is_recorded() {
    let data = Data::new("far");
    let keys = key_pair("delegate a");
    let key = keys.public_key().to_compressed();
    let mut signer = signer(&data, &[keys]);
    let text = || fs::read_to_string(data.record()).unwrap();
    let empty = text();
    // A relay whose answers name a slot two ahead of this forger's clock.
    assert_eq!(
        signer.sign_or_resend(&key, &inputs(1, 5), 40, 42),
        Err(SignError::OutOfWindow {
            slot: 42,
            own_slot: 40
        })
    );
    assert_eq!(signer.record().last(&key), None);
    assert_eq!(text(), empty);
    // Slot 41 is within one: signed. The far slot did not make the record refuse it.
    assert!(matches!(
        signer.sign_or_resend(&key, &inputs(1, 5), 40, 41),
        Ok(SignedOutcome::New(_))
    ));
}

#[test]
fn a_key_the_forger_does_not_hold_and_a_block_that_cannot_be_made_are_refused() {
    let data = Data::new("refusals");
    let mut signer = signer(&data, &[key_pair("delegate a")]);
    let stranger: PublicKeyBytes = key_pair("delegate b").public_key().to_compressed();
    assert_eq!(
        signer.sign_or_resend(&stranger, &inputs(1, 5), 1, 1),
        Err(SignError::UnknownKey {
            public_key: stranger.to_hex()
        })
    );
    let key = key_pair("delegate a").public_key().to_compressed();
    let last = ForgeInputs {
        previous_height: u32::MAX,
        ..inputs(1, 5)
    };
    let refused = signer.sign_or_resend(&key, &last, 1, 1);
    assert!(matches!(refused, Err(SignError::Forge(_))), "{refused:?}");
    assert_eq!(signer.record().last(&key), None);
}

/// The forger's loop over a relay whose answers name a slot two ahead of the forger's clock: its
/// round, its slots and its tip. The loop checks nothing that tells it so; the signer refuses the
/// slot, nothing is recorded and no block is posted. Needs the loop (the wave-1 integration).
mod far_relay {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use heartwood_crypto::crypto::slots;
    use heartwood_forger::{
        Deadline, ForgerEvent, ForgerParts, ForgerService, ForgingTransactions,
    };
    use heartwood_forger::{Options, RelayError, RelayLink, ShutdownSignal};
    use heartwood_kernel::Clock;
    use heartwood_p2p_wire::Amount;
    use heartwood_p2p_wire::internal::{
        CurrentRound, DelegateAttribute, DelegateWallet, LastBlockData, NetworkState,
        NetworkStateStatus, QuorumDetails,
    };
    use heartwood_p2p_wire::status::{
        self, NetworkConfig, PeerConfig, ReceivedStatus, SignedState, StatusHeaderData, TokenConfig,
    };

    use super::*;

    const BLOCK_TIME: i64 = 8;

    /// The wall clock of the test: `start` plus tokio's paused time.
    struct PausedClock {
        start: i64,
        origin: tokio::time::Instant,
    }

    impl Clock for PausedClock {
        fn now_ms(&self) -> i64 {
            let elapsed = self.origin.elapsed().as_millis();
            self.start + i64::try_from(elapsed).unwrap()
        }
    }

    struct FarRelay {
        clock: Arc<PausedClock>,
        epoch: i64,
        key: String,
        posted: AtomicUsize,
    }

    impl FarRelay {
        /// The relay's slot: two ahead of the forger's.
        fn slot(&self) -> i64 {
            let now = slots::epoch_time(self.clock.now_ms(), self.epoch);
            slots::slot_number(now, 8) + 2
        }

        fn height(&self) -> u32 {
            u32::try_from(self.slot()).unwrap()
        }

        fn wallet(&self) -> DelegateWallet {
            DelegateWallet {
                address: String::new(),
                public_key: self.key.clone(),
                balance: "0".into(),
                nonce: "0".into(),
                delegate: DelegateAttribute {
                    username: "genesis_1".into(),
                    fields: Default::default(),
                },
            }
        }
    }

    impl RelayLink for FarRelay {
        async fn get_round(&self) -> Result<CurrentRound, RelayError> {
            let last = self.height();
            Ok(CurrentRound {
                all_delegates: vec![self.wallet()],
                can_forge: true,
                current: 1,
                current_forger: Some(self.wallet()),
                delegates: vec![self.wallet()],
                last_block: LastBlockData {
                    id: "ab".repeat(32),
                    version: 0,
                    timestamp: 0,
                    height: last,
                    previous_block: "cd".repeat(32),
                    number_of_transactions: 0,
                    total_amount: "0".into(),
                    total_fee: "0".into(),
                    burned_fee: None,
                    reward: "0".into(),
                    donations: None,
                    username: None,
                    payload_length: 0,
                    payload_hash: String::new(),
                    generator_public_key: self.key.clone(),
                    block_signature: String::new(),
                },
                next_forger: None,
                reward: "0".into(),
                timestamp: self.slot() * BLOCK_TIME,
            })
        }

        async fn get_slot_number(
            &self,
            timestamp: Option<i64>,
            _: Deadline,
        ) -> Result<i64, RelayError> {
            Ok(timestamp.map_or_else(|| self.slot(), |t| t.div_euclid(BLOCK_TIME)))
        }

        async fn get_network_state(
            &self,
            _: bool,
            _: Deadline,
        ) -> Result<NetworkState, RelayError> {
            Ok(NetworkState {
                status: NetworkStateStatus::Default,
                node_height: Some(self.height()),
                last_block_id: Some("ab".repeat(32)),
                last_generator: None,
                last_slot_number: Some(0),
                quorum_details: QuorumDetails {
                    has_quorum: 53,
                    ..QuorumDetails::default()
                },
            })
        }

        async fn get_status(&self, _: Deadline) -> Result<ReceivedStatus, RelayError> {
            let header = StatusHeaderData {
                id: "ab".repeat(32),
                version: 0,
                timestamp: 0,
                previous_block: "cd".repeat(32),
                height: self.height(),
                number_of_transactions: 0,
                total_amount: 0,
                total_fee: 0,
                reward: 0,
                payload_length: 0,
                payload_hash: String::new(),
                generator_public_key: self.key.clone(),
                block_signature: String::new(),
            };
            let state = SignedState {
                height: self.height(),
                forging_allowed: true,
                current_slot: self.slot(),
                header,
            };
            let config = PeerConfig {
                version: "4.3.1".into(),
                network: NetworkConfig {
                    version: 90,
                    name: "devnet".into(),
                    nethash: NETHASH.into(),
                    explorer: String::new(),
                    token: TokenConfig {
                        name: "dROOT".into(),
                        symbol: "dRT".into(),
                    },
                },
                plugins: Vec::new(),
            };
            Ok(ReceivedStatus {
                message: status::response(&state, &config, &[]),
                total_amount: Amount::ZERO,
                total_fee: Amount::ZERO,
                reward: Amount::ZERO,
            })
        }

        async fn get_transactions(
            &self,
            _: Deadline,
        ) -> Result<Option<ForgingTransactions>, RelayError> {
            Ok(None)
        }

        async fn broadcast_block(&self, _: &[u8], _: Deadline) {
            self.posted.fetch_add(1, Ordering::SeqCst);
        }

        async fn sync_with_network(&self) {}

        async fn emit_event(&self, _: ForgerEvent) {}

        fn dispose(&self) {}
    }

    #[tokio::test(start_paused = true)]
    #[ignore = "integration"]
    async fn the_loop_does_not_sign_a_slot_its_relay_names_two_ahead() {
        let data = Data::new("far-relay");
        let milestones = milestones();
        let epoch = slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap();
        let clock = Arc::new(PausedClock {
            start: epoch + 1_000 * BLOCK_TIME * 1_000 + 100,
            origin: tokio::time::Instant::now(),
        });
        let keys = key_pair("delegate a");
        let key = keys.public_key().to_compressed();
        let relay = Arc::new(FarRelay {
            clock: clock.clone(),
            epoch,
            key: key.to_hex(),
            posted: AtomicUsize::new(0),
        });
        let options = Options::from_json(&Value::Null, &|_| None).unwrap();
        let mut service = ForgerService::new(ForgerParts {
            link: relay.clone(),
            signer: signer(&data, &[keys]),
            delegates: vec![key],
            options,
            milestones,
            clock,
        });
        service.boot().await.unwrap();
        let (trigger, shutdown) = ShutdownSignal::new();
        let stop = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            trigger.trigger();
        };
        tokio::join!(service.run(shutdown), stop);
        assert_eq!(relay.posted.load(Ordering::SeqCst), 0);
        assert_eq!(data.open().unwrap().last(&key), None);
    }
}
