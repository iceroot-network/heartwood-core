//! The key file: where it is, its permission check, its keys and its refusals, with the
//! reference's messages where the reference has one.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use heartwood_crypto::{KeyPair, SecretKey};
use heartwood_forger::{DelegateKeys, KeysError, load_keys};

/// A configuration directory of its own for each test, removed when dropped.
struct Config(PathBuf);

impl Config {
    fn new(name: &str) -> Config {
        let dir = std::env::temp_dir().join(format!(
            "heartwood-forger-keys-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Config(dir)
    }

    /// Write `text` as `delegates.json` with `mode`.
    fn write(&self, text: &str, mode: u32) -> &Path {
        let file = self.0.join("delegates.json");
        let _ = fs::remove_file(&file);
        fs::write(&file, text).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        &self.0
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The secret key of a passphrase, in hex.
fn secret(passphrase: &str) -> String {
    let keys = KeyPair::from_passphrase(passphrase).unwrap();
    keys.secret_key().to_hex().as_str().to_owned()
}

fn public(passphrase: &str) -> heartwood_crypto::PublicKeyBytes {
    KeyPair::from_passphrase(passphrase)
        .unwrap()
        .public_key()
        .to_compressed()
}

fn keys_file(keys: &[String]) -> String {
    serde_json::json!({ "keys": keys }).to_string()
}

#[test]
fn a_private_file_gives_its_keys_in_file_order() {
    let config = Config::new("order");
    let (a, b) = (secret("delegate a"), secret("delegate b"));
    let loaded = load_keys(config.write(&keys_file(&[b.clone(), a.clone()]), 0o600)).unwrap();
    assert_eq!(
        loaded.declared(),
        [public("delegate b"), public("delegate a")]
    );
    let delegates = loaded.delegates(90);
    let keys: Vec<_> = delegates.iter().map(|d| *d.public_key()).collect();
    assert_eq!(keys, [public("delegate b"), public("delegate a")]);
    assert_eq!(
        delegates[0].address().to_base58(),
        heartwood_crypto::Address::from_public_key(
            KeyPair::from_passphrase("delegate b").unwrap().public_key(),
            90
        )
        .to_base58()
    );
    let key = loaded.key(&public("delegate a")).unwrap();
    assert_eq!(key.secret_key().to_hex().as_str(), a);
    assert!(loaded.key(&public("delegate c")).is_none());
    assert_eq!(
        heartwood_forger::keys::path(Path::new("/c")),
        Path::new("/c/delegates.json")
    );
}

#[test]
fn a_file_its_group_or_others_can_read_is_refused() {
    let config = Config::new("mode");
    let text = keys_file(&[secret("delegate a")]);
    for mode in [0o600, 0o400, 0o620, 0o602] {
        assert!(load_keys(config.write(&text, mode)).is_ok(), "{mode:o}");
    }
    for mode in [0o640, 0o604, 0o644, 0o660, 0o606] {
        let error = load_keys(config.write(&text, mode)).unwrap_err();
        assert_eq!(
            error,
            KeysError::Readable {
                path: config.0.join("delegates.json"),
                mode,
            }
        );
        assert!(error.to_string().contains("chmod 600"), "{error}");
    }
}

#[test]
fn a_missing_file_and_a_file_without_keys_have_the_references_messages() {
    let config = Config::new("missing");
    let error = load_keys(&config.0).unwrap_err();
    let file = config.0.join("delegates.json");
    assert_eq!(error, KeysError::Missing { path: file.clone() });
    assert_eq!(
        error.to_string(),
        format!("The {} file does not exist", file.display())
    );
    for text in [
        r#"{}"#,
        r#"{"keys":[]}"#,
        r#"{"keys":[],"secrets":[]}"#,
        "[]",
        r#"{"other":1}"#,
    ] {
        let error = load_keys(config.write(text, 0o600)).unwrap_err();
        assert_eq!(error, KeysError::NoKeys, "{text}");
        assert_eq!(error.to_string(), "Unable to detect any private keys");
    }
}

#[test]
fn secrets_are_refused_unless_empty() {
    let config = Config::new("secrets");
    let key = secret("delegate a");
    let text = serde_json::json!({ "keys": [key], "secrets": [] }).to_string();
    assert!(load_keys(config.write(&text, 0o600)).is_ok());
    for text in [
        serde_json::json!({ "keys": [key], "secrets": ["a passphrase"] }).to_string(),
        serde_json::json!({ "secrets": ["a passphrase"] }).to_string(),
    ] {
        let error = load_keys(config.write(&text, 0o600)).unwrap_err();
        assert!(matches!(error, KeysError::Secrets { .. }), "{error:?}");
        assert!(!error.to_string().contains("a passphrase"));
    }
}

#[test]
fn a_file_the_schema_refuses_is_refused_without_its_keys_in_the_message() {
    let config = Config::new("schema");
    let key = secret("delegate a");
    let cases = [
        "not json".to_owned(),
        serde_json::json!({ "keys": key }).to_string(),
        serde_json::json!({ "keys": [key], "extra": true }).to_string(),
        serde_json::json!({ "keys": [&key[..62]] }).to_string(),
        serde_json::json!({ "keys": [format!("{}zz", &key[..62])] }).to_string(),
        serde_json::json!({ "keys": [1] }).to_string(),
        // Zero and the curve order are not secret keys.
        serde_json::json!({ "keys": ["00".repeat(32)] }).to_string(),
        serde_json::json!({
            "keys": ["fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141"]
        })
        .to_string(),
    ];
    for text in cases {
        let error = load_keys(config.write(&text, 0o600)).unwrap_err();
        assert!(
            matches!(error, KeysError::Invalid { .. }),
            "{text}: {error:?}"
        );
        assert!(!error.to_string().contains(&key[..16]), "{error}");
    }
}

#[test]
fn uppercase_hex_is_a_key() {
    let config = Config::new("uppercase");
    let key = secret("delegate a");
    let loaded = load_keys(config.write(&keys_file(&[key.to_uppercase()]), 0o600)).unwrap();
    assert_eq!(loaded.declared(), [public("delegate a")]);
    assert_eq!(
        loaded
            .key(&public("delegate a"))
            .unwrap()
            .secret_key()
            .to_hex()
            .as_str(),
        key
    );
}

#[test]
fn duplicates_forge_once_and_are_declared_as_the_reference_signs_them() {
    let config = Config::new("duplicates");
    let (a, b) = (secret("delegate a"), secret("delegate b"));
    let listed = [a.clone(), b.clone(), a.clone(), a.to_uppercase()];
    let loaded = load_keys(config.write(&keys_file(&listed), 0o600)).unwrap();
    let (pa, pb) = (public("delegate a"), public("delegate b"));
    // Without `secrets`, every entry is declared, as the reference signs every entry.
    assert_eq!(loaded.declared(), [pa, pb, pa, pa]);
    let keys: Vec<_> = loaded
        .delegates(90)
        .iter()
        .map(|d| *d.public_key())
        .collect();
    assert_eq!(keys, [pa, pb]);
    // With `secrets`, the reference rewrites the keys through a set of their texts.
    let text = serde_json::json!({ "keys": listed, "secrets": [] }).to_string();
    let loaded = load_keys(config.write(&text, 0o600)).unwrap();
    assert_eq!(loaded.declared(), [pa, pb, pa]);
    assert_eq!(loaded.delegates(90).len(), 2);
}

#[test]
fn keys_given_directly_are_declared_as_given() {
    let pair = |p: &str| KeyPair::from_secret_key(SecretKey::from_hex(&secret(p)).unwrap());
    let keys = DelegateKeys::from_keys(vec![pair("delegate a"), pair("delegate a")]);
    assert_eq!(
        keys.declared(),
        [public("delegate a"), public("delegate a")]
    );
    assert_eq!(keys.delegates(90).len(), 1);
}
