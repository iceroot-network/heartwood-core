//! `nes.parse` and `nes.stringify`: frames from and to the oracle's JSON, `{version, type, id,
//! method, statusCode, path, socket, heartbeat, payload}` with the payload as hex.

use crate::subject::{Case, SubjectError, Verdict};
use crate::subjects::{Env, Outcome};

/// `nes.parse`: the frame in `input.hex`, or the reference's error class.
pub(super) fn parse(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported(
        "the nes frames are not answered yet".into(),
    ))
}

/// `nes.stringify`: the bytes of `input.message`, or the reference's error class.
pub(super) fn stringify(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported(
        "the nes frames are not answered yet".into(),
    ))
}
