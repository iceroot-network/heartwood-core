//! Class V04: BIP340 and legacy Schnorr signatures, the preimages of transaction signatures, and
//! the peer status messages that the legacy scheme signs.

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig;
use heartwood_crypto::errors::SigError;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, PublicKey, SchemeId, SecretKey, Signature, SigningDomain};

use crate::common::{self, VectorFile};
use crate::support::{Case, assert_coverage, cases, split_legacy};

/// Every signing domain; none of them changes the bytes of a signature.
pub const DOMAINS: [SigningDomain; 3] = [
    SigningDomain::Transaction,
    SigningDomain::Block,
    SigningDomain::PeerStatus,
];

fn file() -> VectorFile {
    vector_file!("V04-signatures")
}

/// Checks a `sig.bip340.sign` record: the signature under every domain, and that it verifies.
pub fn check_bip340_sign(case: Case<'_>) {
    let secret = SecretKey::from_hex(case.text("privateKey")).unwrap();
    let aux = hex::decode_array::<32>(case.text("aux")).unwrap();
    let message = case.bytes("message");
    let expected = case.output_text("signature");
    for domain in DOMAINS {
        let signature = sig::sign(
            SchemeId::Secp256k1Bip340,
            domain,
            &message,
            &secret,
            Aux::Fixed(aux),
        )
        .unwrap();
        assert_eq!(signature.to_hex(), expected, "{} {domain:?}", case.label());
    }
    let signature = Signature::from_hex(expected).unwrap();
    let public = PublicKey::from_secret_key(&secret);
    assert!(
        sig::verify(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Transaction,
            &message,
            &signature,
            public.as_bytes()
        ),
        "{}",
        case.label()
    );
}

/// Checks a `sig.legacy.sign` record: the scheme takes no aux, so any aux gives the same bytes.
#[cfg(feature = "legacy-schnorr")]
pub fn check_legacy_sign(case: Case<'_>) {
    let secret = SecretKey::from_hex(case.text("privateKey")).unwrap();
    let message = case.bytes("message");
    let expected = case.output_text("signature");
    for (domain, aux) in
        DOMAINS
            .into_iter()
            .zip([Aux::Random, Aux::Fixed([0; 32]), Aux::Fixed([0xff; 32])])
    {
        let signature =
            sig::sign(SchemeId::Secp256k1Legacy, domain, &message, &secret, aux).unwrap();
        assert_eq!(signature.to_hex(), expected, "{} {domain:?}", case.label());
    }
    let signature = Signature::from_hex(expected).unwrap();
    let public = PublicKey::from_secret_key(&secret);
    assert!(
        sig::verify(
            SchemeId::Secp256k1Legacy,
            SigningDomain::PeerStatus,
            &message,
            &signature,
            public.as_bytes()
        ),
        "{}",
        case.label()
    );
}

/// Checks a `sig.*.verify` record under `scheme`. A signature that is not 64 bytes long cannot be
/// built, which is the reference's verdict for it: not valid.
pub fn check_verify(case: Case<'_>, scheme: SchemeId) {
    let message = case.bytes("message");
    let key = case.bytes("publicKey");
    let verdict = match Signature::from_bytes(&case.bytes("signature")) {
        Ok(signature) => {
            let verdicts =
                DOMAINS.map(|domain| sig::verify(scheme, domain, &message, &signature, &key));
            assert!(
                verdicts.iter().all(|&v| v == verdicts[0]),
                "{}",
                case.label()
            );
            verdicts[0]
        }
        Err(SigError::InvalidLength { actual }) => {
            assert_ne!(actual, 64, "{}", case.label());
            false
        }
        Err(other) => panic!("{}: {other}", case.label()),
    };
    assert_eq!(verdict, case.valid(), "{}", case.label());
}

#[test]
fn every_record_is_checked() {
    let file = file();
    assert_eq!(file.records.len(), 704);
    let legacy = file
        .records
        .iter()
        .filter(|record| record.op.starts_with("sig.legacy."))
        .count();
    assert_eq!(legacy, 353);
    let (checked, elsewhere) = split_legacy(&[
        "sig.bip340.sign",
        "sig.bip340.verify",
        "sig.legacy.sign",
        "sig.legacy.verify",
        "tx.preimages",
        "status.preimage",
    ]);
    assert_coverage(&file, &checked, &elsewhere);
}

#[test]
fn bip340_sign() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "sig.bip340.sign") {
        check_bip340_sign(case);
        checked += 1;
    }
    assert_eq!(checked, 59);
}

#[test]
fn bip340_verify() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "sig.bip340.verify") {
        check_verify(case, SchemeId::Secp256k1Bip340);
        checked += 1;
    }
    assert_eq!(checked, 279);
}

#[cfg(feature = "legacy-schnorr")]
#[test]
fn legacy_sign() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "sig.legacy.sign") {
        check_legacy_sign(case);
        checked += 1;
    }
    assert_eq!(checked, 68);
}

