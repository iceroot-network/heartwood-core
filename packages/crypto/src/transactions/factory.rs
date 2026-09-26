//! Making a [`Transaction`] from bytes, fields or JSON.

use serde_json::{Map, Value};

use crate::crypto::sig::Signature;
use crate::errors::{BuildError, DecodeError, JsonError, SchemaError};
use crate::identities::{Address, PublicKey, PublicKeyBytes};
use crate::managers::Params;
use crate::transactions::TransactionType;
use crate::transactions::codec::VERSION;
use crate::transactions::deserialiser::{Deserialised, deserialise_raw};
use crate::transactions::registry::TYPE_GROUP_CORE;
use crate::transactions::serialiser::{SerialiseOptions, serialise, transaction_id};
use crate::transactions::types::{
    Asset, Memo, ResignationType, Transaction, TransactionData, TransferItem,
};
use crate::transactions::verifier;
use crate::validation::{
    MAX_AMOUNT, Mode, bigint, bignumber, validate_transaction, votes_from_json,
};

impl Transaction {
    /// Decode `bytes` under the merged milestone `params` of the rule height, as the reference's
    /// strict `TransactionFactory.fromBytes` does: deserialise, compute the id, apply the
    /// transaction rules, and check the sender's signature.
    ///
    /// A bad signature does not refuse the transaction: it decodes with
    /// [`Transaction::is_verified`] false, and the block or pool rules decide.
    pub fn decode(bytes: &[u8], params: &Params) -> Result<Transaction, DecodeError> {
        Transaction::decode_with(bytes, params, Mode::Strict)
    }

    /// [`Transaction::decode`] under the rule variant `mode`.
    ///
    /// The steps are the reference's, in its order. The id is the SHA-256 of the decoded fields
    /// serialised again, which differs from the SHA-256 of `bytes` when the wire form is not the
    /// one this serialiser writes (a version byte 0, a resignation byte `0xff`, repeated vote
    /// names). The rules then remove vote names that are not delegate names, and the signature is
    /// checked over the fields that are left; [`Transaction::bytes`] keeps the bytes as received.
    /// Once the rules have removed a name, the fields serialise to another id: a block from
    /// another node is built again from the fields ([`crate::Block::receive`]), and its payload
    /// hash must cover those ids.
    pub fn decode_with(
        bytes: &[u8],
        params: &Params,
        mode: Mode,
    ) -> Result<Transaction, DecodeError> {
        let Deserialised { mut data, pending } = deserialise_raw(bytes, true)?;
        let id = transaction_id(&data, params).map_err(DecodeError::Id)?;
        if let Some(error) = pending {
            return Err(DecodeError::Schema(error));
        }
        validate_transaction(&mut data, params, mode)?;
        let verified = verifier::verify(&data, params);
        Ok(Transaction {
            data,
            id,
            bytes: bytes.to_vec(),
            verified,
        })
    }

    /// Decode `bytes` that this node verified before, as the reference's
    /// `TransactionFactory.fromBytesUnsafe` does: no transaction rules, no version check and no
    /// signature check, and the transaction counts as verified.
    ///
    /// Callers, each of which must only pass bytes verified before: [`crate::Block::decode_trusted`].
    pub fn decode_trusted(bytes: &[u8], params: &Params) -> Result<Transaction, DecodeError> {
        let Deserialised { data, pending } = deserialise_raw(bytes, false)?;
        let id = transaction_id(&data, params).map_err(DecodeError::Id)?;
        if let Some(error) = pending {
            return Err(DecodeError::Schema(error));
        }
        Ok(Transaction {
            data,
            id,
            bytes: bytes.to_vec(),
            verified: true,
        })
    }

    /// The transaction with fields `data`, as the reference's `TransactionFactory.fromData` makes
    /// it: apply the transaction rules, serialise, then [`Transaction::decode`] the bytes.
    pub fn from_data(data: TransactionData, params: &Params) -> Result<Transaction, BuildError> {
        Transaction::from_data_with(data, params, Mode::Strict)
    }

    /// [`Transaction::from_data`] under the rule variant `mode`.
    pub fn from_data_with(
        mut data: TransactionData,
        params: &Params,
        mode: Mode,
    ) -> Result<Transaction, BuildError> {
        validate_transaction(&mut data, params, mode)?;
        let bytes = serialise(&data, SerialiseOptions::FULL, params)?;
        Ok(Transaction::decode_with(&bytes, params, mode)?)
    }

