//! `wire.checkReply`: a reply of one of the six routes other than getStatus, decoded and checked
//! with its reply schema. The input is `{route, hex}`, or `{hex}` whose first byte selects the
//! route (`byte % 6` over `heartwood_p2p_wire::replies::ROUTES`) and whose rest is the reply. The
//! output is `{valid: true, value}` with the value after the schema's changes, or
//! `{valid: false, refused: part}`; a reply the codec refuses is the codec's error class.

use crate::subject::{Case, SubjectError, Verdict};
use crate::subjects::{Env, Outcome};

/// `wire.checkReply`.
pub(super) fn check_reply(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
