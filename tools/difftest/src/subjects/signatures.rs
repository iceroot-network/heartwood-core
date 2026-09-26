//! BIP340 and legacy Schnorr signatures, the preimages of transaction signatures, the published
//! vector files and BIP39 mnemonics.

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig;
use heartwood_crypto::genesis::bip39;
use heartwood_crypto::transactions::deserialiser;
use heartwood_crypto::transactions::serialiser::{self, SerialiseOptions};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, SchemeId, SecretKey, Signature, SigningDomain};
use serde_json::{Value, json};

use super::{Env, Outcome, bytes32, decode_class, hex_bytes, params, rejected, text};
use crate::subject::{Case, SubjectError, Verdict};

/// The published vector files of the external class, as committed beside the vectors.
const EXTERNAL_FILES: [(&str, &[u8]); 3] = [
    (
        "external/bip340-test-vectors.csv",
        include_bytes!(
            "../../../../packages/crypto/tests/vectors/external/bip340-test-vectors.csv"
        ),
    ),
    (
        "external/bip-schnorr-d194620-test-vectors.csv",
        include_bytes!(
            "../../../../packages/crypto/tests/vectors/external/bip-schnorr-d194620-test-vectors.csv"
        ),
    ),
    (
        "external/bip39-vectors.json",
        include_bytes!("../../../../packages/crypto/tests/vectors/external/bip39-vectors.json"),
    ),
];

/// Sign `input.message` with `input.privateKey` under `scheme`.
fn sign(case: &Case<'_>, scheme: SchemeId, domain: SigningDomain, aux: Aux) -> Outcome<Verdict> {
    let (Some(message), Ok(secret)) = (
        hex_bytes(case.input, "message")?,
        SecretKey::from_hex(text(case.input, "privateKey")?),
    ) else {
        return Ok(rejected("Error"));
    };
    Ok(match sig::sign(scheme, domain, &message, &secret, aux) {
        Ok(signature) => Verdict::Output(json!({ "signature": signature.to_hex() })),
        Err(error) => rejected(format!("{error:?}")),
    })
}

/// Verify `input.signature` over `input.message` with `input.publicKey` under `scheme`. A
/// signature that is not 64 bytes long is not valid, as in the reference.
fn verify(case: &Case<'_>, scheme: SchemeId, domain: SigningDomain) -> Outcome<Verdict> {
    let (Some(message), Some(signature), Some(key)) = (
        hex_bytes(case.input, "message")?,
        hex_bytes(case.input, "signature")?,
        hex_bytes(case.input, "publicKey")?,
    ) else {
        return Ok(rejected("Error"));
    };
    let valid = Signature::from_bytes(&signature)
        .is_ok_and(|signature| sig::verify(scheme, domain, &message, &signature, &key));
    Ok(Verdict::Output(json!({ "valid": valid })))
}

/// `sig.bip340.sign`: a 32-byte message is signed as given, any other length is hashed first.
pub(super) fn bip340_sign(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let aux = Aux::fixed(bytes32(case.input, "aux")?);
    sign(
        case,
        SchemeId::Secp256k1Bip340,
        SigningDomain::Transaction,
        aux,
    )
}

/// `sig.bip340.verify`.
pub(super) fn bip340_verify(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    verify(case, SchemeId::Secp256k1Bip340, SigningDomain::Transaction)
}

/// `sig.legacy.sign`: deterministic; the scheme takes no aux.
pub(super) fn legacy_sign(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    sign(
        case,
        SchemeId::Secp256k1Legacy,
        SigningDomain::PeerStatus,
        // Not used: the legacy scheme derives its nonce from the key and the message.
        Aux::random(),
    )
}

/// `sig.legacy.verify`.
pub(super) fn legacy_verify(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    verify(case, SchemeId::Secp256k1Legacy, SigningDomain::PeerStatus)
}

/// `tx.preimages`: what each signature of a decoded transaction covers. The first signature
/// covers the fields without signatures, the second one the fields with the first signature, and a
/// multi-signature entry (which no kept type carries) the fields without signatures.
pub(super) fn transaction_preimages(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("Error"));
    };
    let data = match deserialiser::deserialise(&bytes) {
        Ok(data) => data,
        Err(error) => return Ok(rejected(decode_class(&error))),
    };
    let entry = |options: SerialiseOptions| -> Outcome<Value> {
        let bytes = serialiser::serialise(&data, options, params)
            .map_err(|e| super::failed(format!("a decoded transaction does not serialise: {e}")))?;
        Ok(json!({ "bytes": hex::encode(&bytes), "hash": hex::encode(&sha256(&bytes)) }))
    };
    let Ok(id) = serialiser::transaction_id(&data, params) else {
        return Ok(rejected("Error"));
    };
    Ok(Verdict::Output(json!({
        "id": id.to_hex(),
        "signature": entry(SerialiseOptions::UNSIGNED)?,
        "secondSignature": entry(SerialiseOptions::FOR_SECOND_SIGNATURE)?,
        "multiSignature": entry(SerialiseOptions::UNSIGNED)?,
    })))
}

/// `status.preimage`: the peer status message is written by the peer-to-peer codec, which is not
/// part of `heartwood-crypto`. The crate's tests check each message as what the legacy scheme
/// signs.
pub(super) fn status_preimage(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported(
        "the peer status message belongs to the peer-to-peer codec".into(),
    ))
}

/// `external.file`: the committed copy of a published vector file has the length and SHA-256 of
/// the file the oracle read.
pub(super) fn external_file(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let name = text(case.input, "file")?;
    let (_, bytes) = EXTERNAL_FILES
        .iter()
        .find(|(file, _)| *file == name)
        .ok_or_else(|| SubjectError::Unsupported(format!("no committed copy of {name}")))?;
    Ok(Verdict::Output(json!({
        "sha256": hex::encode(&sha256(bytes)),
        "length": bytes.len(),
    })))
}

/// `bip39.entropyToMnemonic`: the English mnemonic of 16 to 32 bytes of entropy.
pub(super) fn entropy_to_mnemonic(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let Some(entropy) = hex_bytes(case.input, "entropy")? else {
        return Ok(rejected("TypeError"));
    };
    Ok(match bip39::entropy_to_mnemonic(&entropy) {
        Ok(mnemonic) => Verdict::Output(json!({ "mnemonic": mnemonic.as_str() })),
        Err(error) => rejected(format!("{error:?}")),
    })
}