    /// The transaction in the reference's JSON form, as `TransactionFactory.fromJson` reads it:
    /// the JSON rules of the strict schema, then [`Transaction::from_data`].
    ///
    /// Numbers of the 64-bit fields (nonce, fee, amounts) may be decimal strings or JSON integers,
    /// read as JavaScript's `BigInt` reads them. Vote shares are JSON numbers. A vote asset that is
    /// not a JSON object is refused.
    pub fn from_json(json: &Value, params: &Params) -> Result<Transaction, BuildError> {
        Transaction::from_json_with(json, params, Mode::Strict)
    }

    /// [`Transaction::from_json`] under the rule variant `mode`.
    pub fn from_json_with(
        json: &Value,
        params: &Params,
        mode: Mode,
    ) -> Result<Transaction, BuildError> {
        let data = data_from_json(json, params, mode)?;
        Transaction::from_data_with(data, params, mode)
    }
}

/// The fields of a transaction in JSON form, after the checks that only the JSON form needs.
fn data_from_json(
    json: &Value,
    params: &Params,
    mode: Mode,
) -> Result<TransactionData, BuildError> {
    let object = json
        .as_object()
        .ok_or(BuildError::Json(JsonError::NotAnObject))?;

    // The reference converts `amount` (when it is truthy) and `fee` to big integers before any
    // rule, and refuses values that do not convert.
    let amount = match object.get("amount") {
        Some(value) if truthy(value) => Some(
            bigint(value).ok_or_else(|| SchemaError::invalid("amount", "must be an integer"))?,
        ),
        _ => None,
    };
    let fee = object
        .get("fee")
        .and_then(bigint)
        .ok_or_else(|| SchemaError::invalid("fee", "must be an integer"))?;

    let type_id =
        integer(object.get("type"), "type")?.ok_or_else(|| SchemaError::missing("type"))?;
    let type_group =
        integer(object.get("typeGroup"), "typeGroup")?.unwrap_or(i128::from(TYPE_GROUP_CORE));
    let kind = TransactionType::from_wire(
        u32::try_from(type_group)
            .map_err(|_| SchemaError::invalid("typeGroup", "must be a 32-bit integer from 0"))?,
        u16::try_from(type_id)
            .map_err(|_| SchemaError::invalid("type", "must be a 16-bit integer from 0"))?,
    )?;

    if mode.is_strict() {
        match object.get("id") {
            None => return Err(SchemaError::missing("id").into()),
            Some(Value::Null) => {}
            Some(Value::String(id)) if id.len() == 64 && is_hex(id) => {}
            Some(_) => {
                return Err(SchemaError::invalid("id", "must be 64 hex digits or null").into());
            }
        }
    }
    match object.get("headerType") {
        None => {}
        Some(value) if value.as_f64() == Some(0.0) => {}
        Some(value) if value.as_f64() == Some(1.0) => {
            return Err(DecodeError::UnsupportedHeaderType(1).into());
        }
        Some(_) => return Err(SchemaError::invalid("headerType", "must be 0 or 1").into()),
    }
    let version = match object.get("version") {
        None => VERSION,
        Some(value) if value.as_f64() == Some(3.0) => VERSION,
        Some(value) if value.as_f64() == Some(2.0) => 2,
        Some(_) => return Err(SchemaError::invalid("version", "must be 2 or 3").into()),
    };
    let network = match object.get("network") {
        None => params.network(),
        Some(value) if value.as_f64() == Some(f64::from(params.network())) => params.network(),
        Some(_) => {
            return Err(SchemaError::invalid(
                "network",
                "must be the network byte of the milestone",
            )
            .into());
        }
    };
    let nonce = object
        .get("nonce")
        .ok_or_else(|| SchemaError::missing("nonce"))
        .and_then(|value| u64_field(bignumber(value), "nonce", 0))?;
    let fee = u64_field(Some(fee), "fee", 0)?;
    if let Some(sender) = object.get("senderId") {
        address_text(sender, "senderId")?;
    }
    let sender_public_key = PublicKey::from_hex(
        &hex_text(object.get("senderPublicKey"), "senderPublicKey", 66)?
            .ok_or_else(|| SchemaError::missing("senderPublicKey"))?,
    )?;
    // A truthy `vendorField` is the memo's old name, and takes its place.
    let memo_value = match object.get("vendorField") {
        Some(value) if truthy(value) => Some(value),
        _ => object.get("memo"),
    };
    let memo = match memo_value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.is_empty() => None,
        Some(Value::String(text)) => Some(
            Memo::new(text)
                .map_err(|_| SchemaError::invalid("memo", "must be at most 255 bytes"))?,
        ),
        Some(_) => return Err(SchemaError::invalid("memo", "must be a string or null").into()),
    };
    if object.contains_key("signatures") {
        return Err(DecodeError::MultiSignature.into());
    }
    let signature = signature_field(object, "signature")?;
    let second_signature = match signature_field(object, "secondSignature")? {
        Some(signature) => Some(signature),
        None => signature_field(object, "signSignature")?,
    };
    if kind == TransactionType::SecondSignature && object.contains_key("secondSignature") {
        return Err(SchemaError::invalid(
            "secondSignature",
            "a second-signature registration has no second signature",
        )
        .into());
    }
    let asset = asset_from_json(
        kind,
        object,
        amount,
        type_group == i128::from(TYPE_GROUP_CORE) && object.contains_key("typeGroup"),
    )?;

    Ok(TransactionData {
        version,
        network,
        kind,
        nonce,
        sender_public_key,
        fee,
        memo,
        asset,
        signature,
        second_signature,
    })
}

