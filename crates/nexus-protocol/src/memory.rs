//! DTOs for projects and memory (`CONTRACTE_NEXUS_RO.md` §3: Project,
//! MemoryItem). A memory fact is about the user (Personal, used in every
//! project) or about one project; the agent only proposes facts, the user
//! decides, and only active facts are ever sent to a model.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{ConversationId, MemoryId, ProjectId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum MemoryScope {
    Personal,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum MemoryStatus {
    /// Proposed by the agent, waiting for the user.
    Proposed,
    Active,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum MemorySource {
    User,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ProjectView {
    pub project_id: ProjectId,
    pub name: String,
    pub description: String,
    /// New tasks go to the active project.
    pub active: bool,
    pub conversations: u32,
    /// Active facts of this project (personal ones not counted).
    pub memory_items: u32,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveProjectRequest {
    /// `None` creates a project.
    pub project_id: Option<ProjectId>,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MemoryItemView {
    pub memory_id: MemoryId,
    pub scope: MemoryScope,
    pub project_id: Option<ProjectId>,
    pub project_name: Option<String>,
    pub content: String,
    pub status: MemoryStatus,
    pub source: MemorySource,
    /// The user changed the agent's wording.
    pub edited_by_user: bool,
    /// The task it was proposed in, if any.
    pub conversation_id: Option<ConversationId>,
    pub conversation_title: Option<String>,
    /// Hosts the agent had read in that reply before proposing it.
    pub influences: Vec<String>,
    /// Sent back with an edit or a decision; a stale one is refused.
    pub revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub decided_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct MemoryQuery {
    /// Words to find; diacritics are ignored and words match as prefixes.
    pub text: Option<String>,
    pub scope: Option<MemoryScope>,
    /// Facts usable in this project: its own and the personal ones.
    pub project_id: Option<ProjectId>,
    pub status: Option<MemoryStatus>,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AddMemoryRequest {
    pub scope: MemoryScope,
    /// Required for a Project fact.
    pub project_id: Option<ProjectId>,
    pub content: String,
}

/// Exactly the facts a request in this project carries to the model.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MemoryContextView {
    pub project_id: ProjectId,
    pub items: Vec<MemoryItemView>,
    pub max_items: u32,
    pub max_chars: u32,
}
