//! The API's read-only connections.
//!
//! [`Readers`] opens its own connections to the node's file, read only (`query_only`), each with a
//! progress handler that interrupts a statement running for more than [`READ_TIMEOUT`], and with
//! the two SQL functions of the transaction criteria registered ([`crate::filters`]). The writer
//! connection is never used for the API's reads, so a read never waits for a chunk and never sees
//! one half written. [`Reader`] is one connection lent to one request; the history queries are
//! its methods ([`crate::history`]).

use std::sync::atomic::AtomicUsize;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;

use crate::connection::Settings;
use crate::errors::DatabaseError;

/// How long one statement of a reader may run before it is interrupted.
pub const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// The read-only connections of the API.
#[derive(Debug)]
pub struct Readers {
    /// The connections; a request takes the next one in turn and waits while it is lent.
    connections: Vec<Mutex<Connection>>,
    /// The next connection to lend.
    next: AtomicUsize,
}

/// One read-only connection, lent to one request until it is dropped.
#[derive(Debug)]
pub struct Reader<'r> {
    /// The connection.
    pub(crate) connection: MutexGuard<'r, Connection>,
}

impl Readers {
    /// Open `count` read-only connections (at least one) to the database of `settings`, which the
    /// node's writer has opened, created or upgraded before.
    pub fn open(settings: &Settings, count: usize) -> Result<Readers, DatabaseError> {
        let _ = (settings, count);
        todo!()
    }

    /// Lend the next connection, waiting while another request holds it.
    pub fn get(&self) -> Reader<'_> {
        let _ = (&self.connections, &self.next);
        todo!()
    }
}