/// The asset of `kind` in the JSON `object`.
///
/// `explicit_core` says whether the JSON states type group 1: only then does the reference read a
/// transfer's `asset.payments`, the old name of `asset.transfers`.
fn asset_from_json(
    kind: TransactionType,
    object: &Map<String, Value>,
    amount: Option<i128>,
    explicit_core: bool,
) -> Result<Asset, BuildError> {
    let asset = object.get("asset");
    let asset_object = || -> Result<&Map<String, Value>, BuildError> {
        match asset {
            Some(Value::Object(asset)) => Ok(asset),
            Some(_) => Err(SchemaError::invalid("asset", "must be an object").into()),
            None => Err(SchemaError::missing("asset").into()),
        }
    };
    match kind {
        TransactionType::Transfer => {
            let asset = asset_object()?;
            let renamed = match (asset.get("transfers"), asset.get("payments")) {
                (None, Some(payments)) if explicit_core && truthy(payments) => Some(payments),
                (transfers, _) => transfers,
            };
            let items = match renamed {
                Some(Value::Array(items)) => items,
                Some(_) => {
                    return Err(SchemaError::invalid("asset.transfers", "must be an array").into());
                }
                None => return Err(SchemaError::missing("asset.transfers").into()),
            };
            let mut transfers = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let field = |name: &str| format!("asset.transfers[{index}].{name}");
                let item = item.as_object().ok_or_else(|| {
                    SchemaError::invalid(format!("asset.transfers[{index}]"), "must be an object")
                })?;
                let amount = item
                    .get("amount")
                    .ok_or_else(|| SchemaError::missing(field("amount")))
                    .and_then(|value| u64_field(bignumber(value), "asset.transfers.amount", 1))?;
                let recipient = item
                    .get("recipientId")
                    .ok_or_else(|| SchemaError::missing(field("recipientId")))
                    .and_then(|value| address_text(value, "asset.transfers.recipientId"))?;
                transfers.push(TransferItem {
                    amount,
                    recipient: Address::from_base58(recipient)?,
                });
            }
            Ok(Asset::Transfer { transfers })
        }
        TransactionType::SecondSignature => {
            let signature = asset_object()?
                .get("signature")
                .and_then(Value::as_object)
                .ok_or_else(|| SchemaError::missing("asset.signature"))?;
            let key = hex_text(signature.get("publicKey"), "asset.signature.publicKey", 66)?
                .ok_or_else(|| SchemaError::missing("asset.signature.publicKey"))?;
            Ok(Asset::SecondSignature {
                public_key: PublicKeyBytes::from_hex(&key)?,
            })
        }
        TransactionType::DelegateRegistration => {
            let delegate = asset_object()?
                .get("delegate")
                .and_then(Value::as_object)
                .ok_or_else(|| SchemaError::missing("asset.delegate"))?;
            let username = match delegate.get("username") {
                Some(Value::String(username)) => username.clone(),
                Some(_) => {
                    return Err(SchemaError::invalid(
                        "asset.delegate.username",
                        "must be a string",
                    )
                    .into());
                }
                None => return Err(SchemaError::missing("asset.delegate.username").into()),
            };
            Ok(Asset::DelegateRegistration { username })
        }
        TransactionType::DelegateResignation => {
            let resignation_type = match asset {
                None => None,
                Some(_) => {
                    let value = asset_object()?
                        .get("resignationType")
                        .ok_or_else(|| SchemaError::missing("asset.resignationType"))?;
                    let kind = value
                        .as_f64()
                        .filter(|number| number.fract() == 0.0 && (0.0..=2.0).contains(number))
                        .and_then(|number| ResignationType::from_number(number as u8))
                        .ok_or_else(|| {
                            SchemaError::invalid("asset.resignationType", "must be 0, 1 or 2")
                        })?;
                    Some(kind)
                }
            };
            Ok(Asset::DelegateResignation { resignation_type })
        }
        TransactionType::Burn => {
            let value = object
                .get("amount")
                .ok_or_else(|| SchemaError::missing("amount"))?;
            let amount = amount.or_else(|| bignumber(value));
            Ok(Asset::Burn {
                amount: u64_field(amount, "amount", 0)?,
            })
        }
        TransactionType::Vote => {
            let asset = asset_object()?;
            if asset.contains_key("additionalProperties") {
                return Err(
                    SchemaError::invalid("asset.additionalProperties", "is not allowed").into(),
                );
            }
            let votes = match asset.get("votes") {
                Some(Value::Object(votes)) => votes_from_json(votes)?,
                Some(_) => {
                    return Err(JsonError::Invalid {
                        field: "asset.votes",
                        expected: "an object",
                    }
                    .into());
                }
                None => return Err(SchemaError::missing("asset.votes").into()),
            };
            Ok(Asset::Vote { votes })
        }
    }
}

