//! Class V11, the BIP340 and bip-schnorr parts: the published vector files as the reference runs
//! them. BIP340 rows whose message is not 32 bytes long are hashed first, as the reference hashes
//! them, so their signatures differ from the published ones; the unit tests of the BIP340 backend
//! run the published file as it is.

use heartwood_crypto::SchemeId;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;

use crate::common::{self, VectorFile};
use crate::signatures::{check_bip340_sign, check_verify};
use crate::support::{assert_coverage, cases, split_legacy};

fn file() -> VectorFile {
    vector_file!("V11-external")
}

/// The published files, as committed.
const FILES: [(&str, &[u8]); 3] = [
    (
        "external/bip340-test-vectors.csv",
        include_bytes!("../vectors/external/bip340-test-vectors.csv"),
    ),
    (
        "external/bip-schnorr-d194620-test-vectors.csv",
        include_bytes!("../vectors/external/bip-schnorr-d194620-test-vectors.csv"),
    ),
    (
        "external/bip39-vectors.json",
        include_bytes!("../vectors/external/bip39-vectors.json"),
    ),
];

#[test]
fn every_record_is_checked() {
    let file = file();
    assert_eq!(file.records.len(), 73);
    let (checked, mut elsewhere) = split_legacy(&[
        "external.file",
        "sig.bip340.sign",
        "sig.bip340.verify",
        "sig.legacy.sign",
        "sig.legacy.verify",
    ]);
    elsewhere.push((
        "bip39.entropyToMnemonic",
        "the mnemonic records belong to the genesis tests (feature genesis)",
    ));
    assert_coverage(&file, &checked, &elsewhere);
}

/// The committed files are the ones the oracle read: same length, same SHA-256.
#[test]
fn published_files() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "external.file") {
        let name = case.text("file");
        let (_, bytes) = FILES
            .iter()
            .find(|(path, _)| *path == name)
            .unwrap_or_else(|| panic!("{}: unknown file {name}", case.label()));
        assert_eq!(
            hex::encode(&sha256(bytes)),
            case.output_text("sha256"),
            "{}",
            case.label()
        );
        assert_eq!(
            Some(bytes.len() as u64),
            case.output()["length"].as_u64(),
            "{}",
            case.label()
        );
        checked += 1;
    }
    assert_eq!(checked, 3);
}

#[test]
fn bip340_sign() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "sig.bip340.sign") {
        check_bip340_sign(case);
        checked += 1;
    }
    assert_eq!(checked, 8);
}

#[test]
fn bip340_verify() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "sig.bip340.verify") {
        check_verify(case, SchemeId::Secp256k1Bip340);
        checked += 1;
    }
    assert_eq!(checked, 19);
}

#[cfg(feature = "legacy-schnorr")]
#[test]
fn draft_sign() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "sig.legacy.sign") {
        crate::signatures::check_legacy_sign(case);
        checked += 1;
    }
    assert_eq!(checked, 3);
}

#[cfg(feature = "legacy-schnorr")]
#[test]
fn draft_verify() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "sig.legacy.verify") {
        check_verify(case, SchemeId::Secp256k1Legacy);
        checked += 1;
    }
    assert_eq!(checked, 16);
}

/// The records restate the published rows: each BIP340 verify record has its row's message, key
/// and signature, and the verdict differs from the row's only for messages that are not 32 bytes.
#[test]
fn records_follow_the_published_rows() {
    let file = file();
    let (_, bytes) = FILES[0];
    let text = std::str::from_utf8(bytes).unwrap();
    let rows: Vec<Vec<&str>> = text
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.splitn(8, ',').map(str::trim).collect())
        .collect();
    let records: Vec<_> = cases(&file, "sig.bip340.verify").collect();
    assert_eq!(rows.len(), records.len());
    for (row, case) in rows.iter().zip(records) {
        assert_eq!(
            case.text("message"),
            row[4].to_ascii_lowercase(),
            "{}",
            case.label()
        );
        assert_eq!(
            case.text("publicKey"),
            row[2].to_ascii_lowercase(),
            "{}",
            case.label()
        );
        assert_eq!(
            case.text("signature"),
            row[5].to_ascii_lowercase(),
            "{}",
            case.label()
        );
        let published = row[6] == "TRUE";
        let hashed = case.bytes("message").len() != 32;
        assert_eq!(case.valid(), published && !hashed, "{}", case.label());
    }
}
