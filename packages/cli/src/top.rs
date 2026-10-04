//! `top`: the running instances of a token across its networks, from the instance index
//! ([`crate::instance_index`]), with `status`'s columns. Each entry's supervisor pid and liveness
//! are read from its held lock, as `status` reads them; an entry whose supervisor is gone is left
//! out. With none running, the command uses the reference's words "No processes are running",
//! with Heartwood's exit 1; the reference logs `[ERROR] <message>` and exits 0.

use std::path::Path;

use crate::errors::ProcessError;
use crate::instance_index;
use crate::process;
use crate::seams::StatusRow;
use crate::supervisor::signals::is_locked;

/// The reference's message text when no instance of the token runs.
pub const NONE_RUNNING: &str = "No processes are running";

/// The status rows of `token`'s running instances under `data_root`, in name order. An instance
/// whose supervisor's lock is not held is left out, whatever its entry or its state file says.
pub fn top(token: &str, data_root: &Path) -> Result<Vec<StatusRow>, ProcessError> {
    let mut rows = Vec::new();
    for instance in instance_index::list(data_root, token)? {
        if is_locked(&instance.supervisor_lock_path())? {
            rows.push(process::status(&instance)?);
        }
    }
    rows.retain(|row| row.status.is_running());
    Ok(rows)
}
