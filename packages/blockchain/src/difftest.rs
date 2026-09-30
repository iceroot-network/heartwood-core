//! The job stall of a harness build (feature `difftest`), which no release build has: the next job
//! of one block sleeps, ahead of its work and so of its commit, for the number of milliseconds
//! that the file `stall-next-job` in the harness's directory holds, and the file is removed, so a
//! live run can hold one job past AwaitBlock's bound while reference clients wait on the relay.
//! The job's origin is not read: a block received in `newBlock` comes in such a job, and so does a
//! download that fetched one block, which may take the control first on a node that syncs.

use std::path::PathBuf;
use std::time::Duration;

use heartwood_kernel::debug;

/// The name of the control file in the harness's directory.
pub const STALL_FILE: &str = "stall-next-job";

/// The stall control of a relay: its harness's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StallControl {
    dir: PathBuf,
}

impl StallControl {
    /// The control of the harness's directory `dir`.
    pub fn new(dir: PathBuf) -> StallControl {
        StallControl { dir }
    }

    /// The stall the control file asks for, which removes the file; `None` when there is none.
    ///
    /// The file holds a number of milliseconds. A file that does not hold one is left alone, for
    /// a writer that is not done with it.
    pub fn take(&self) -> Option<Duration> {
        let path = self.dir.join(STALL_FILE);
        let text = std::fs::read_to_string(&path).ok()?;
        let millis: u64 = text.trim().parse().ok()?;
        std::fs::remove_file(&path).ok()?;
        debug!("difftest: holding the job for {millis} ms");
        Some(Duration::from_millis(millis))
    }
}

/// The stall of a job of `blocks` blocks, ahead of its work: a job of one block, which is how a
/// block received in `newBlock` comes (and a download of one block), sleeps for the time the
/// control file asks for. It comes before the block moves the tip, so that a request waiting on
/// the job still sees the tip the job found; the commit follows the wait either way.
pub(crate) fn hold(stall: Option<&StallControl>, blocks: usize) {
    if let Some(time) = stall.filter(|_| blocks == 1).and_then(StallControl::take) {
        std::thread::sleep(time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system's, removed with the guard.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("heartwood-stall-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a directory");
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_file_is_taken_once_and_removed() {
        let scratch = Scratch::new("taken");
        let control = StallControl::new(scratch.0.clone());
        assert_eq!(control.take(), None);
        std::fs::write(scratch.0.join(STALL_FILE), "12000\n").expect("written");
        assert_eq!(control.take(), Some(Duration::from_millis(12_000)));
        assert!(!scratch.0.join(STALL_FILE).exists());
        assert_eq!(control.take(), None);
    }

    #[test]
    fn only_a_job_of_one_block_is_held() {
        let scratch = Scratch::new("held");
        let control = StallControl::new(scratch.0.clone());
        std::fs::write(scratch.0.join(STALL_FILE), "5").expect("written");
        hold(Some(&control), 2);
        hold(None, 1);
        assert!(scratch.0.join(STALL_FILE).exists());
        hold(Some(&control), 1);
        assert!(!scratch.0.join(STALL_FILE).exists());
    }

    #[test]
    fn a_file_without_a_number_stays() {
        let scratch = Scratch::new("empty");
        let control = StallControl::new(scratch.0.clone());
        std::fs::write(scratch.0.join(STALL_FILE), "").expect("written");
        assert_eq!(control.take(), None);
        assert!(scratch.0.join(STALL_FILE).exists());
    }
}
