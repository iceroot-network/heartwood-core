//! `p2p.validateRequest`: a request the client writes, as a reference server reads it (its
//! decoder, its version check and its request schema). The input is `{route, hex}`; the output
//! is `{accepted: true, value}`, or `{status: 400, message}` for a request the server refuses.

use crate::subject::{Case, SubjectError, Verdict};
use crate::subjects::{Env, Outcome};

/// `p2p.validateRequest`.
pub(super) fn validate_request(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