#[cfg(feature = "legacy-schnorr")]
#[test]
fn legacy_verify() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "sig.legacy.verify") {
        check_verify(case, SchemeId::Secp256k1Legacy);
        checked += 1;
    }
    assert_eq!(checked, 285);
}

/// The sign record of `file` with this name.
fn sign_record<'a>(file: &'a VectorFile, name: &str) -> Option<Case<'a>> {
    cases(file, "sig.bip340.sign").find(|case| case.name() == name)
}

/// Offset of the sender's public key in a version 3 transaction: marker, version, network, type
/// group (4 bytes), type (2) and nonce (8) come first.
const SENDER_KEY: std::ops::Range<usize> = 17..50;

/// `tx.preimages`: what each signature of a transaction signs. The hashes, the prefix relation, and
/// the signatures that follow each preimage are checked here, with the signer's key; the preimage
/// bytes themselves come from the transaction serialiser and are its tests' concern.
#[test]
fn transaction_preimages() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "tx.preimages") {
        let label = case.label();
        let transaction = case.bytes("hex");
        let output = case.output();
        assert_eq!(
            hex::encode(&sha256(&transaction)),
            case.output_text("id"),
            "{label}"
        );

        let part = |key: &str| {
            let bytes = hex::decode(output[key]["bytes"].as_str().unwrap()).unwrap();
            let hash = hex::decode_array::<32>(output[key]["hash"].as_str().unwrap()).unwrap();
            assert_eq!(sha256(&bytes), hash, "{label} {key}");
            assert!(transaction.starts_with(&bytes), "{label} {key}");
            (bytes, hash)
        };
        let (first_preimage, first_hash) = part("signature");
        let (second_preimage, second_hash) = part("secondSignature");
        // No multisignature type is decoded; the reference reports the first preimage for it.
        assert_eq!(part("multiSignature"), (first_preimage.clone(), first_hash));

        // The first signature follows its preimage and verifies with the sender's key.
        let sender = &transaction[SENDER_KEY];
        let first = &transaction[first_preimage.len()..first_preimage.len() + 64];
        let first = Signature::from_bytes(first).unwrap();
        assert!(
            sig::verify(
                SchemeId::Secp256k1Bip340,
                SigningDomain::Transaction,
                &first_hash,
                &first,
                sender
            ),
            "{label}"
        );
        let signer = sign_record(&file, &format!("{}: first signature", case.name()))
            .unwrap_or_else(|| panic!("{label}: no first signature record"));
        assert_eq!(signer.bytes("message"), first_hash, "{label}");
        assert_eq!(signer.output_text("signature"), first.to_hex(), "{label}");
        let signer_key =
            PublicKey::from_secret_key(&SecretKey::from_hex(signer.text("privateKey")).unwrap());
        assert_eq!(signer_key.as_bytes(), sender, "{label}");

        if second_preimage.len() == transaction.len() {
            // Signed once: the second preimage is the whole transaction, hashed to its id.
            assert_eq!(hex::encode(&second_hash), case.output_text("id"), "{label}");
            assert_eq!(second_preimage.len(), first_preimage.len() + 64, "{label}");
        } else {
            // Signed twice: the first signature is part of the second preimage.
            assert_eq!(second_preimage.len(), first_preimage.len() + 64, "{label}");
            assert_eq!(transaction.len(), second_preimage.len() + 64, "{label}");
            let second = Signature::from_bytes(&transaction[second_preimage.len()..]).unwrap();
            let signer = sign_record(&file, &format!("{}: second signature", case.name()))
                .unwrap_or_else(|| panic!("{label}: no second signature record"));
            assert_eq!(signer.bytes("message"), second_hash, "{label}");
            assert_eq!(signer.output_text("signature"), second.to_hex(), "{label}");
            let key = PublicKey::from_secret_key(
                &SecretKey::from_hex(signer.text("privateKey")).unwrap(),
            );
            assert!(
                sig::verify(
                    SchemeId::Secp256k1Bip340,
                    SigningDomain::Transaction,
                    &second_hash,
                    &second,
                    key.as_bytes()
                ),
                "{label}"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 8);
}

/// `status.preimage`: the peer status messages that the legacy scheme signs. Building the message
/// from a block belongs to the peer-to-peer codec; here each message must be the one that both
/// legacy sign records of its status sign.
#[test]
fn status_preimages() {
    let file = file();
    let mut checked = 0;
    for case in cases(&file, "status.preimage") {
        let preimage = case.output_text("preimage");
        let signers: Vec<Case<'_>> = cases(&file, "sig.legacy.sign")
            .filter(|signer| {
                signer
                    .name()
                    .strip_prefix(case.name())
                    .is_some_and(|rest| rest.starts_with(", "))
            })
            .collect();
        assert_eq!(signers.len(), 2, "{}", case.label());
        for signer in signers {
            assert_eq!(signer.text("message"), preimage, "{}", signer.label());
        }
        checked += 1;
    }
    assert_eq!(checked, 5);
}
