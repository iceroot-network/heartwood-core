//! The API's read-only connections.
//!
//! [`Readers`] opens its own connections to the node's file, read only (`query_only`), each with a
//! progress handler that interrupts a statement running for more than its timeout
//! ([`READ_TIMEOUT`], or the database's `apiConnectionTimeout` through [`Readers::with_timeout`]),
//! as the reference's API connection has that statement timeout, and with the SQL functions of the
//! transaction criteria registered ([`crate::filters`]). The writer connection is never used for
//! the API's reads, so a read never waits for a chunk and never sees one half written. [`Reader`]
//! is one connection lent to one request; the history queries are its methods
//! ([`crate::history`]).

use std::ffi::OsString;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::config::DbConfig;
use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::connection::{SCHEMA_VERSION, Settings};
use crate::errors::DatabaseError;
use crate::filters::{Casts, contains_alternative, jsonb_order_key, like_bytes};

use self::watchdog::Watchdog;

/// How long one statement of a reader may run before it is interrupted, unless the readers are
/// opened with another timeout: the reference's default `apiConnectionTimeout`.
pub const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// The virtual machine steps between two checks of a statement's deadline.
const CHECK_EVERY: i32 = 1000;

/// The prepared statements each connection keeps.
const STATEMENTS: usize = 32;

/// The size of the log above which [`Readers::relieve_log`] resets it: the writer truncates it to
/// 16 MiB whenever it can reset it on its own.
pub const LOG_LIMIT: u64 = 64 * 1024 * 1024;

/// The read-only connections of the API.
#[derive(Debug)]
pub struct Readers {
    /// The first connection; there is always one.
    first: Mutex<ReaderConnection>,
    /// The other connections.
    rest: Vec<Mutex<ReaderConnection>>,
    /// The next connection to lend.
    next: AtomicUsize,
    /// The connections lent, and whether a reset of the log holds new reads back.
    lending: Mutex<Lending>,
    /// Told when a connection comes back and when a reset of the log ends.
    changed: Condvar,
    /// The database's log (`-wal`).
    log: OsString,
    /// The statement timeout of the connections.
    timeout: Duration,
}

/// The connections lent, and whether a reset of the log holds new reads back.
#[derive(Debug, Default)]
struct Lending {
    lent: usize,
    draining: bool,
}

/// One read-only connection and the deadline of its running statement.
#[derive(Debug)]
pub(crate) struct ReaderConnection {
    connection: Connection,
    watchdog: Arc<Watchdog>,
    timeout: Duration,
}

/// One read-only connection, lent to one request until it is dropped.
#[derive(Debug)]
pub struct Reader<'r> {
    connection: MutexGuard<'r, ReaderConnection>,
    readers: &'r Readers,
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        let mut lending = self.readers.lending();
        lending.lent = lending.lent.saturating_sub(1);
        drop(lending);
        self.readers.changed.notify_all();
    }
}

impl Readers {
    /// Open `count` read-only connections (at least one) to the database of `settings`, which the
    /// node's writer has opened, created or upgraded before.
    pub fn open(settings: &Settings, count: usize) -> Result<Readers, DatabaseError> {
        Readers::with_timeout(settings, count, READ_TIMEOUT)
    }

    /// Open `count` read-only connections, as [`Readers::open`] does, whose statements are
    /// interrupted after `timeout` instead of [`READ_TIMEOUT`].
    pub fn with_timeout(
        settings: &Settings,
        count: usize,
        timeout: Duration,
    ) -> Result<Readers, DatabaseError> {
        let open = || ReaderConnection::open(settings, timeout).map(Mutex::new);
        let first = open()?;
        let rest = (1..count).map(|_| open()).collect::<Result<Vec<_>, _>>()?;
        let mut log = settings.path.clone().into_os_string();
        log.push("-wal");
        Ok(Readers {
            first,
            rest,
            next: AtomicUsize::new(0),
            lending: Mutex::default(),
            changed: Condvar::new(),
            log,
            timeout,
        })
    }

    fn lending(&self) -> MutexGuard<'_, Lending> {
        self.lending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reset the database's log when it has grown past [`LOG_LIMIT`]; a reader of the API calls
    /// it before it asks for a connection. SQLite's automatic checkpoint resets the log only when
    /// no read holds a snapshot older than the last commit, so reads that never pause (a client
    /// that keeps slow listings running) kept the log growing with every commit. New reads wait
    /// while the connections lent end (each read within two statement timeouts), then a
    /// `TRUNCATE` checkpoint runs on one of these connections. The reference's PostgreSQL has no
    /// such log.
    pub fn relieve_log(&self) {
        self.relieve_log_above(LOG_LIMIT);
    }

