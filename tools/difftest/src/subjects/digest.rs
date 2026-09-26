//! The canonical state digest of a wallet fixture.

use super::{Env, Outcome};
use crate::subject::{Case, SubjectError, Verdict};

/// `state.digest`: the digest sections and hashes of a fixture.
pub(super) fn state_digest(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}
