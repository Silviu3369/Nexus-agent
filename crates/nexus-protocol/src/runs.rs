//! DTOs for the M1 circuit: submit a note, preview it exactly, approve or
//! deny, follow the run, read the result (`CONTRACTE_NEXUS_RO.md` §5, §7, §8).
//! Integers that reach the webview are below 2^53 (unix-ms times, sequence
//! numbers, sizes ≤ 1 MiB); identifiers and hashes are strings.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{ApprovalId, ArtifactId, RunId, TaskId};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SubmitNoteRequest {
    /// Chosen by the client per user intent; repeating it with the same
    /// note returns the original run instead of creating another (RUN-03).
    pub idempotency_key: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SubmitNoteResponse {
    pub task_id: TaskId,
    pub run_id: RunId,
    /// False when the idempotency key matched an earlier identical request.
    pub created: bool,
}

/// Mirrors `nexus_core::RunState`; the host converts with an exhaustive match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum RunStatus {
    Queued,
    Running,
    AwaitingApproval,
    Verifying,
    CancellationRequested,
    NeedsReconciliation,
    NeedsReview,
    Succeeded,
    Failed,
    Cancelled,
    Denied,
}

/// Exactly what will be written and where. Built by the host from the same
/// manifest the approval is bound to.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NotePreview {
    pub destination: String,
    pub content: String,
    pub size_bytes: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PendingApproval {
    pub approval_id: ApprovalId,
    /// The client sends this back with its decision; the host refuses a
    /// decision for any other manifest (SEC-01).
    pub manifest_sha256: String,
    pub expires_at_ms: i64,
    /// Present from `get_run` only; snapshots omit it so their size stays
    /// bounded. The client must fetch the run and show this before deciding.
    pub preview: Option<NotePreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ArtifactSummary {
    pub artifact_id: ArtifactId,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u32,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RunFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RunView {
    pub run_id: RunId,
    pub title: String,
    pub status: RunStatus,
    pub revision: i64,
    pub failure: Option<RunFailure>,
    pub pending_approval: Option<PendingApproval>,
    pub artifact: Option<ArtifactSummary>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Snapshot {
    pub runs: Vec<RunView>,
    /// Sequence of the last event reflected in `runs`; continue from here.
    pub watermark: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DecideApprovalRequest {
    pub approval_id: ApprovalId,
    pub manifest_sha256: String,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EventView {
    pub seq: i64,
    pub kind: String,
    pub entity_id: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EventsPage {
    pub events: Vec<EventView>,
    /// Pass back as `after` for the next page.
    pub next_after: i64,
}
