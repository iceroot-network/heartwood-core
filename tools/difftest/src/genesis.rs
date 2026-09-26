//! The devnet chain-definition driver, `difftest genesis`.
//!
//! It reads a genesis profile, builds the chain definition for a seed and an epoch with
//! [`heartwood_crypto::genesis::build`], and writes it into a directory:
//!
//! ```text
//! crypto/network.json, crypto/milestones.json, crypto/genesisBlock.json, crypto/exceptions.json
//! delegate-keys.json   the delegates' keys (mode 0600)
//! wallets.json         the generator's, the genesis wallets', the donation recipients' and the
//!                      test wallets' keys (mode 0600)
//! summary.json         nethash, genesis id, transaction count, issued amount, file digests
//! ```
//!
//! [`compare`] then checks the result against a directory that the reference's devnet generator
//! wrote for the same seed, epoch and profile: the four crypto files byte for byte, and the key
//! material and the summary figures field by field.
//!
//! The profile is a JSON object with every input except the seed and the epoch:
//!
//! ```text
//! name                 a label for the summary
//! derivationTag        the first part of every key-derivation input
//! network              network.json without nethash and addressCharacter
//! rules                the rules the first milestone starts from, keys in file order
//! overrides            {"dotted.path": value, ...}, set on the first milestone in order
//! generatorRole        the role of the key that signs the block and the issuance record
//! allocations          [{"role", "amount"}], amounts as decimal strings
//! supply               the sum of the allocations, as a decimal string, checked
//! delegates            {"count", "rolePrefix", "usernamePrefix"}: roles and names count from 1
//! donations            [{"role", "basisPoints", "purpose"?}]
//! donationsFromHeight  the height of the milestone that sets the donations
//! wallets              {"count", "rolePrefix"}: test wallets, derived but not funded
//! ```
//!
//! The profile of the reference's devnet generator is written by the oracle's genesis vector
//! class: `node tools/oracle/lib/vectors/genesis.js --devnet-generator FILE --solar-dir DIR --out
//! FILE`. Every genesis vector record carries the same profile as its input.

use std::fs::{self, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use heartwood_crypto::Network;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::genesis::{
    self, Allocation, ChainDefinition, DerivedKey, DonationShare, GenesisDelegate, GenesisInput,
    RuleOverride,
};
use heartwood_crypto::utils::{hex, js};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::errors::{Error, Result};

/// The warning at the top of the key files.
const WARNING: &str = "DEVNET ONLY. Derived from a seed: anyone with the seed has these keys. \
                       Never use them for anything of value.";

/// The four crypto files, in the order they are written and compared.
pub const CRYPTO_FILES: [&str; 4] = [
    "crypto/network.json",
    "crypto/milestones.json",
    "crypto/genesisBlock.json",
    "crypto/exceptions.json",
];

/// A genesis profile (see the module documentation).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub derivation_tag: String,
    pub network: ProfileNetwork,
    pub rules: Map<String, Value>,
    pub overrides: Map<String, Value>,
    pub generator_role: String,
    pub allocations: Vec<ProfileAllocation>,
    pub supply: String,
    pub delegates: DelegatePattern,
    pub donations: Vec<ProfileDonation>,
    pub donations_from_height: u32,
    pub wallets: WalletPattern,
}

