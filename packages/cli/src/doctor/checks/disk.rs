//! Disks: space and growth, the other directories' filesystems, file entries, the store's device,
//! and slow signing-record writes.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `disk.space`.
pub fn space(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `disk.other`.
pub fn other(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `disk.inodes`.
pub fn inodes(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `disk.type`.
pub fn disk_type(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `disk.slow-writes`.
pub fn slow_writes(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
