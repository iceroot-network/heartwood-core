//! The log files: names, the midnight rotation, gzip, and the retention of 10 rotated files and
//! 100 MiB.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use flate2::read::GzDecoder;
use heartwood_kernel::ManualClock;
use heartwood_kernel::clock::Clock;
use heartwood_logger::LoggerOptions;
use heartwood_logger::rotation::{RotatingFile, current_name, history_name, prune, rotated_name};

const STEM: &str = "root-testnet-core";
/// 2026-01-01T23:59:59.000Z.
const LAST_SECOND: i64 = 1_767_311_999_000;

/// A fresh, empty directory under the system's temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "heartwood-logger-{}-{name}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn gunzip(path: &Path) -> String {
    let mut text = String::new();
    GzDecoder::new(fs::File::open(path).unwrap())
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn history(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join(history_name(STEM)))
        .unwrap()
        .lines()
        .map(|line| {
            Path::new(line)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn names() {
    assert_eq!(current_name(STEM), "root-testnet-core-current.log");
    assert_eq!(history_name(STEM), "root-testnet-core-current.log.txt");
    assert_eq!(
        rotated_name(STEM, "2026-01-02", 0),
        "root-testnet-core-2026-01-02.log.gz"
    );
    assert_eq!(
        rotated_name(STEM, "2026-01-02", 1),
        "root-testnet-core-2026-01-02.log.gz"
    );
    assert_eq!(
        rotated_name(STEM, "2026-01-02", 2),
        "root-testnet-core-2026-01-02.2.log.gz"
    );
    assert_eq!(
        rotated_name(STEM, "2026-01-02", 10),
        "root-testnet-core-2026-01-02.10.log.gz"
    );
}

#[test]
fn the_defaults_keep_10_files_of_100_mib() {
    let options = LoggerOptions::default();
    assert_eq!((options.max_files, options.max_bytes), (10, 104_857_600));
}

#[test]
fn the_first_write_after_midnight_rotates_into_a_gzip_file() {
    let dir = Scratch::new("midnight");
    let clock = Arc::new(ManualClock::new(LAST_SECOND));
    let mut file = RotatingFile::open(&dir, STEM, 10, 104_857_600, clock.clone()).unwrap();
    file.write_all(b"before midnight\n").unwrap();
    clock.advance(999);
    file.write_all(b"still the first day\n").unwrap();
    assert!(!dir.join(rotated_name(STEM, "2026-01-02", 1)).exists());

    clock.advance(1);
    file.write_all(b"the second day\n").unwrap();
    let rotated = dir.join(rotated_name(STEM, "2026-01-02", 1));
    assert_eq!(gunzip(&rotated), "before midnight\nstill the first day\n");
    assert_eq!(
        fs::read_to_string(dir.join(current_name(STEM))).unwrap(),
        "the second day\n"
    );
    assert_eq!(history(&dir), [rotated_name(STEM, "2026-01-02", 1)]);

    // Another rotation on the same date takes the next index; the next day's midnight rotates
    // again.
    assert_eq!(
        file.rotate().unwrap(),
        dir.join(rotated_name(STEM, "2026-01-02", 2))
    );
    clock.advance(86_400_000);
    file.write_all(b"the third day\n").unwrap();
    assert_eq!(gunzip(&dir.join(rotated_name(STEM, "2026-01-03", 1))), "");
    assert_eq!(history(&dir).len(), 3);
}

#[test]
fn a_failed_rotation_keeps_the_lines_and_waits_for_the_next_midnight() {
    let dir = Scratch::new("failed");
    let clock = Arc::new(ManualClock::new(LAST_SECOND));
    let mut file = RotatingFile::open(&dir, STEM, 10, 104_857_600, clock.clone()).unwrap();
    file.write_all(b"the first day\n").unwrap();
    // Every rotated name of 2026-01-02 is taken, so the rotation at midnight fails.
    for index in 1..=999 {
        fs::write(dir.join(rotated_name(STEM, "2026-01-02", index)), "").unwrap();
    }
    clock.advance(1_000);
    file.write_all(b"after midnight\n").unwrap();
    // A free name does not bring the rotation back before the next midnight.
    fs::remove_file(dir.join(rotated_name(STEM, "2026-01-02", 1))).unwrap();
    file.write_all(b"later\n").unwrap();
    assert!(!dir.join(rotated_name(STEM, "2026-01-02", 1)).exists());
    assert_eq!(
        fs::read_to_string(file.path()).unwrap(),
        "the first day\nafter midnight\nlater\n"
    );
    clock.advance(86_400_000);
    file.write_all(b"the third day\n").unwrap();
    assert_eq!(
        gunzip(&dir.join(rotated_name(STEM, "2026-01-03", 1))),
        "the first day\nafter midnight\nlater\n"
    );
    assert_eq!(fs::read_to_string(file.path()).unwrap(), "the third day\n");
    assert!(format!("{file:?}").starts_with("RotatingFile {"));
}

#[test]
fn a_start_appends_and_does_not_rotate() {
    let dir = Scratch::new("restart");
    fs::write(dir.join(current_name(STEM)), "old\n").unwrap();
    let clock = Arc::new(ManualClock::new(LAST_SECOND + 86_400_000 * 3));
    let mut file = RotatingFile::open(&dir, STEM, 10, 104_857_600, clock).unwrap();
    file.write_all(b"new\n").unwrap();
    assert_eq!(fs::read_to_string(file.path()).unwrap(), "old\nnew\n");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn a_removed_live_file_is_started_again() {
    let dir = Scratch::new("removed");
    let clock = Arc::new(ManualClock::new(LAST_SECOND - 10_000));
    let mut file = RotatingFile::open(&dir, STEM, 10, 104_857_600, clock).unwrap();
    file.write_all(b"one\n").unwrap();
    fs::remove_file(file.path()).unwrap();
    file.write_all(b"two\n").unwrap();
    assert_eq!(fs::read_to_string(file.path()).unwrap(), "two\n");
}

#[test]
fn at_most_10_rotated_files_are_kept() {
    let dir = Scratch::new("count");
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(LAST_SECOND));
    let mut file = RotatingFile::open(&dir, STEM, 10, 104_857_600, clock).unwrap();
    let mut rotated = Vec::new();
    for index in 0..12 {
        file.write_all(format!("line {index}\n").as_bytes())
            .unwrap();
        rotated.push(file.rotate().unwrap());
    }
    // The two oldest go. The first name is free again by the last rotation, which takes it, as
    // the reference takes the first free index.
    let kept = history(&dir);
    let names: Vec<String> = (3..=11)
        .chain([1])
        .map(|index| rotated_name(STEM, "2026-01-01", index))
        .collect();
    assert_eq!(kept, names);
    assert!(!dir.join(rotated_name(STEM, "2026-01-01", 2)).exists());
    assert_eq!(rotated[11], dir.join(rotated_name(STEM, "2026-01-01", 1)));
    assert_eq!(gunzip(&rotated[11]), "line 11\n");
}

#[test]
fn the_oldest_files_go_until_the_rest_fit_the_byte_limit() {
    let dir = Scratch::new("bytes");
    let files: Vec<PathBuf> = (1..=4)
        .map(|index| {
            let path = dir.join(rotated_name(STEM, "2026-01-01", index));
            fs::write(&path, vec![b'x'; 400]).unwrap();
            path
        })
        .collect();
    fs::write(
        dir.join(history_name(STEM)),
        format!(
            "{}\n{}\n{}\n",
            files[0].display(),
            dir.join("gone.log.gz").display(),
            files[1].display()
        ),
    )
    .unwrap();
    prune(&dir, STEM, &files[2], 10, 1_000).unwrap();
    assert!(!files[0].exists());
    assert!(files[1].exists() && files[2].exists());
    assert_eq!(
        history(&dir),
        [
            rotated_name(STEM, "2026-01-01", 2),
            rotated_name(STEM, "2026-01-01", 3)
        ]
    );

    // A limit of 0 is no limit.
    prune(&dir, STEM, &files[3], 0, 0).unwrap();
    assert_eq!(history(&dir).len(), 3);
}
