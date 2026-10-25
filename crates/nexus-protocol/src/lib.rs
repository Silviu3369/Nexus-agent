//! Shared wire types for Nexus: typed identifiers, protocol version and the
//! first DTOs (handshake, health, error). This is the single source for
//! contracts (`CONTRACTE_NEXUS_RO.md` §1) — `nexus-host` and `nexus-client`
//! depend on it; it depends on neither.

pub mod automation;
pub mod chat;
pub mod data;
pub mod error;
pub mod files;
pub mod handshake;
pub mod ids;
pub mod mcp;
pub mod memory;
pub mod pairing;
pub mod permissions;
pub mod records;
pub mod runs;
pub mod service;
pub mod usage;
pub mod version;

pub use data::{
    ConnectionDataView, DataChange, DataPolicyView, SetDataPolicyRequest, SourceDataView,
};
pub use permissions::{PermissionView, SetPermissionRequest};
pub use runs::{
    ActorView, ApprovalDecision, ApprovalPath, ArtifactSummary, BackupSummary,
    DecideApprovalRequest, EventView, EventsPage, McpCallPreview, McpCallView, NotePreview,
    PendingApproval, RecordChangePreview, RunFailure, RunKind, RunOrigin, RunStatus, RunView,
    Snapshot, SubmitNoteRequest, SubmitNoteResponse, SystemStatusView,
};

pub use automation::{
    AutomationRunStatus, AutomationRunView, AutomationView, SaveAutomationRequest, Schedule,
};
pub use chat::{
    Acceptance, AgentStepDetail, AgentStepStatus, AgentStepView, BackupModelView, ChatMessageView,
    ChatRole, ChatStatus, ConnectionView, ConversationSummary, ConversationView, DeletedView,
    ModelView, PersonaView, ProviderChoice, SaveConnectionRequest, SavePersonaRequest,
    SendMessageRequest, SendMessageResponse, ThinkingLevel, TurnView,
};
pub use error::{ErrorCode, NexusError};
pub use files::{FolderEntryView, FolderView, SaveFolderRequest};
pub use handshake::{GetHealthResponse, HandshakeRequest, HandshakeResponse, HealthStatus};
pub use ids::{
    ActionId, ActorId, AgentStepId, ApprovalId, ArtifactId, AutomationId, ConnectionId,
    ConversationId, EventId, FolderId, McpServerId, MemoryId, MessageId, OperationId, PolicyRuleId,
    ProjectId, RecordId, RegisterId, RequestId, RunId, StepId, TaskId,
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
