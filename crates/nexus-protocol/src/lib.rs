//! Shared wire types for Nexus: typed identifiers, protocol version and the
//! first DTOs (handshake, health, error). This is the single source for
//! contracts (`CONTRACTE_NEXUS_RO.md` §1) — `nexus-host` and `nexus-client`
//! depend on it; it depends on neither.

pub mod error;
pub mod handshake;
pub mod ids;
pub mod pairing;
pub mod runs;
pub mod service;
pub mod version;

pub use runs::{
    ApprovalDecision, ArtifactSummary, BackupSummary, DecideApprovalRequest, EventView, EventsPage,
    NotePreview, PendingApproval, RunFailure, RunStatus, RunView, Snapshot, SubmitNoteRequest,
    SubmitNoteResponse,
};

pub use error::{ErrorCode, NexusError};
pub use handshake::{GetHealthResponse, HandshakeRequest, HandshakeResponse, HealthStatus};
pub use ids::{
    ActionId, ApprovalId, ArtifactId, EventId, OperationId, ProjectId, RequestId, RunId, StepId,
    TaskId,
};
pub use service::{NexusService, NexusServiceClient};
pub use version::PROTOCOL_VERSION;
