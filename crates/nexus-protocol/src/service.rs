//! The RPC contract itself, generated from a single Rust trait
//! (ADR-IPC-001: tarpc instead of a separate schema language). `nexus-host`
//! implements this; `nexus-client` calls the generated client stub.

use crate::error::NexusError;
use crate::handshake::{GetHealthResponse, HandshakeRequest, HandshakeResponse};
use crate::ids::{ArtifactId, RunId};
use crate::runs::{
    BackupSummary, DecideApprovalRequest, EventsPage, RunView, Snapshot, SubmitNoteRequest,
    SubmitNoteResponse,
};

#[tarpc::service]
pub trait NexusService {
    /// First call on a new connection. Negotiates protocol version and
    /// identity; carries no secrets and no actor claim.
    async fn handshake(request: HandshakeRequest) -> Result<HandshakeResponse, NexusError>;

    /// Liveness/identity probe used after handshake (`CONTRACTE_NEXUS_RO.md` §8).
    async fn get_health() -> Result<GetHealthResponse, NexusError>;

    /// Persists the request and queues a run. Nothing is written yet: the
    /// host first prepares an exact preview that must be approved.
    async fn submit_note(request: SubmitNoteRequest) -> Result<SubmitNoteResponse, NexusError>;

    async fn get_snapshot(limit: u32) -> Result<Snapshot, NexusError>;

    async fn get_run(run_id: RunId) -> Result<RunView, NexusError>;

    /// Bound to the manifest hash the client previewed (`CONTRACTE_NEXUS_RO.md` §5).
    async fn decide_approval(request: DecideApprovalRequest) -> Result<RunView, NexusError>;

    async fn cancel_run(run_id: RunId) -> Result<RunView, NexusError>;

    async fn get_events(after: i64, limit: u32) -> Result<EventsPage, NexusError>;

    /// Content of a verified artifact, re-checked against its recorded hash.
    async fn get_artifact_content(artifact_id: ArtifactId) -> Result<String, NexusError>;

    /// Consistent snapshot of the database and the notes it references,
    /// with a manifest of hashes (BACKUP-01). Writes wait while it runs.
    async fn create_backup() -> Result<BackupSummary, NexusError>;

    /// Explicit stop: no new work is taken, the current step finishes,
    /// uncertain effects stay recorded for reconciliation on next start.
    async fn stop_host() -> Result<(), NexusError>;
}
