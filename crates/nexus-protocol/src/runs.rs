//! DTOs for the M1 circuit: submit a note, preview it exactly, approve or
//! deny, follow the run, read the result (`CONTRACTE_NEXUS_RO.md` §5, §7, §8).
//! Integers that reach the webview are below 2^53 (unix-ms times, sequence
//! numbers, sizes ≤ 1 MiB); identifiers and hashes are strings.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{ApprovalId, ArtifactId, ConversationId, MessageId, RunId, TaskId};

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
    /// For a call to an MCP tool: what the server says about the tool, as
    /// approved (from `get_run` only, like `preview`).
    pub call: Option<McpCallPreview>,
}

/// What a run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum RunKind {
    /// Create a Markdown note (`CreateMarkdownArtifact`).
    Note,
    /// Call a tool of an MCP server that can change things there.
    McpCall,
    /// Create a new document (Word or text) in a folder the user shared.
    Document,
}

/// A call to an MCP tool, exactly as the user approves it.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct McpCallView {
    pub server: String,
    pub url: String,
    pub tool: String,
    /// The exact arguments, pretty-printed.
    pub arguments_json: String,
    /// After the call: how much the server answered (never what).
    pub result_chars: Option<u32>,
    pub truncated: Option<bool>,
    pub duration_ms: Option<u32>,
}

/// The tool as the user enabled it; the server's own words, untrusted.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct McpCallPreview {
    pub description: String,
    pub destructive_hint: Option<bool>,
    pub definition_sha256: String,
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
    /// ADR-ACTION-001: the class of its effect (Read, InternalReversible,
    /// ExternalReversible, ExternalIrreversible) and the evidence its
    /// success has (Verified, Reported, Unverifiable).
    pub effect_class: Option<String>,
    pub evidence: Option<String>,
    /// When the user undid its effect (a document Nexus created).
    pub undone_at_ms: Option<i64>,
    /// Its effect can be undone now.
    pub can_undo: bool,
    pub revision: i64,
    pub failure: Option<RunFailure>,
    pub pending_approval: Option<PendingApproval>,
    pub artifact: Option<ArtifactSummary>,
    /// The agent reply that proposed this run; `None` when the user
    /// submitted the note directly.
    pub origin: Option<RunOrigin>,
    pub kind: RunKind,
    /// For an MCP call: which, with what.
    pub call: Option<McpCallView>,
    /// ADR-IDENTITY-001: who asked for it (the user or the agent) and who
    /// decided its approval, once decided.
    pub requested_by: Option<ActorView>,
    pub decided_by: Option<ActorView>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Who did or asked for something (ADR-IDENTITY-001).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ActorView {
    /// User, Agent, Automation, Intake, Channel or System.
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RunOrigin {
    pub message_id: MessageId,
    pub conversation_id: ConversationId,
    pub conversation_title: String,
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
pub struct BackupSummary {
    pub path: String,
    pub created_at_ms: i64,
    pub artifacts: u32,
    /// Notes whose file no longer matched the hash recorded at creation
    /// (edited outside Nexus); they are backed up as found and flagged.
    pub unmatched_artifacts: u32,
    pub database_sha256: String,
}

/// Facts about the machine and the host process, for the dashboard. Each
/// figure is measured when asked; one that cannot be read is `None`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SystemStatusView {
    pub host_version: String,
    /// All logical processors, since the previous reading (or over a short
    /// sample on the first one).
    pub cpu_percent: Option<f32>,
    pub memory_used_mib: Option<u32>,
    pub memory_total_mib: Option<u32>,
    pub logical_processors: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EventsPage {
    pub events: Vec<EventView>,
    /// Pass back as `after` for the next page.
    pub next_after: i64,
}
