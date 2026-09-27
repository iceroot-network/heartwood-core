//! The route corpus and its comparison: the queries of every served route (each criterion alone
//! and in pairs, orders, pages, unknown keys, invalid values, the 404 texts, the wallets, the peers'
//! filters, the pool after a submission, and a section of 101 requests for the rate limit), sent to
//! two nodes alternately at a paused height, and their answers judged: equal status codes,
//! `connection: close` and `X-Block-Height` present, bodies equal after the projections (the
//! volatile fields left out, the peer routes compared as sets by address), and each documented
//! difference behaving as documented.

use std::path::PathBuf;

use crate::errors::Result;
use crate::report::Report;
use crate::subject::{Case, SubjectError, Verdict};

/// The committed route corpus, relative to the harness's directory.
pub const CORPUS_DIR: &str = "api-corpus";

/// What `difftest api compare` compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiCompareArgs {
    /// The answers a live run recorded at its pause, in its run directory.
    Recorded {
        /// The run directory.
        run: PathBuf,
    },
    /// Two live nodes, queried now.
    Live {
        /// The node under test's API.
        left: String,
        /// The control node's API.
        right: String,
        /// The route corpus.
        corpus: PathBuf,
        /// Where the answers and the report go.
        out: PathBuf,
    },
}

/// Compare the answers of `args`.
pub fn compare(args: &ApiCompareArgs) -> Result<Report> {
    let _ = args;
    todo!()
}

/// `api.parseQuery`: a query string to the parsed object, with `heartwood_api::schemas`.
pub fn parse_query_case(case: &Case<'_>) -> std::result::Result<Verdict, SubjectError> {
    let _ = case;
    Err(SubjectError::NotImplemented)
}

/// `api.paginate`: a list's path, query and count to its `meta`, with
/// `heartwood_api::pagination`.
pub fn paginate_case(case: &Case<'_>) -> std::result::Result<Verdict, SubjectError> {
    let _ = case;
    Err(SubjectError::NotImplemented)
}
