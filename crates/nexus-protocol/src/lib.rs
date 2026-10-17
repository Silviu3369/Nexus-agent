//! Shared wire types for Nexus: typed identifiers, protocol version and the
//! first DTOs (handshake, health, error). This is the single source for
//! contracts (`CONTRACTE_NEXUS_RO.md` §1) — `nexus-host` and `nexus-client`
//! depend on it; it depends on neither.

pub mod chat;
pub mod error;
pub mod handshake;
pub mod ids;
pub mod mcp;
pub mod memory;
pub mod pairing;
pub mod runs;
pub mod service;
pub mod usage;
pub mod version;

pub use runs::{
    ApprovalDecision, ArtifactSummary, BackupSummary, DecideApprovalRequest, EventView, EventsPage,
    McpCallPreview, McpCallView, NotePreview, PendingApproval, RunFailure, RunKind, RunOrigin,
    RunStatus, RunView, Snapshot, SubmitNoteRequest, SubmitNoteResponse, SystemStatusView,
};

pub use chat::{
    AgentStepDetail, AgentStepStatus, AgentStepView, BackupModelView, ChatMessageView, ChatRole,
    ChatStatus, ConnectionView, ConversationSummary, ConversationView, DeletedView, ModelView,
    PersonaView, ProviderChoice, SaveConnectionRequest, SavePersonaRequest, SendMessageRequest,
    SendMessageResponse, ThinkingLevel,
};
pub use error::{ErrorCode, NexusError};
pub use handshake::{GetHealthResponse, HandshakeRequest, HandshakeResponse, HealthStatus};
pub use ids::{
    ActionId, AgentStepId, ApprovalId, ArtifactId, ConnectionId, ConversationId, EventId,
    McpServerId, MemoryId, MessageId, OperationId, ProjectId, RequestId, RunId, StepId, TaskId,
};
pub use mcp::{McpServerView, McpToolView, SaveMcpServerRequest};
pub use memory::{
    AddMemoryRequest, MemoryContextView, MemoryItemView, MemoryQuery, MemoryScope, MemorySource,
    MemoryStatus, ProjectView, SaveProjectRequest,
};
pub use service::{NexusService, NexusServiceClient};
pub use usage::{ModelPrice, ModelUsageView, UsageView};
pub use version::PROTOCOL_VERSION;