    fn relieve_log_above(&self, limit: u64) {
        if std::fs::metadata(&self.log).map_or(0, |meta| meta.len()) <= limit {
            return;
        }
        let mut lending = self.lending();
        if std::mem::replace(&mut lending.draining, true) {
            return;
        }
        let wait = self.timeout.saturating_mul(2);
        let drained = self
            .changed
            .wait_timeout_while(lending, wait, |l| l.lent > 0);
        let (lending, _) = drained.unwrap_or_else(PoisonError::into_inner);
        let idle = lending.lent == 0;
        drop(lending);
        if idle {
            let connection = self.first.lock().unwrap_or_else(PoisonError::into_inner);
            let checkpoint = "PRAGMA wal_checkpoint(TRUNCATE)";
            let done = connection.connection.query_row(checkpoint, [], |_| Ok(()));
            if let Err(error) = done {
                heartwood_kernel::warning!("The store's log could not be reset: {error}");
            }
        }
        self.lending().draining = false;
        self.changed.notify_all();
    }

    /// Lend a connection: a free one, from the next in turn, or else the next in turn once its
    /// request is done. While [`Readers::relieve_log`] resets the log, it waits first (at most
    /// four statement timeouts).
    pub fn get(&self) -> Reader<'_> {
        let lending = self.lending();
        let wait = self.timeout.saturating_mul(4);
        let waited = self
            .changed
            .wait_timeout_while(lending, wait, |l| l.draining);
        let (mut lending, _) = waited.unwrap_or_else(PoisonError::into_inner);
        lending.lent = lending.lent.saturating_add(1);
        drop(lending);
        let count = self.rest.len() + 1;
        let start = self.next.fetch_add(1, Ordering::Relaxed) % count;
        let nth = |index: usize| {
            index
                .checked_sub(1)
                .and_then(|index| self.rest.get(index))
                .unwrap_or(&self.first)
        };
        let free = (0..count).find_map(|step| nth((start + step) % count).try_lock().ok());
        let connection = free.unwrap_or_else(|| {
            // A connection stays usable after a panic of its borrower: every read ends its
            // transaction when it is dropped.
            nth(start).lock().unwrap_or_else(PoisonError::into_inner)
        });
        Reader {
            connection,
            readers: self,
        }
    }
}

impl ReaderConnection {
    /// Open one reader of `settings` whose statements are interrupted after `timeout`.
    fn open(settings: &Settings, timeout: Duration) -> Result<ReaderConnection, DatabaseError> {
        // Not SQLITE_OPEN_READ_ONLY: a read-only connection cannot recover the shared memory of
        // a WAL file after the writer's crash; `query_only` refuses every write instead.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(&settings.path, flags)?;
        connection.busy_timeout(READ_TIMEOUT)?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
        connection.pragma_update(None, "query_only", true)?;
        connection.pragma_update(None, "mmap_size", 0)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != i64::from(SCHEMA_VERSION) {
            return Err(DatabaseError::SchemaVersion {
                found: version.to_string(),
                expected: SCHEMA_VERSION,
            });
        }
        connection.set_prepared_statement_cache_capacity(STATEMENTS);
        register_functions(&connection)?;
        let watchdog = Arc::new(Watchdog::new());
        let expired = Arc::clone(&watchdog);
        connection.progress_handler(CHECK_EVERY, Some(move || expired.expired()))?;
        Ok(ReaderConnection {
            connection,
            watchdog,
            timeout,
        })
    }
}

impl Reader<'_> {
    /// The connection, to begin a read transaction on it.
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection.connection
    }

    /// Run `read`, one statement or a few, and interrupt it after the connection's timeout.
    pub(crate) fn timed<T>(
        &self,
        read: impl FnOnce(&Connection) -> Result<T, DatabaseError>,
    ) -> Result<T, DatabaseError> {
        let _armed = self.connection.watchdog.arm(self.connection.timeout);
        read(&self.connection.connection)
    }
}