/// Whether `value` is truthy in JavaScript.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::String(text) => !text.is_empty(),
        Value::Number(number) => number.as_f64().is_some_and(|double| double != 0.0),
        Value::Bool(true) | Value::Array(_) | Value::Object(_) => true,
    }
}

/// The JSON integer `value` of `field`, if present.
fn integer(value: Option<&Value>, field: &'static str) -> Result<Option<i128>, SchemaError> {
    match value {
        None => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|number| number.is_finite() && number.fract() == 0.0)
            // Saturating conversion; the callers check the range.
            .map(|number| Some(number as i128))
            .ok_or_else(|| SchemaError::invalid(field, "must be an integer")),
    }
}

/// `value` as a 64-bit field of the reference: an integer from `minimum` to `2^63 − 1`.
fn u64_field(value: Option<i128>, field: &'static str, minimum: u64) -> Result<u64, SchemaError> {
    let value = value.ok_or_else(|| SchemaError::invalid(field, "must be an integer"))?;
    if value < i128::from(minimum) {
        return Err(SchemaError::invalid(field, "is below the minimum"));
    }
    if value > i128::from(MAX_AMOUNT) {
        return Err(SchemaError::invalid(field, "must be at most 2^63 - 1"));
    }
    u64::try_from(value).map_err(|_| SchemaError::invalid(field, "is out of range"))
}

/// The text of an address field: 34 characters of the Base58 alphabet.
fn address_text<'a>(value: &'a Value, field: &'static str) -> Result<&'a str, SchemaError> {
    match value {
        Value::String(text)
            if text.len() == 34 && text.bytes().all(|byte| BASE58_ALPHABET.contains(&byte)) =>
        {
            Ok(text)
        }
        _ => Err(SchemaError::invalid(field, "must be 34 Base58 characters")),
    }
}

/// The Base58 alphabet, which the reference's address schema requires.
const BASE58_ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Whether `text` is one or more hex digits of either case.
fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The hex text of `field` with exactly `length` digits, if present.
fn hex_text(
    value: Option<&Value>,
    field: &'static str,
    length: usize,
) -> Result<Option<String>, SchemaError> {
    match value {
        None => Ok(None),
        Some(Value::String(text)) if text.len() == length && is_hex(text) => {
            Ok(Some(text.to_ascii_lowercase()))
        }
        Some(_) => Err(SchemaError::invalid(
            field,
            "must be hex of the right length",
        )),
    }
}

