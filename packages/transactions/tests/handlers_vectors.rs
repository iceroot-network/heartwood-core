//! V17: one transaction applied to a wallet fixture as a block applies it, then reverted,
//! compared with the reference by the `#W` records of the state digest after each step, or by the
//! refusal's class, its message (with amounts in the devnet's token symbol) and the records after
//! the refused attempt.
//!
//! The fixture reader and the record writer here are independent of the harness's, so that the
//! handlers are checked against the reference by this crate alone.

mod golden;

use heartwood_crypto::identities::{Address, PublicKey, PublicKeyBytes};
use heartwood_crypto::transactions::types::{Asset, Votes};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{BlockId, Transaction};
use heartwood_state::dpos::build_vote_balances;
use heartwood_state::wallets::vote_balances::update_wallet_vote_balance;
use heartwood_state::wallets::{Resignation, Wallet, WalletRepository};
use heartwood_state::{ApplyContext, TransactionHandlers};
use heartwood_transactions::{Handlers, TransactionError};
use serde_json::Value;

use golden::{Devnet, Record, VectorFile};

fn amount(value: &Value) -> u64 {
    value
        .as_str()
        .map_or(0, |text| text.parse().expect("an amount"))
}

/// The wallets of a fixture: `a`, `pk`, `bal`, `n`, `spk`, the delegate keys and `resignedAt`,
/// and `v`; then the vote balances, built from the voters that have a key.
fn fixture(value: &Value, network: u8) -> WalletRepository {
    let mut wallets = WalletRepository::new(network);
    for entry in value.as_array().expect("a fixture is an array") {
        let address =
            Address::from_base58_for(entry["a"].as_str().expect("a"), network).expect("an address");
        if let Some(key) = entry["pk"].as_str() {
            wallets.bind_public_key(&address, PublicKey::from_hex(key).expect("a key"));
        }
        let wallet = wallets.get_or_create(&address);
        wallet.credit(amount(&entry["bal"])).expect("a balance");
        wallet.set_nonce(amount(&entry["n"]));
        if let Some(key) = entry["spk"].as_str() {
            wallet
                .set_second_public_key(Some(PublicKeyBytes::from_hex(key).expect("a second key")));
        }
        if let Some(votes) = entry["v"].as_array() {
            let votes: Votes = votes
                .iter()
                .map(|pair| {
                    let name = pair[0].as_str().expect("a name").to_owned();
                    (
                        name,
                        u16::try_from(pair[1].as_u64().expect("points")).expect("points"),
                    )
                })
                .collect();
            wallet.set_votes(&votes);
        }
        if let Some(username) = entry["u"].as_str() {
            let delegate = wallets
                .register_delegate(&address, username.to_owned())
                .expect("a free name");
            delegate.produced_blocks = entry["pb"].as_u64().unwrap_or(0);
            delegate.forged_fees = amount(&entry["ff"]);
            delegate.forged_rewards = amount(&entry["fr"]);
            delegate.burned_fees = amount(&entry["bf"]);
            delegate.donations = amount(&entry["dn"]);
            delegate.last_block = entry["lb"]
                .as_str()
                .map(|id| BlockId::from_hex(id).expect("a block id"));
            delegate.resigned = match entry["rs"].as_u64() {
                None => None,
                Some(0) => Some(Resignation::Temporary),
                Some(_) => Some(Resignation::Permanent),
            };
            delegate.resigned_at = entry["resignedAt"]
                .as_u64()
                .map(|height| u32::try_from(height).expect("a height"));
        }
    }
    build_vote_balances(&mut wallets).expect("the fixture's vote balances");
    wallets
}

/// The `#W` section: every wallet with a balance, a nonce, a key, a second key, a delegate record
/// or votes, by the address text, with the keys in the digest's order.
fn wallet_section(wallets: &WalletRepository) -> String {
    let mut listed: Vec<&Wallet> = wallets
        .iter()
        .filter(|wallet| {
            wallet.balance() != 0
                || wallet.nonce() != 0
                || wallet.public_key().is_some()
                || wallet.second_public_key().is_some()
                || wallet.is_delegate()
                || wallet.has_voted()
        })
        .collect();
    listed.sort_by_key(|wallet| wallet.address().to_base58());
    let text = |value: &str| Value::from(value).to_string();
    let mut section = String::from("#W\n");
    for wallet in listed {
        let mut fields = vec![("a", text(&wallet.address().to_base58()))];
        if let Some(key) = wallet.public_key() {
            fields.push(("pk", text(&key.to_hex())));
        }
        fields.push(("bal", text(&wallet.balance().to_string())));
        fields.push(("n", text(&wallet.nonce().to_string())));
        if let Some(key) = wallet.second_public_key() {
            fields.push(("spk", text(&key.to_hex())));
        }
        if let Some(delegate) = wallet.delegate() {
            fields.push(("u", text(delegate.username())));
            fields.push(("vb", text(&delegate.vote_balance.to_string())));
            fields.push(("vc", delegate.voters.to_string()));
            if let Some(rank) = delegate.rank {
                fields.push(("rk", rank.to_string()));
            }
            if let Some(resigned) = delegate.resigned {
                fields.push(("rs", resigned.number().to_string()));
            }
            fields.push(("pb", delegate.produced_blocks.to_string()));
            fields.push(("ff", text(&delegate.forged_fees.to_string())));
            fields.push(("fr", text(&delegate.forged_rewards.to_string())));
            fields.push(("bf", text(&delegate.burned_fees.to_string())));
            fields.push(("dn", text(&delegate.donations.to_string())));
            if let Some(id) = &delegate.last_block {
                fields.push(("lb", text(&id.to_hex())));
            }
        }
        if wallet.has_voted() {
            let votes: Vec<String> = wallet
                .votes()
                .iter()
                .map(|(name, points)| format!("[{},{points}]", text(name)))
                .collect();
            fields.push(("v", format!("[{}]", votes.join(","))));
        }
        let record: Vec<String> = fields
            .iter()
            .map(|(key, value)| format!("\"{key}\":{value}"))
            .collect();
        section.push_str(&format!("{{{}}}\n", record.join(",")));
    }
    section
}

