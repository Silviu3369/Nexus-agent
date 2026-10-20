//! Shared wire types for Nexus: typed identifiers, protocol version and the
//! first DTOs (handshake, health, error). This is the single source for
//! contracts (`CONTRACTE_NEXUS_RO.md` §1) — `nexus-host` and `nexus-client`
//! depend on it; it depends on neither.

pub mod automation;
pub mod chat;
pub mod error;
pub mod files;
pub mod handshake;
pub mod ids;
pub mod mcp;
pub mod memory;
pub mod pairing;
pub mod records;
pub mod runs;
pub mod service;
pub mod usage;
pub mod version;

pub use runs::{
    ApprovalDecision, ArtifactSummary, BackupSummary, DecideApprovalRequest, EventView, EventsPage,
    McpCallPreview, McpCallView, NotePreview, PendingApproval, RunFailure, RunKind, RunOrigin,
    RunStatus, RunView, Snapshot, SubmitNoteRequest, SubmitNoteResponse, SystemStatusView,
};

pub use automation::{
    AutomationRunStatus, AutomationRunView, AutomationView, SaveAutomationRequest, Schedule,
};
pub use chat::{
    AgentStepDetail, AgentStepStatus, AgentStepView, BackupModelView, ChatMessageView, ChatRole,
    ChatStatus, ConnectionView, ConversationSummary, ConversationView, DeletedView, ModelView,
    PersonaView, ProviderChoice, SaveConnectionRequest, SavePersonaRequest, SendMessageRequest,
    SendMessageResponse, ThinkingLevel,
};
pub use error::{ErrorCode, NexusError};
pub use files::{FolderEntryView, FolderView, SaveFolderRequest};
pub use handshake::{GetHealthResponse, HandshakeRequest, HandshakeResponse, HealthStatus};
pub use ids::{
    ActionId, AgentStepId, ApprovalId, ArtifactId, AutomationId, ConnectionId, ConversationId,
    EventId, FolderId, McpServerId, MemoryId, MessageId, OperationId, ProjectId, RecordId,
    RegisterId, RequestId, RunId, StepId, TaskId,
};
pub use mcp::{McpServerView, McpToolView, SaveMcpServerRequest};
pub use memory::{
    AddMemoryRequest, MemoryContextView, MemoryItemView, MemoryQuery, MemoryScope, MemorySource,
    MemoryStatus, ProjectView, SaveProjectRequest,
};
pub use records::{
    AdsHandlingView, CauseCountView, ExportView, FieldChange, FieldDef, FieldKind, GroupCountView,
    ImportPreviewView, ImportRequest, ImportResultView, ImportSkipView, InboxFileStatus,
    InboxFileView, InboxView, JobResultsView, RecordActor, RecordChangeKind, RecordChangeView,
    RecordDetailView, RecordQuery, RecordView, RegisterView, SaveInboxRequest, SaveRecordRequest,
    ScoreBand, ScoreBandView, SourceCountView, WeekCountView,
};
pub use service::{NexusService, NexusServiceClient};
pub use usage::{ModelPrice, ModelUsageView, UsageView};
pub use version::PROTOCOL_VERSION;
