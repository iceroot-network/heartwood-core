//! The attestation record of one start of a relay: one JSON line per step of a status's
//! attestation ([`AttestationRecord`]: an offer, its signatures' attachment, a getStatus served
//! with them, or one served unsigned after the wait), appended to its `attest-NNNN.jsonl` beside
//! the start's AwaitBlock record and flushed line by line. A release build has no such file.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use heartwood_kernel::timestamp::UtcDateTime;
use serde_json::json;

use crate::node_services::{AttestationRecord, AttestationStep};

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
    /// `step` one of `offered`, `signed` (the signatures attached), `served` (a getStatus served
    /// them) and `unsigned`; `attestations` is the number attached or served (zero at the other two
    /// steps). The line is written whole and flushed; a failure is logged, as the harness finds
    /// the gap.
    pub fn write(&self, record: &AttestationRecord) {
        let t = UtcDateTime::from_unix_ms(record.at_ms).map(|time| time.to_iso_string());
        let (step, attestations) = match record.step {
            AttestationStep::Offered => ("offered", 0),
            AttestationStep::Signed { attestations } => ("signed", attestations),
            AttestationStep::Served { attestations } => ("served", attestations),
            AttestationStep::ServedUnsigned => ("unsigned", 0),
        };
        let line = json!({
            "t": t, "step": step, "attestations": attestations,
            "status": record.status, "slot": record.slot,
        });
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        let written = file.as_mut().map(|file| {
            let bytes = format!("{line}\n");
            file.write_all(bytes.as_bytes()).and_then(|()| file.flush())
        });
        if let Some(Err(error)) = written {
            heartwood_kernel::error!("the attestation record could not be written: {error}");
        }
    }
}
