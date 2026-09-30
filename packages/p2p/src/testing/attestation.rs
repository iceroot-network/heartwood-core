//! The attestation record of one start of a relay: one JSON line per step of a status's
//! attestation ([`AttestationRecord`]: an offer, its signatures' attachment, a getStatus served
//! unsigned after the wait), appended to its `attest-NNNN.jsonl` beside the start's AwaitBlock
//! record and flushed line by line. A release build has no such file.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Mutex;

use crate::node_services::AttestationRecord;

/// The attestation record's file.
#[derive(Debug)]
pub struct AttestationRecordFile {
    file: Mutex<Option<File>>,
}

impl AttestationRecordFile {
    /// The record at `path` (created when missing, else appended to).
    pub fn open(path: &Path) -> Result<AttestationRecordFile, String> {
        let file = OpenOptions::new().create(true).append(true).open(path);
        let file = file.map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(AttestationRecordFile {
            file: Mutex::new(Some(file)),
        })
    }

    /// Record `record` as one line, `{t, step, attestations, status, slot}`, `t` its UTC time and
    /// `step` one of `offered`, `signed` and `unsigned` (not written yet: the line's writer is the
    /// attestation record's to come).
    pub fn write(&self, record: &AttestationRecord) {
        let _ = (record, &self.file);
    }
}
