//! DTOs for M2: AI provider connections, persona and conversations
//! (`PLAN_NEXUS_RO.md` M2, `CONTRACTE_NEXUS_RO.md` §8, §11). An API key
//! travels to the host once, in `SaveConnectionRequest`, and no DTO
//! ever carries it back.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{
    AgentStepId, ConnectionId, ConversationId, MemoryId, MessageId, ProjectId, RunId,
};
use crate::memory::MemoryScope;
use crate::runs::RunFailure;

/// The user's speed/quality choice, where the model supports it (Gemini
/// 3.x). Ignored for a model that does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ThinkingLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConnectionView {
    pub connection_id: ConnectionId,
    pub provider: String,
    pub display_name: String,
    pub model: Option<String>,
    /// Declared by the user: Google may use free-tier traffic to improve its
    /// products, so the UI keeps this visible.
    pub free_tier: bool,
    pub has_key: bool,
    pub active: bool,
    pub thinking_level: ThinkingLevel,
}

/// The AI providers this build can talk to. `ConnectionView::provider`
/// carries the same names as text ("Gemini", "OpenAI", "Anthropic",
/// "DeepSeek").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ProviderChoice {
    Gemini,
    OpenAi,
    Anthropic,
    DeepSeek,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveConnectionRequest {
    /// `None` creates a connection; `Some` updates that one.
    pub connection_id: Option<ConnectionId>,
    /// `None` names it after the provider.
    pub display_name: Option<String>,
    /// `None` lets the host read the provider from the key's format; when
    /// the format does not settle it, the request is refused and the user
    /// chooses. Ignored for an existing connection (it keeps its provider).
    pub provider: Option<ProviderChoice>,
    /// Declared by the user; only meaningful for Google AI Studio keys.
    pub free_tier: bool,
    /// Sent once; stored in the Windows vault; never returned. `None` keeps
    /// the key already stored for an existing connection.
    pub api_key: Option<String>,
}

/// The model used only when the active one is overloaded, chosen by the
/// user. There is no other fallback.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct BackupModelView {
    pub connection_id: ConnectionId,
    pub display_name: String,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelView {
    pub id: String,
    pub display_name: String,
    pub input_token_limit: Option<u32>,
    pub output_token_limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PersonaView {
    pub version: i64,
    pub name: String,
    pub tone: String,
    pub style: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SavePersonaRequest {
    pub name: String,
    pub tone: String,
    pub style: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ChatRole {
    User,
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ChatStatus {
    Streaming,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ChatMessageView {
    pub message_id: MessageId,
    pub role: ChatRole,
    /// For a Streaming reply: what has arrived so far (not yet persisted).
    pub text: String,
    pub status: ChatStatus,
    pub model: Option<String>,
    pub persona_version: Option<i64>,
    pub failure: Option<RunFailure>,
    /// A note the model proposed; it waits for the user's approval like any
    /// other run — the model cannot approve it.
    pub proposed_run_id: Option<RunId>,
    pub prompt_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub created_at_ms: i64,
    /// What the agent did for this reply, in order (empty for the user's
    /// own messages).
    pub steps: Vec<AgentStepView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum AgentStepStatus {
    Running,
    Done,
    Failed,
}

/// One step's kind-specific facts. Metadata only: a web read shows its
/// address, status and size, never the page's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind")]
pub enum AgentStepDetail {
    ModelCall {
        model: String,
        /// Time until the provider's first event — mostly its own thinking.
        first_byte_ms: Option<u32>,
        duration_ms: Option<u32>,
        error: Option<String>,
        /// 2 or 3 when this call repeats one the provider answered with a
        /// temporary outage; absent for a first try.
        attempt: Option<u32>,
        /// True when the user's backup model answered because the active
        /// one stayed unavailable.
        backup: Option<bool>,
        /// How many approved memory facts the request carried.
        memory_items: Option<u32>,
    },
    Fetch {
        url: String,
        final_url: Option<String>,
        http_status: Option<u16>,
        /// Size of the extracted text that was given to the model.
        text_bytes: Option<u32>,
        truncated: Option<bool>,
        error: Option<String>,
    },
    MemoryProposed {
        /// The proposed wording (shortened); the fact itself is in memory,
        /// waiting for the user.
        content: String,
        scope: Option<MemoryScope>,
        memory_id: Option<MemoryId>,
        error: Option<String>,
    },
    /// A tool of a remote MCP server the user enabled.
    McpCall {
        server: String,
        tool: String,
        /// What was sent, shortened (the model chose it).
        arguments: String,
        result_chars: Option<u32>,
        truncated: Option<bool>,
        /// The tool reported a failure of its own.
        is_error: Option<bool>,
        duration_ms: Option<u32>,
        error: Option<String>,
        /// A tool that can change things: the call waits for the user's
        /// approval as this run instead of running.
        run_id: Option<RunId>,
    },
    NoteProposed {
        title: String,
        /// The run waiting for the user's approval, when the proposal was
        /// accepted as a run at all.
        run_id: Option<RunId>,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentStepView {
    pub step_id: AgentStepId,
    pub message_id: MessageId,
    pub conversation_id: ConversationId,
    pub conversation_title: String,
    pub seq: i64,
    pub status: AgentStepStatus,
    pub detail: AgentStepDetail,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConversationSummary {
    pub conversation_id: ConversationId,
    pub project_id: Option<ProjectId>,
    pub title: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConversationView {
    pub conversation_id: ConversationId,
    pub project_id: Option<ProjectId>,
    pub title: String,
    pub messages: Vec<ChatMessageView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SendMessageRequest {
    /// `None` starts a new conversation.
    pub conversation_id: Option<ConversationId>,
    /// The project a new conversation joins (`None`: the active project).
    /// A follow-up stays in its conversation's project.
    pub project_id: Option<ProjectId>,
    pub idempotency_key: String,
    pub text: String,
}

/// What a deletion removed.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DeletedView {
    pub conversation_id: ConversationId,
    pub messages: u32,
    /// The whole task is gone (it was deleted, or its last exchange was).
    pub conversation_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SendMessageResponse {
    pub conversation_id: ConversationId,
    pub user_message_id: MessageId,
    pub reply_message_id: MessageId,
}
