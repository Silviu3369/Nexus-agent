//! Host-owned SQLite persistence. One dedicated thread owns the connection
//! and runs every read and write in order, so writes are serialised without
//! locks in the callers (`ARHITECTURA_NEXUS_RO.md` §5, §7).

mod automation;
mod chat;
mod mcp;
mod memory;
mod repo;
mod schema;
mod steps;
mod usage;

pub use automation::{AutomationRow, AutomationRunRow, NewAutomation};
pub use chat::{
    ConnectionRow, Continuation, ConversationRow, DEFAULT_PERSONA_NAME, Deleted, Exchange,
    MessageRow, NewExchange, Pause, PersonaRow, Settlement,
};
pub use mcp::{McpServerRow, McpToolRow, NewMcpTool};
pub use memory::{MemoryFilter, MemoryRow, NewMemory, ProjectRow};
pub use repo::{
    ActionRow, ApprovalRow, ArtifactRow, Decision, EventRow, NewAction, NewOperation, OperationRow,
    RunDetail, RunError, RunOrigin, RunRow, SubmitOutcome, TaskRow,
};
pub use steps::AgentStepRow;
pub use usage::{PriceRow, UsageRow};

use std::path::Path;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

pub use schema::SUPPORTED_VERSION;

/// Bounded so a flood of requests applies backpressure instead of growing
/// memory without limit.
const JOB_QUEUE_CAPACITY: usize = 256;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("refusing a database on a network path (WAL needs local disk): {0}")]
    NetworkPath(String),
    #[error("journal mode is {0:?}, expected \"wal\"")]
    NotWal(String),
    #[error("storage thread is not running")]
    Closed,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("stored data is inconsistent: {0}")]
    Corrupt(String),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    IllegalTransition(#[from] nexus_core::IllegalTransition),
    #[error("{0} changed concurrently")]
    RevisionConflict(&'static str),
    #[error("idempotency key {0:?} was already used with a different request")]
    IdempotencyConflict(String),
    #[error("approval already decided")]
    ApprovalAlreadyDecided,
    #[error("approval expired")]
    ApprovalExpired,
    #[error("approval invalidated: {0}")]
    ApprovalInvalidated(String),
    #[error("cancellation was requested before the effect started")]
    CancellationRequested,
    #[error("a reply is still being written; stop it first")]
    ReplyInProgress,
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

pub struct Store {
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl Store {
    /// Opens (creating if needed) and migrates the database, then hands the
    /// connection to its owning thread. A database that fails to open is left
    /// untouched for diagnosis — never deleted or reinitialised.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let conn = open_connection(path)?;
        let (jobs, mut rx) = mpsc::channel::<Job>(JOB_QUEUE_CAPACITY);
        let worker = std::thread::Builder::new()
            .name("nexus-storage".to_string())
            .spawn(move || {
                let mut conn = conn;
                while let Some(job) = rx.blocking_recv() {
                    job(&mut conn);
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            worker: Some(worker),
        })
    }

    /// Runs `f` on the storage thread and returns its result.
    pub async fn call<T, F>(&self, f: F) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StorageError> + Send + 'static,
    {
        let (reply, response) = oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let _ = reply.send(f(conn));
        });
        self.jobs
            .as_ref()
            .ok_or(StorageError::Closed)?
            .send(job)
            .await
            .map_err(|_| StorageError::Closed)?;
        response.await.map_err(|_| StorageError::Closed)?
    }

    pub async fn schema_version(&self) -> Result<i64, StorageError> {
        self.call(|conn| Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?))
            .await
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // Closing the channel ends the worker loop after queued jobs finish.
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn open_connection(path: &Path) -> Result<Connection, StorageError> {
    let shown = path.display().to_string();
    // `\\server\share\...` is a network share; `\\?\C:\...` is a local path.
    if shown.starts_with(r"\\") && !shown.starts_with(r"\\?\") {
        return Err(StorageError::NetworkPath(shown));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut conn = Connection::open(path)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(StorageError::NotWal(mode));
    }
    // FULL: an intent must be durable before the side effect it guards.
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    // What the user deletes is overwritten with zeros in the file, not just
    // unlinked (`chat::shrink_journal` handles the journal).
    conn.pragma_update(None, "secure_delete", true)?;
    schema::migrate(&mut conn)?;
    Ok(conn)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