/// Register the criteria's SQL functions on `connection`: `heartwood_like(pattern, value)`,
/// `heartwood_contains(asset, criterion)` (the asset contains one alternative of the `asset`
/// criteria, [`contains_alternative`]; the criteria and its casts are read once per statement)
/// and `heartwood_jsonb_order(asset)`. Each is `NULL` when its stored argument is.
fn register_functions(connection: &Connection) -> Result<(), DatabaseError> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_DIRECTONLY;
    connection.create_scalar_function("heartwood_like", 2, flags, |ctx| {
        let (Some(pattern), Some(value)) = (bytes(ctx, 0)?, bytes(ctx, 1)?) else {
            return Ok(None);
        };
        like_bytes(pattern, value)
            .map(Some)
            .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))
    })?;
    connection.create_scalar_function("heartwood_contains", 2, flags, |ctx| {
        let criterion = ctx.get_or_create_aux(1, |value| -> Result<_, BoxedError> {
            let criterion: Value = serde_json::from_slice(value.as_bytes()?)?;
            let casts = Casts::of(&criterion);
            Ok((criterion, casts))
        })?;
        let Some(asset) = json(ctx, 0)? else {
            return Ok(None);
        };
        let (criterion, casts) = &*criterion;
        Ok(Some(contains_alternative(&asset, criterion, casts)))
    })?;
    connection.create_scalar_function("heartwood_jsonb_order", 1, flags, |ctx| {
        Ok(json(ctx, 0)?.map(|asset| jsonb_order_key(&asset)))
    })?;
    Ok(())
}

/// An error of an SQL function's argument.
type BoxedError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The bytes of argument `index`, text or blob; `None` for `NULL`.
fn bytes<'a>(ctx: &'a Context<'_>, index: usize) -> rusqlite::Result<Option<&'a [u8]>> {
    match ctx.get_raw(index) {
        ValueRef::Null => Ok(None),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Ok(Some(bytes)),
        ValueRef::Integer(_) | ValueRef::Real(_) => Err(
            rusqlite::Error::InvalidFunctionParameterType(index, ctx.get_raw(index).data_type()),
        ),
    }
}

/// Argument `index` as JSON; `None` for `NULL`.
fn json(ctx: &Context<'_>, index: usize) -> rusqlite::Result<Option<Value>> {
    let Some(bytes) = bytes(ctx, index)? else {
        return Ok(None);
    };
    serde_json::from_slice(bytes)
        .map(Some)
        .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))
}