/// `network.json` without `nethash` and `addressCharacter`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileNetwork {
    pub name: String,
    pub message_prefix: String,
    pub bip32: Bip32,
    pub pub_key_hash: u8,
    pub wif: u8,
    pub slip44: u32,
    pub client: Client,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bip32 {
    pub public: u32,
    pub private: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    pub token: String,
    pub symbol: String,
    pub explorer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileAllocation {
    pub role: String,
    pub amount: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegatePattern {
    pub count: u32,
    pub role_prefix: String,
    pub username_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileDonation {
    pub role: String,
    pub basis_points: u16,
    #[serde(default)]
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalletPattern {
    pub count: u32,
    pub role_prefix: String,
}

impl Profile {
    /// Read the profile file at `path`.
    pub fn load(path: &Path) -> Result<Profile> {
        let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        Profile::parse(path, &text)
    }

    /// Parse the text of the profile file at `path`.
    pub fn parse(path: &Path, text: &str) -> Result<Profile> {
        serde_json::from_str(text).map_err(|e| profile_error(path, e.to_string()))
    }

    /// The builder's input for `seed` and `epoch`. `path` names the profile in errors.
    pub fn input(&self, path: &Path, seed: &str, epoch: &str) -> Result<GenesisInput> {
        let network = &self.network;
        let allocations = self
            .allocations
            .iter()
            .map(|a| {
                Ok(Allocation {
                    role: a.role.clone(),
                    amount: parse_amount(path, &a.amount)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(GenesisInput {
            seed: seed.to_owned(),
            derivation_tag: self.derivation_tag.clone(),
            epoch: normalise_epoch(epoch),
            network: Network {
                name: network.name.clone(),
                message_prefix: network.message_prefix.clone(),
                address_character: None,
                bip32_public: network.bip32.public,
                bip32_private: network.bip32.private,
                pub_key_hash: network.pub_key_hash,
                nethash: String::new(),
                wif: network.wif,
                slip44: network.slip44,
                token: network.client.token.clone(),
                symbol: network.client.symbol.clone(),
                explorer: network.client.explorer.clone(),
            },
            rules: self.rules.clone(),
            overrides: self
                .overrides
                .iter()
                .map(|(path, value)| RuleOverride {
                    path: path.clone(),
                    value: value.clone(),
                })
                .collect(),
            generator_role: self.generator_role.clone(),
            allocations,
            delegates: numbered(&self.delegates.role_prefix, self.delegates.count)
                .zip(numbered(
                    &self.delegates.username_prefix,
                    self.delegates.count,
                ))
                .map(|(role, username)| GenesisDelegate { role, username })
                .collect(),
            donations: self
                .donations
                .iter()
                .map(|d| DonationShare {
                    role: d.role.clone(),
                    basis_points: d.basis_points,
                    purpose: d.purpose.clone(),
                })
                .collect(),
            donations_from_height: self.donations_from_height,
            wallet_roles: numbered(&self.wallets.role_prefix, self.wallets.count).collect(),
        })
    }
}

/// `prefix1`, `prefix2`, ... up to `prefix{count}`.
fn numbered(prefix: &str, count: u32) -> impl Iterator<Item = String> + '_ {
    (1..=count).map(move |i| format!("{prefix}{i}"))
}

fn profile_error(path: &Path, message: impl Into<String>) -> Error {
    Error::Profile {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

/// A decimal amount without sign, fraction or leading zeros.
fn parse_amount(path: &Path, text: &str) -> Result<u64> {
    let canonical = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    canonical
        .then(|| text.parse().ok())
        .flatten()
        .ok_or_else(|| profile_error(path, format!("{text:?} is not an amount")))
}

/// `2026-01-01T00:00:00Z` as `2026-01-01T00:00:00.000Z`; any other text unchanged (the builder
/// refuses what is not a whole second in that form).
pub fn normalise_epoch(epoch: &str) -> String {
    match epoch.strip_suffix('Z') {
        Some(time) if epoch.len() == 20 && time.bytes().all(|b| b != b'.') => {
            format!("{time}.000Z")
        }
        _ => epoch.to_owned(),
    }
}

/// A chain definition built from a profile, with the files the driver writes.
#[derive(Debug)]
pub struct Built {
    /// The profile.
    pub profile: Profile,
    /// The builder's input.
    pub input: GenesisInput,
    /// The chain definition.
    pub definition: ChainDefinition,
}

impl Built {
    /// Build the chain definition of the profile at `path` for `seed` and `epoch`, and check the
    /// issued amount against the profile's supply.
    pub fn new(path: &Path, seed: &str, epoch: &str) -> Result<Built> {
        Built::from_profile(path, Profile::load(path)?, seed, epoch)
    }

    /// As [`Built::new`], with the profile already read. `path` names the profile in errors.
    pub fn from_profile(path: &Path, profile: Profile, seed: &str, epoch: &str) -> Result<Built> {
        let input = profile.input(path, seed, epoch)?;
        let definition = genesis::build(&input)?;
        let supply = parse_amount(path, &profile.supply)?;
        if definition.summary.issued != supply {
            return Err(profile_error(
                path,
                format!(
                    "the genesis issues {}, but the supply is {supply}",
                    definition.summary.issued
                ),
            ));
        }
        Ok(Built {
            profile,
            input,
            definition,
        })
    }

    /// The files of the chain definition: relative path, text, and whether it holds secrets.
    pub fn files(&self) -> Vec<(&'static str, String, bool)> {
        let d = &self.definition;
        let mut files = vec![
            (CRYPTO_FILES[0], d.network_json.clone(), false),
            (CRYPTO_FILES[1], d.milestones_json.clone(), false),
            (CRYPTO_FILES[2], d.genesis_block_json.clone(), false),
            (CRYPTO_FILES[3], d.exceptions_json.clone(), false),
        ];
        files.push((
            "delegate-keys.json",
            write_json(&self.delegate_keys()),
            true,
        ));
        files.push(("wallets.json", write_json(&self.wallets()), true));
        files.push(("summary.json", write_json(&self.summary()), false));
        files
    }

    /// `delegate-keys.json`.
    pub fn delegate_keys(&self) -> Value {
        let delegates: Vec<Value> = self
            .input
            .delegates
            .iter()
            .zip(&self.definition.delegates)
            .map(|(delegate, key)| {
                json!({
                    "username": delegate.username,
                    "role": key.role,
                    "address": key.address.to_base58(),
                    "publicKey": key.keys.public_key().to_hex(),
                    "privateKey": key.keys.secret_key().to_hex().as_str(),
                    "passphrase": key.passphrase.as_str(),
                })
            })
            .collect();
        json!({ "warning": WARNING, "delegates": delegates })
    }

    /// `wallets.json`.
    pub fn wallets(&self) -> Value {
        let d = &self.definition;
        let genesis: Vec<Value> = d
            .allocations
            .iter()
            .zip(&self.input.allocations)
            .map(|(key, a)| {
                with(
                    labelled(key),
                    json!({ "genesisBalance": a.amount.to_string() }),
                )
            })
            .collect();
        let donations: Vec<Value> = d
            .donations
            .iter()
            .zip(&self.input.donations)
            .map(|(key, share)| {
                with(
                    labelled(key),
                    json!({
                        "genesisBalance": "0",
                        "basisPoints": share.basis_points,
                        "fromHeight": self.input.donations_from_height,
                    }),
                )
            })
            .collect();
        let wallets: Vec<Value> = d.wallets.iter().map(key_json).collect();
        json!({
            "warning": WARNING,
            "generator": with(key_json(&d.generator), json!({ "issued": d.summary.issued.to_string() })),
            "genesis": genesis,
            "donations": donations,
            "wallets": wallets,
        })
    }

    /// `summary.json`.
    pub fn summary(&self) -> Value {
        let d = &self.definition;
        let digests: Map<String, Value> = [
            (CRYPTO_FILES[0], &d.network_json),
            (CRYPTO_FILES[1], &d.milestones_json),
            (CRYPTO_FILES[2], &d.genesis_block_json),
            (CRYPTO_FILES[3], &d.exceptions_json),
        ]
        .into_iter()
        .map(|(file, text)| (file.to_owned(), Value::from(sha256_hex(text.as_bytes()))))
        .collect();
        json!({
            "profile": self.profile.name,
            "epoch": self.input.epoch,
            "seedSha256": sha256_hex(self.input.seed.as_bytes()),
            "nethash": hex::encode(&d.summary.nethash),
            "genesisId": d.summary.genesis_id.to_hex(),
            "genesisTransactions": d.summary.transactions,
            "issued": d.summary.issued.to_string(),
            "files": digests,
        })
    }

    /// Write the files into `dir`, which must be empty or missing unless `force` is set. Returns
    /// the paths written.
    pub fn write(&self, dir: &Path, force: bool) -> Result<Vec<PathBuf>> {
        if !force && dir.exists() {
            let mut entries = fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
            if entries.next().is_some() {
                return Err(Error::Usage(format!(
                    "{} is not empty; pass --force to overwrite its files",
                    dir.display()
                )));
            }
        }
        let crypto = dir.join("crypto");
        fs::create_dir_all(&crypto).map_err(|e| Error::io(&crypto, e))?;
        let mut written = Vec::new();
        for (name, text, secret) in self.files() {
            let path = dir.join(name);
            write_file(&path, &text, secret)?;
            written.push(path);
        }
        Ok(written)
    }
}

fn write_file(path: &Path, text: &str, secret: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if secret {
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| Error::io(path, e))?;
    if secret {
        // The mode above applies only to a new file.
        file.set_permissions(Permissions::from_mode(0o600))
            .map_err(|e| Error::io(path, e))?;
    }
    file.write_all(text.as_bytes())
        .map_err(|e| Error::io(path, e))
}

fn write_json(value: &Value) -> String {
    let mut text = js::stringify(value, 4);
    text.push('\n');
    text
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(&sha256(bytes))
}

fn key_json(key: &DerivedKey) -> Value {
    json!({
        "role": key.role,
        "address": key.address.to_base58(),
        "publicKey": key.keys.public_key().to_hex(),
        "passphrase": key.passphrase.as_str(),
    })
}

/// [`key_json`] with a `label` first, the role, as the devnet generator labels the genesis and
/// donation wallets (the reference's `verify-network.js` reads them).
fn labelled(key: &DerivedKey) -> Value {
    with(json!({ "label": key.role }), key_json(key))
}

fn with(mut value: Value, extra: Value) -> Value {
    if let (Value::Object(map), Value::Object(extra)) = (&mut value, extra) {
        map.extend(extra);
    }
    value
}

/// Compare `built` with the directory `dir` that the reference's devnet generator wrote: the four
/// crypto files byte for byte, then every field of `delegate-keys.json` and `wallets.json` that
/// the driver writes too, and the nethash, genesis id, transaction count and issued amount of
/// `network-summary.json`. Returns the differences, one line each.
pub fn compare(built: &Built, dir: &Path) -> Result<Vec<String>> {
    let mut differences = Vec::new();
    let ours = built.files();
    for (name, text, _) in ours.iter().filter(|(name, ..)| name.starts_with("crypto/")) {
        let path = dir.join(name);
        let theirs = fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        if let Some(line) = first_difference(text, &theirs) {
            differences.push(format!("{name}: differs from line {line}"));
        }
    }
    let read = |name: &str| -> Result<Value> {
        let path = dir.join(name);
        let text = fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        serde_json::from_str(&text).map_err(|e| Error::Parse {
            path,
            line: e.line(),
            message: e.to_string(),
        })
    };
    let mut delegate_keys = built.delegate_keys();
    let mut wallets = built.wallets();
    for value in [&mut delegate_keys, &mut wallets] {
        if let Value::Object(map) = value {
            map.shift_remove("warning");
        }
    }
    compare_fields(
        "delegate-keys.json",
        &delegate_keys,
        &read("delegate-keys.json")?,
        &mut differences,
    );
    compare_fields(
        "wallets.json",
        &wallets,
        &read("wallets.json")?,
        &mut differences,
    );
    let summary = built.summary();
    let wanted = ["nethash", "genesisId", "genesisTransactions", "issued"];
    let summary: Map<String, Value> = wanted
        .iter()
        .filter_map(|key| summary.get(*key).map(|v| ((*key).to_owned(), v.clone())))
        .collect();
    compare_fields(
        "network-summary.json",
        &Value::Object(summary),
        &read("network-summary.json")?,
        &mut differences,
    );
    Ok(differences)
}

/// The 1-based line at which two texts first differ, if they differ.
fn first_difference(a: &str, b: &str) -> Option<usize> {
    if a == b {
        return None;
    }
    let same = a
        .split_inclusive('\n')
        .zip(b.split_inclusive('\n'))
        .take_while(|(x, y)| x == y)
        .count();
    Some(same + 1)
}

/// Compare every field of `ours` with the same field of `theirs`, which may have more fields.
fn compare_fields(at: &str, ours: &Value, theirs: &Value, differences: &mut Vec<String>) {
    match (ours, theirs) {
        (Value::Object(ours), Value::Object(theirs)) => {
            for (key, value) in ours {
                match theirs.get(key) {
                    Some(other) => {
                        compare_fields(&format!("{at}.{key}"), value, other, differences)
                    }
                    None => differences.push(format!("{at}.{key}: missing")),
                }
            }
        }
        (Value::Array(ours), Value::Array(theirs)) => {
            if ours.len() != theirs.len() {
                differences.push(format!(
                    "{at}: {} entries, the generator has {}",
                    ours.len(),
                    theirs.len()
                ));
            }
            for (i, (value, other)) in ours.iter().zip(theirs).enumerate() {
                compare_fields(&format!("{at}[{i}]"), value, other, differences);
            }
        }
        _ if ours == theirs => {}
        _ => differences.push(format!("{at}: {ours} here, {theirs} from the generator")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors::testutil::TestDir;

    const V10: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/crypto/tests/vectors/V10-genesis.jsonl"
    ));

    /// The `genesis.generate` records of the genesis vectors.
    fn records() -> Vec<Value> {
        V10.lines()
            .skip(1)
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|r| r["op"] == "genesis.generate")
            .collect()
    }

    fn profile_of(record: &Value) -> Profile {
        let text = serde_json::to_string(&record["input"]["profile"]).unwrap();
        Profile::parse(Path::new("profile.json"), &text).unwrap()
    }

    #[test]
    fn the_vector_profile_parses_into_the_builder_input() {
        let record = &records()[0];
        let profile = profile_of(record);
        let input = profile
            .input(Path::new("p"), "seed", "2026-01-01T00:00:00Z")
            .unwrap();
        assert_eq!(input.epoch, "2026-01-01T00:00:00.000Z");
        assert_eq!(input.delegates.len(), profile.delegates.count as usize);
        assert_eq!(input.wallet_roles.len(), profile.wallets.count as usize);
        let first = &input.delegates[0];
        assert_eq!(first.role, format!("{}1", profile.delegates.role_prefix));
        assert_eq!(
            first.username,
            format!("{}1", profile.delegates.username_prefix)
        );
        assert_eq!(input.overrides.len(), profile.overrides.len());
        assert_eq!(
            input.rules.keys().collect::<Vec<_>>(),
            profile.rules.keys().collect::<Vec<_>>()
        );
        let supply: u64 = input.allocations.iter().map(|a| a.amount).sum();
        assert_eq!(supply.to_string(), profile.supply);
    }

    #[test]
    fn bad_profiles_are_refused() {
        let record = &records()[0];
        let good = record["input"]["profile"].clone();
        let refused = |change: &dyn Fn(&mut Value)| {
            let mut value = good.clone();
            change(&mut value);
            let text = serde_json::to_string(&value).unwrap();
            let path = Path::new("p.json");
            Profile::parse(path, &text).and_then(|p| p.input(path, "seed", "2026-01-01T00:00:00Z"))
        };
        assert!(refused(&|_| {}).is_ok());
        for change in [
            &(|v: &mut Value| v["extra"] = json!(1)) as &dyn Fn(&mut Value),
            &|v| v["network"]["pubKeyHash"] = json!(256),
            &|v| v["network"]["nethash"] = json!(""),
            &|v| v["allocations"][0]["amount"] = json!("01"),
            &|v| v["allocations"][0]["amount"] = json!("1.5"),
            &|v| v["allocations"][0]["amount"] = json!(1),
            &|v| v["allocations"][0]["amount"] = json!("18446744073709551616"),
            &|v| v["donations"][0]["basisPoints"] = json!(70000),
            &|v| {
                v.as_object_mut().unwrap().remove("generatorRole");
            },
        ] {
            assert!(
                matches!(refused(change), Err(Error::Profile { .. })),
                "{:?}",
                refused(change)
            );
        }
    }

    #[test]
    fn epochs_without_milliseconds_are_normalised() {
        assert_eq!(
            normalise_epoch("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00.000Z"
        );
        for unchanged in ["2026-01-01T00:00:00.000Z", "2026-01-01", "", "x"] {
            assert_eq!(normalise_epoch(unchanged), unchanged);
        }
    }

    #[test]
    fn first_difference_names_the_line() {
        assert_eq!(first_difference("a\nb\n", "a\nb\n"), None);
        assert_eq!(first_difference("a\nb\n", "a\nc\n"), Some(2));
        assert_eq!(first_difference("a\n", "a\nb\n"), Some(2));
        assert_eq!(first_difference("a", "a\n"), Some(1));
    }

    #[test]
    fn compare_fields_reports_missing_and_different_values() {
        let mut differences = Vec::new();
        compare_fields(
            "f",
            &json!({ "a": 1, "b": [1, 2], "c": { "d": "x" } }),
            &json!({ "a": 1, "b": [1, 3, 4], "c": {}, "e": 5 }),
            &mut differences,
        );
        assert_eq!(
            differences,
            [
                "f.b: 2 entries, the generator has 3",
                "f.b[1]: 2 here, 3 from the generator",
                "f.c.d: missing"
            ]
        );
    }

    /// Write, in `dir`, the parts of a devnet generator's output that a genesis record holds.
    fn generator_output(record: &Value, dir: &Path) {
        let output = &record["output"];
        fs::create_dir_all(dir.join("crypto")).unwrap();
        for file in CRYPTO_FILES {
            fs::write(dir.join(file), output["files"][file].as_str().unwrap()).unwrap();
        }
        let json = |name: &str, value: Value| {
            fs::write(dir.join(name), serde_json::to_string(&value).unwrap()).unwrap();
        };
        json(
            "delegate-keys.json",
            json!({ "delegates": output["delegates"] }),
        );
        // The generator labels its genesis and donation wallets with their roles.
        let labelled = |wallets: &Value| -> Value {
            wallets
                .as_array()
                .unwrap()
                .iter()
                .map(|w| with(json!({ "label": w["role"] }), w.clone()))
                .collect()
        };
        json(
            "wallets.json",
            json!({
                "generator": output["generator"],
                "genesis": labelled(&output["genesisWallets"]),
                "donations": labelled(&output["donationWallets"]),
                "wallets": output["testWallets"],
            }),
        );
        json(
            "network-summary.json",
            json!({
                "nethash": output["nethash"],
                "genesisId": output["genesisId"],
                "genesisTransactions": output["transactions"],
                "issued": output["issued"],
            }),
        );
    }

    #[test]
    fn the_driver_writes_what_the_generator_wrote() {
        for record in records() {
            let seed = record["input"]["seed"].as_str().unwrap();
            let epoch = record["input"]["epoch"].as_str().unwrap();
            let built =
                Built::from_profile(Path::new("p.json"), profile_of(&record), seed, epoch).unwrap();

            let out = TestDir::new("genesis-out");
            let written = built.write(out.path(), false).unwrap();
            assert_eq!(written.len(), 7);
            for file in CRYPTO_FILES {
                assert_eq!(
                    fs::read_to_string(out.path().join(file)).unwrap(),
                    record["output"]["files"][file].as_str().unwrap(),
                    "{file}"
                );
            }
            for secret in ["delegate-keys.json", "wallets.json"] {
                let mode = fs::metadata(out.path().join(secret))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600, "{secret}");
            }
            assert!(matches!(
                built.write(out.path(), false),
                Err(Error::Usage(_))
            ));
            assert_eq!(built.write(out.path(), true).unwrap().len(), 7);

            let generator = TestDir::new("genesis-generator");
            generator_output(&record, generator.path());
            assert_eq!(
                compare(&built, generator.path()).unwrap(),
                Vec::<String>::new()
            );
            let path = generator.path().join(CRYPTO_FILES[1]);
            let changed = fs::read_to_string(&path)
                .unwrap()
                .replacen("9000", "9001", 1);
            fs::write(&path, changed).unwrap();
            let differences = compare(&built, generator.path()).unwrap();
            assert_eq!(differences.len(), 1, "{differences:?}");
            assert!(differences[0].starts_with("crypto/milestones.json"));
        }
    }

    #[test]
    fn a_supply_that_is_not_issued_is_refused() {
        let record = &records()[0];
        let mut profile = profile_of(record);
        profile.supply = "1".into();
        let seed = record["input"]["seed"].as_str().unwrap();
        let epoch = record["input"]["epoch"].as_str().unwrap();
        assert!(matches!(
            Built::from_profile(Path::new("p.json"), profile, seed, epoch),
            Err(Error::Profile { .. })
        ));
    }
}
