//! DTOs for M2: AI provider connections, persona and conversations
//! (`PLAN_NEXUS_RO.md` M2, `CONTRACTE_NEXUS_RO.md` §8, §11). An API key
//! travels to the host once, in `SaveGeminiConnectionRequest`, and no DTO
//! ever carries it back.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{ConnectionId, ConversationId, MessageId, RunId};
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

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveGeminiConnectionRequest {
    /// `None` creates a connection; `Some` updates that one.
    pub connection_id: Option<ConnectionId>,
    pub display_name: String,
    pub free_tier: bool,
    /// Sent once; stored in the Windows vault; never returned. `None` keeps
    /// the key already stored for an existing connection.
    pub api_key: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConversationSummary {
    pub conversation_id: ConversationId,
    pub title: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConversationView {
    pub conversation_id: ConversationId,
    pub title: String,
    pub messages: Vec<ChatMessageView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SendMessageRequest {
    /// `None` starts a new conversation.
    pub conversation_id: Option<ConversationId>,
    pub idempotency_key: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SendMessageResponse {
    pub conversation_id: ConversationId,
    pub user_message_id: MessageId,
    pub reply_message_id: MessageId,
}