/// The deadline of a reader's running statement.
mod watchdog {
    #![expect(
        clippy::disallowed_types,
        reason = "a read's deadline is the store's one use of time: it bounds how long a \
                  statement of the API runs and decides no stored value"
    )]

    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    /// When the running statement of a connection is interrupted.
    #[derive(Debug)]
    pub(crate) struct Watchdog {
        /// The start of the watchdog's clock.
        base: Instant,
        /// The deadline, in nanoseconds after `base`; `u64::MAX` while no statement runs.
        deadline: AtomicU64,
    }

    /// A running statement's deadline, cleared when it is dropped.
    #[derive(Debug)]
    pub(crate) struct Armed<'w>(&'w Watchdog);

    impl Watchdog {
        /// A watchdog with no deadline.
        pub(crate) fn new() -> Watchdog {
            Watchdog {
                base: Instant::now(),
                deadline: AtomicU64::new(u64::MAX),
            }
        }

        /// Interrupt what runs from now on after `timeout`.
        pub(crate) fn arm(&self, timeout: Duration) -> Armed<'_> {
            let deadline = self.now().saturating_add(nanos(timeout));
            self.deadline.store(deadline, Ordering::Relaxed);
            Armed(self)
        }

        /// Whether the running statement is past its deadline.
        pub(crate) fn expired(&self) -> bool {
            self.now() > self.deadline.load(Ordering::Relaxed)
        }

        /// The nanoseconds since `base`.
        fn now(&self) -> u64 {
            nanos(self.base.elapsed())
        }
    }

    impl Drop for Armed<'_> {
        fn drop(&mut self) {
            self.0.deadline.store(u64::MAX, Ordering::Relaxed);
        }
    }

    /// `duration` in nanoseconds, saturating.
    fn nanos(duration: Duration) -> u64 {
        u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Database;

    fn scratch(name: &str) -> (Database, Settings, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "heartwood-database-reader-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings::in_data_dir(&dir);
        (Database::open(&settings).unwrap(), settings, dir)
    }

    #[test]
    #[expect(
        clippy::disallowed_types,
        reason = "the test measures the read's timeout"
    )]
    fn a_statement_is_interrupted_after_the_read_timeout() {
        let (_database, settings, dir) = scratch("timeout");
        let readers = Readers::open(&settings, 1).unwrap();
        let reader = readers.get();
        let started = std::time::Instant::now();
        let result = reader.timed(|connection| {
            Ok(connection.query_row(
                "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) \
                 SELECT count(*) FROM n",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        });
        let elapsed = started.elapsed();
        let error = result.unwrap_err();
        assert!(error.to_string().contains("interrupted"), "{error}");
        assert!(elapsed >= READ_TIMEOUT, "{elapsed:?}");
        assert!(elapsed < READ_TIMEOUT * 3, "{elapsed:?}");
        // The next statement has its own deadline.
        let one = reader.timed(|connection| {
            Ok(connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?)
        });
        assert_eq!(one, Ok(1));
        drop(reader);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[expect(
        clippy::disallowed_types,
        reason = "the test measures the read's timeout"
    )]
    fn a_configured_timeout_replaces_the_default() {
        let (_database, settings, dir) = scratch("configured-timeout");
        let timeout = Duration::from_millis(300);
        let readers = Readers::with_timeout(&settings, 2, timeout).unwrap();
        let started = std::time::Instant::now();
        let result = readers.get().timed(|connection| {
            Ok(connection.query_row(
                "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) \
                 SELECT count(*) FROM n",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        });
        let elapsed = started.elapsed();
        assert!(result.unwrap_err().to_string().contains("interrupted"));
        assert!(elapsed >= timeout && elapsed < READ_TIMEOUT, "{elapsed:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn readers_refuse_writes() {
        let (_database, settings, dir) = scratch("query-only");
        let readers = Readers::open(&settings, 2).unwrap();
        let reader = readers.get();
        let error = reader
            .timed(|connection| Ok(connection.execute("DELETE FROM blocks", [])?))
            .unwrap_err();
        assert!(error.to_string().contains("readonly"), "{error}");
        drop(reader);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The log's size, 0 when there is none.
    fn log_size(settings: &Settings) -> u64 {
        let mut wal = settings.path.clone().into_os_string();
        wal.push("-wal");
        std::fs::metadata(wal).map_or(0, |meta| meta.len())
    }

    #[test]
    fn a_log_that_reads_kept_from_resetting_is_reset_between_them() {
        let (mut database, settings, dir) = scratch("log-reset");
        let readers = Readers::open(&settings, 2).unwrap();
        let write = |database: &mut Database, n: usize| {
            let chunk = database.begin().unwrap();
            let rows =
                format!("INSERT INTO meta (key, value) VALUES ('k{n}', hex(randomblob(4096)))");
            chunk.transaction.execute_batch(&rows).unwrap();
            chunk.commit().unwrap();
        };
        // A read holds a snapshot across every commit: the automatic checkpoint cannot reset
        // the log, which grows with each commit.
        let held = readers.get();
        held.connection()
            .execute_batch("BEGIN; SELECT count(*) FROM blocks;")
            .unwrap();
        for n in 0..300 {
            write(&mut database, n);
        }
        let grown = log_size(&settings);
        assert!(grown > 2 * 1024 * 1024, "{grown}");
        // Past the limit, new reads wait while the lent ones end, then the log is reset.
        std::thread::scope(|scope| {
            let relieve = scope.spawn(|| readers.relieve_log_above(1024 * 1024));
            std::thread::sleep(Duration::from_millis(200));
            held.connection().execute_batch("COMMIT").unwrap();
            drop(held);
            relieve.join().unwrap();
        });
        assert_eq!(log_size(&settings), 0);
        // Reads go on.
        let one = readers.get().timed(|connection| {
            Ok(connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?)
        });
        assert_eq!(one, Ok(1));
        // Under the limit nothing waits and nothing is reset.
        write(&mut database, 300);
        let small = log_size(&settings);
        readers.relieve_log();
        assert_eq!(log_size(&settings), small);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn busy_connections_are_passed_over() {
        let (_database, settings, dir) = scratch("lend");
        let readers = Readers::open(&settings, 2).unwrap();
        let first = readers.get();
        let second = readers.get();
        // Both are lent at once, so they are different connections.
        let a: *const Connection = &first.connection.connection;
        let b: *const Connection = &second.connection.connection;
        assert_ne!(a, b);
        drop((first, second));
        let _ = std::fs::remove_dir_all(dir);
    }
}