/// After a transaction or its revert, the block brings the vote balances of the wallets it changed
/// up to date: each transfer recipient in item order, then the sender.
fn refresh(wallets: &mut WalletRepository, transaction: &Transaction, sender: &Address) {
    if let Asset::Transfer { transfers } = &transaction.data().asset {
        for item in transfers {
            update_wallet_vote_balance(wallets, &item.recipient)
                .expect("a recipient's vote balance");
        }
    }
    update_wallet_vote_balance(wallets, sender).expect("the sender's vote balance");
}

/// The section `w` without the key of `sender`, and without its record when nothing else would
/// list the wallet.
fn without_key(w: &str, sender: &str) -> String {
    let mut out = String::new();
    for line in w.lines() {
        let mut line = line.to_owned();
        if line.starts_with(&format!("{{\"a\":\"{sender}\",\"pk\":")) {
            let value: Value = serde_json::from_str(&line).expect("a record");
            let key = value["pk"].as_str().expect("a key");
            line = line.replacen(&format!(",\"pk\":\"{key}\""), "", 1);
            if line == format!("{{\"a\":\"{sender}\",\"bal\":\"0\",\"n\":\"0\"}}") {
                continue;
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Compare one record; returns whether it was the refused first transaction of a sender without
/// a key, which the reference leaves bound and Heartwood does not.
fn check(devnet: &Devnet, record: &Record, label: &str) -> bool {
    let input = &record.input;
    let milestones = devnet
        .milestones_of(input)
        .expect("the record's milestones load");
    let last_block_height =
        u32::try_from(input["lastBlockHeight"].as_u64().expect("a height")).expect("a height");
    let ctx = ApplyContext {
        height: last_block_height + 1,
        params: milestones.at(last_block_height + 1),
        last_block_height,
    };
    let mut wallets = fixture(&input["fixture"], ctx.params.network());
    let bytes = hex::decode(input["hex"].as_str().expect("hex")).expect("hex");
    let transaction = Transaction::decode(&bytes, ctx.params).expect("the transaction decodes");
    let sender = transaction.data().sender_address(ctx.params);
    let expected = record.expected.as_ref().expect("an output");
    let section = |key: &str, step: &str| {
        expected[key][step]
            .as_str()
            .unwrap_or_else(|| panic!("{label}: no {key}.{step}"))
            .to_owned()
    };

    let applied: Result<_, TransactionError> = Handlers.apply(&ctx, &mut wallets, &transaction);
    match applied {
        Ok(undo) => {
            refresh(&mut wallets, &transaction, &sender);
            assert_eq!(
                wallet_section(&wallets),
                section("applied", "W"),
                "{label}: after apply"
            );
            Handlers
                .revert(&ctx, &mut wallets, &transaction, &undo)
                .unwrap_or_else(|error| panic!("{label}: the revert failed: {error}"));
            refresh(&mut wallets, &transaction, &sender);
            assert_eq!(
                wallet_section(&wallets),
                section("reverted", "W"),
                "{label}: after revert"
            );
            false
        }
        Err(rejected) => {
            let class = expected["error"]["class"].as_str();
            assert_eq!(
                class,
                Some(rejected.class()),
                "{label}: the refusal's class"
            );
            assert_eq!(
                expected["error"]["message"].as_str(),
                Some(rejected.message(&devnet.network.symbol).as_str()),
                "{label}: the refusal's message"
            );
            let after = section("after", "W");
            let ours = wallet_section(&wallets);
            let keyless = input["fixture"]
                .as_array()
                .expect("a fixture")
                .iter()
                .find(|entry| entry["a"] == sender.to_base58())
                .is_some_and(|entry| entry.get("pk").is_none());
            if keyless {
                // The reference binds the sender's key before its checks and keeps it after the
                // refusal; Heartwood leaves the wallets as they were.
                let key = transaction.data().sender_public_key.to_hex();
                assert!(
                    after.contains(&format!("\"pk\":\"{key}\"")),
                    "{label}: the reference bound the key"
                );
                assert!(!ours.contains(&key), "{label}: Heartwood bound the key");
                assert_eq!(
                    without_key(&after, &sender.to_base58()),
                    ours,
                    "{label}: after the refusal"
                );
                true
            } else {
                assert_eq!(ours, after, "{label}: after the refusal");
                false
            }
        }
    }
}

#[test]
fn v17_handlers() {
    let devnet = Devnet::load();
    let file = VectorFile::parse("V17-handlers", golden::V17);
    devnet.check(&file);
    let mut keyless_refusals = 0;
    for record in &file.records {
        assert_eq!(record.op, "state.applyTransaction");
        if check(&devnet, record, &record.label(&file.class)) {
            keyless_refusals += 1;
        }
    }
    assert_eq!(keyless_refusals, 2);
}
