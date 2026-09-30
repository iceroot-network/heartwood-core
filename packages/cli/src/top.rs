//! `top`: the running instances of a token across its networks, from the instance index
//! ([`crate::instance_index`]), with `status`'s columns. Each entry's supervisor pid and liveness
//! are read from its held lock, as `status` reads them; an entry whose supervisor is gone is left
//! out. With none running, the reference's "No processes are running".

use std::path::Path;

use crate::errors::ProcessError;
use crate::seams::StatusRow;

/// The reference's answer when no instance of the token runs.
pub const NONE_RUNNING: &str = "No processes are running";

/// The status rows of `token`'s running instances under `data_root`, in name order.
pub fn top(token: &str, data_root: &Path) -> Result<Vec<StatusRow>, ProcessError> {
    let _ = (token, data_root);
    Err(ProcessError::NotBuilt("top"))
}