/// The signature in `field`, if present: 128 hex digits.
fn signature_field(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<Signature>, BuildError> {
    match hex_text(object.get(field), field, 128)? {
        Some(text) => Ok(Some(Signature::from_hex(&text)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use proptest::collection::vec;
    use proptest::prelude::*;
    use proptest::sample::Index;

    use super::*;
    use crate::crypto::hash::sha256;
    use crate::crypto::sig::Aux;
    use crate::transactions::signer;
    use crate::transactions::testing::{NETWORK, devnet, keys, memo, valid_asset};

    const AUX: Aux = Aux::fixed([7; 32]);

    /// Signed fields with `asset`, and the second key when the transaction is second-signed.
    fn signed(
        asset: Asset,
        nonce: u64,
        fee: u64,
        memo: Option<Memo>,
        second: bool,
        params: &Params,
    ) -> (TransactionData, Option<Vec<u8>>) {
        let sender = keys("round trip sender");
        let mut data = TransactionData {
            version: VERSION,
            network: NETWORK,
            kind: asset.kind(),
            nonce,
            sender_public_key: sender.public_key().clone(),
            fee,
            memo,
            asset,
            signature: None,
            second_signature: None,
        };
        signer::sign(&mut data, &sender, AUX, params).unwrap();
        // A second-signature registration never carries a second signature.
        if second && data.kind != TransactionType::SecondSignature {
            let second = keys("round trip second");
            signer::second_sign(&mut data, &second, AUX, params).unwrap();
            return (data, Some(second.public_key().as_bytes().to_vec()));
        }
        (data, None)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        #[test]
        fn valid_transactions_round_trip(
            asset in valid_asset(),
            nonce in 0..=MAX_AMOUNT,
            fee in 1..=MAX_AMOUNT,
            memo in memo(),
            second in any::<bool>(),
        ) {
            let params = devnet();
            let (data, second_key) = signed(asset, nonce, fee, memo, second, &params);
            let tx = Transaction::from_data(data.clone(), &params).unwrap();
            prop_assert_eq!(tx.data(), &data);
            prop_assert!(tx.is_verified());
            prop_assert_eq!(tx.id().as_bytes(), &sha256(tx.bytes()));
            prop_assert_eq!(&Transaction::decode(tx.bytes(), &params).unwrap(), &tx);
            prop_assert_eq!(&Transaction::decode_trusted(tx.bytes(), &params).unwrap(), &tx);
            prop_assert_eq!(&Transaction::from_json(&tx.to_json(&params), &params).unwrap(), &tx);
            if let Some(key) = second_key {
                prop_assert!(tx.verify_second_signature(&key, &params));
                let sender = data.sender_public_key.as_bytes();
                prop_assert!(!tx.verify_second_signature(sender, &params));
            }
        }

        #[test]
        fn arbitrary_bytes_decode_without_panic(bytes in vec(any::<u8>(), 0..600)) {
            let params = devnet();
            let _ = Transaction::decode_trusted(&bytes, &params);
            if let Ok(tx) = Transaction::decode(&bytes, &params) {
                check_canonical_form(&tx, &params)?;
            }
        }

        #[test]
        fn mutated_transactions_decode_without_panic(
            asset in valid_asset(),
            flips in vec((any::<Index>(), any::<u8>()), 0..4),
            cut in any::<Index>(),
            truncate in any::<bool>(),
            extra in vec(any::<u8>(), 0..3),
        ) {
            let params = devnet();
            let (data, _) = signed(asset, 1, 1, None, false, &params);
            let mut bytes = serialise(&data, SerialiseOptions::FULL, &params).unwrap();
            for (index, value) in flips {
                let at = index.index(bytes.len());
                bytes[at] ^= value;
            }
            if truncate {
                bytes.truncate(cut.index(bytes.len()));
            }
            bytes.extend(extra);
            let _ = Transaction::decode_trusted(&bytes, &params);
            if let Ok(tx) = Transaction::decode(&bytes, &params) {
                check_canonical_form(&tx, &params)?;
            }
        }
    }

    /// The fields of a decoded transaction serialise to a canonical form that decodes to the same
    /// verdict and serialises to itself, and whose id is its SHA-256; the JSON form reads back to
    /// the same canonical bytes. The fields themselves may differ: a version byte 0 reads back as
    /// 3, and a resignation byte `0xff` as no byte.
    fn check_canonical_form(tx: &Transaction, params: &Params) -> Result<(), TestCaseError> {
        let canonical = serialise(tx.data(), SerialiseOptions::FULL, params).unwrap();
        let again = Transaction::decode(&canonical, params).unwrap();
        prop_assert_eq!(again.bytes(), canonical.as_slice());
        prop_assert_eq!(
            &serialise(again.data(), SerialiseOptions::FULL, params).unwrap(),
            &canonical
        );
        prop_assert_eq!(again.id().as_bytes(), &sha256(&canonical));
        prop_assert_eq!(again.is_verified(), tx.is_verified());
        let json = Transaction::from_json(&tx.to_json(params), params).unwrap();
        prop_assert_eq!(json.bytes(), canonical.as_slice());
        Ok(())
    }

    #[test]
    fn version_zero_reads_as_three() {
        let params = devnet();
        let (data, _) = signed(Asset::Burn { amount: 5 }, 1, 1, None, false, &params);
        let tx = Transaction::from_data(data, &params).unwrap();
        let mut bytes = tx.bytes().to_vec();
        bytes[1] = 0;
        let zero = Transaction::decode(&bytes, &params).unwrap();
        assert_eq!(
            zero.id(),
            tx.id(),
            "the id hashes the bytes written again, with version 3"
        );
        assert!(zero.is_verified());
        assert_eq!(zero.bytes(), bytes.as_slice());
        assert_eq!(zero.data().version, 0);
        assert_eq!(zero.to_json(&params)["version"], 3);
        for version in [1, 2, 4, 255] {
            bytes[1] = version;
            assert_eq!(
                Transaction::decode(&bytes, &params),
                Err(DecodeError::UnsupportedVersion(version))
            );
            assert!(Transaction::decode_trusted(&bytes, &params).is_ok());
        }
    }

    #[test]
    fn genesis_mode_waives_the_fee_minimum() {
        let params = devnet();
        let (data, _) = signed(
            Asset::DelegateRegistration {
                username: "genesis".into(),
            },
            1,
            0,
            None,
            false,
            &params,
        );
        assert!(matches!(
            Transaction::from_data(data.clone(), &params),
            Err(BuildError::Schema(SchemaError::Invalid { .. }))
        ));
        let tx = Transaction::from_data_with(data, &params, Mode::Genesis).unwrap();
        assert!(tx.is_verified());
        assert!(Transaction::decode(tx.bytes(), &params).is_err());
        assert!(Transaction::decode_with(tx.bytes(), &params, Mode::Genesis).is_ok());
    }

    #[test]
    fn transfer_to_another_network_has_no_id() {
        let params = devnet();
        let recipient = Address::from_bytes([30; 21]);
        let (data, _) = signed(
            Asset::Transfer {
                transfers: vec![TransferItem {
                    amount: 1,
                    recipient,
                }],
            },
            1,
            1,
            None,
            false,
            &params,
        );
        let bytes = serialise(&data, SerialiseOptions::FULL, &params).unwrap();
        assert!(matches!(
            Transaction::decode(&bytes, &params),
            Err(DecodeError::Id(crate::errors::EncodeError::AddressNetwork(
                _
            )))
        ));
    }

    #[test]
    fn json_forms_of_the_reference() {
        let params = devnet();
        let (data, _) = signed(
            Asset::Burn { amount: 2_000_000 },
            3,
            1,
            None,
            false,
            &params,
        );
        let tx = Transaction::from_data(data, &params).unwrap();
        let json = tx.to_json(&params);
        // Numbers may be JSON integers or text with surrounding space, as BigInt reads them.
        let mut numbers = json.clone();
        numbers["amount"] = serde_json::json!(2_000_000);
        numbers["nonce"] = serde_json::json!(" 3 ");
        numbers["fee"] = serde_json::json!("0x1");
        assert_eq!(Transaction::from_json(&numbers, &params).unwrap(), tx);
        // The strict rules need an id key and a signature.
        let mut no_id = json.clone();
        no_id.as_object_mut().unwrap().remove("id");
        assert!(matches!(
            Transaction::from_json(&no_id, &params),
            Err(BuildError::Schema(_))
        ));
        let mut null_id = json.clone();
        null_id["id"] = Value::Null;
        assert_eq!(Transaction::from_json(&null_id, &params).unwrap(), tx);
        let mut unsigned = json.clone();
        unsigned.as_object_mut().unwrap().remove("signature");
        assert!(matches!(
            Transaction::from_json(&unsigned, &params),
            Err(BuildError::Schema(_))
        ));
        let mut vendor = json.clone();
        vendor["vendorField"] = serde_json::json!("old name");
        let with_memo = Transaction::from_json(&vendor, &params).unwrap();
        assert_eq!(
            with_memo.data().memo.as_ref().map(Memo::as_str),
            Some("old name")
        );
        let mut other_network = json;
        other_network["network"] = serde_json::json!(30);
        assert!(matches!(
            Transaction::from_json(&other_network, &params),
            Err(BuildError::Schema(_))
        ));
    }
}
