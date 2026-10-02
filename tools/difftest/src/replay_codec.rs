//! Replay the codec fuzz targets' corpora through the oracle and Heartwood (`difftest oracle
//! replay-codec`), beside [`crate::replay`]'s replay of the binary decoders' corpora.
//!
//! - `address_decode`: each input, as text, validated as an address of the devnet's network by
//!   Heartwood and by the reference's address validation: equal verdicts and network errors, and
//!   an accepted address re-encoded to the same text.
//! - `transaction_json`: each input, as JSON text, read as a transaction's data, strictly, by
//!   Heartwood and by the reference's transaction factory (with its `vendorField` and
//!   `asset.payments` aliases): equal acceptance or refusal, ids and bytes.
//! - `api_head`: a request's head read as Node's parser reads it; a differential replay only where
//!   the oracle has an operation that parses a head so, else the target is crash-only.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use crate::cli::CodecReplayArgs;
use crate::errors::{Error, Result};
use crate::oracle::OracleServer;
use crate::replay::ReplayReport;
use crate::subject::Registry;

/// A codec fuzz target whose corpus can be replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecTarget {
    /// `address_decode`: text read as an address.
    AddressDecode,
    /// `transaction_json`: JSON text read as a transaction's data.
    TransactionJson,
    /// `api_head`: a request's head read as Node's parser reads it.
    ApiHead,
}

impl CodecTarget {
    /// Every codec target.
    pub const ALL: [CodecTarget; 3] = [
        CodecTarget::AddressDecode,
        CodecTarget::TransactionJson,
        CodecTarget::ApiHead,
    ];

    /// The fuzz target's name.
    pub const fn name(self) -> &'static str {
        match self {
            CodecTarget::AddressDecode => "address_decode",
            CodecTarget::TransactionJson => "transaction_json",
            CodecTarget::ApiHead => "api_head",
        }
    }
}

impl fmt::Display for CodecTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for CodecTarget {
    type Err = Error;

    fn from_str(text: &str) -> Result<CodecTarget> {
        CodecTarget::ALL
            .into_iter()
            .find(|target| target.name() == text)
            .ok_or_else(|| {
                Error::Usage(format!(
                    "{text} is not a codec target (address_decode, transaction_json or api_head)"
                ))
            })
    }
}

/// Replay every file of `corpus`, in name order, as inputs of `target`.
pub fn run(
    server: &mut OracleServer,
    registry: &Registry,
    target: CodecTarget,
    corpus: &Path,
) -> Result<ReplayReport> {
    let _ = (server, registry, corpus);
    Err(Error::NotYet(format!(
        "the replay of {target} is not built yet"
    )))
}

/// `difftest oracle replay-codec`: the corpus of `args`.
pub fn run_command(args: &CodecReplayArgs) -> Result<ReplayReport> {
    Err(Error::NotYet(format!(
        "the replay of {} is not built yet",
        args.target
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_codec_target_reads_its_own_name_only() {
        for target in CodecTarget::ALL {
            assert_eq!(target.name().parse::<CodecTarget>().unwrap(), target);
        }
        assert!("api_query".parse::<CodecTarget>().is_err());
    }
}
