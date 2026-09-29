//! The reference's marker files in the temp directory: `clear-saved-states.lock` (replay the state
//! from the genesis block) and `clear-pool.lock` (clear the pool's storage).
//!
//! The reference honours a marker whenever it exists. Its temp directory is by default
//! `<tmp>/<user>/<token>-core/<network>`, which any local user can make first, with markers in it
//! that the relay cannot remove: every start then replayed the chain and emptied the pool. A
//! marker is honoured here only when it is a file, and it and its directory belong to the user of
//! the relay's data directory (whose `run` directory the process lock has checked) and are not
//! writable by every user; another is ignored with a warning. The group's write bit is allowed, as
//! a umask of 002 gives it to the user's own group.

use std::path::Path;

use heartwood_cli::recovery::{self, Trust};

/// Whether the marker file `marker` is to be honoured by the relay whose data directory is `data`
/// (see the module documentation; the rule is [`recovery::marker_trust`], which the commands
/// share). A marker that does not exist is not.
pub fn honoured(marker: &Path, data: &Path) -> bool {
    match recovery::marker_trust(marker, data) {
        None => false,
        Some(Trust::Trusted) => true,
        Some(Trust::Untrusted { .. }) => {
            heartwood_kernel::warning!(
                "Ignored {}: it or its directory is another user's, or any user may write it :warning:",
                marker.display()
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("heartwood-markers-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("data/run")).unwrap();
        fs::create_dir_all(root.join("temp")).unwrap();
        fs::set_permissions(root.join("temp"), Permissions::from_mode(0o755)).unwrap();
        root
    }

    #[test]
    fn only_a_marker_of_this_user_that_no_one_else_may_write_is_honoured() {
        let root = scratch("modes");
        let (data, temp) = (root.join("data"), root.join("temp"));
        let marker = temp.join("clear-pool.lock");
        assert!(!honoured(&marker, &data), "no marker");
        fs::write(&marker, "").unwrap();
        fs::set_permissions(&marker, Permissions::from_mode(0o644)).unwrap();
        assert!(honoured(&marker, &data));
        // The group may write them (a umask of 002); every user may not.
        fs::set_permissions(&marker, Permissions::from_mode(0o664)).unwrap();
        fs::set_permissions(&temp, Permissions::from_mode(0o775)).unwrap();
        assert!(honoured(&marker, &data));
        fs::set_permissions(&marker, Permissions::from_mode(0o666)).unwrap();
        assert!(!honoured(&marker, &data));
        fs::set_permissions(&marker, Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&temp, Permissions::from_mode(0o777)).unwrap();
        assert!(!honoured(&marker, &data));
        fs::set_permissions(&temp, Permissions::from_mode(0o755)).unwrap();
        // A link, or a directory, in the marker's place.
        fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), &marker).unwrap();
        fs::write(root.join("elsewhere"), "").unwrap();
        assert!(!honoured(&marker, &data));
        fs::remove_file(&marker).unwrap();
        fs::create_dir(&marker).unwrap();
        assert!(!honoured(&marker, &data));
        // Without the data directory's run directory there is no user to compare with.
        fs::remove_dir(&marker).unwrap();
        fs::write(&marker, "").unwrap();
        assert!(!honoured(&marker, &root.join("no data")));
        fs::remove_dir_all(&root).unwrap();
    }
}
