//! DTOs for the user's specialised agents (ADR-AGENT-001): each one a
//! configuration — its instructions, skills, tools, model and project —
//! that only takes away from the user's permissions. The default agent,
//! Nexus, is not one of them: it is the absence of an agent.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, ConnectionId, McpServerId, ProjectId};

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AgentView {
    pub agent_id: AgentId,
    pub name: String,
    /// What it is for.
    pub description: String,
    /// How it works, in the user's words; added to Nexus's persona.
    pub instructions: String,
    /// Tools (registry ids, as Permissions lists them) it does not use.
    pub tools_off: Vec<String>,
    /// The user's skills, by name, it may use (only those turned on count).
    pub skills: Vec<String>,
    /// The MCP servers it may use.
    pub mcp_servers: Vec<McpServerId>,
    /// Its own model; `None`: the active model.
    pub connection_id: Option<ConnectionId>,
    pub model: Option<String>,
    /// Its tasks go to this project and use its memory; `None`: the
    /// active project.
    pub project_id: Option<ProjectId>,
    /// Tasks started with it.
    pub tasks: u32,
    pub revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveAgentRequest {
    /// `None` creates an agent.
    pub agent_id: Option<AgentId>,
    /// The revision the user edited (0 for a new agent).
    pub revision: i64,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub tools_off: Vec<String>,
    pub skills: Vec<String>,
    pub mcp_servers: Vec<McpServerId>,
    pub connection_id: Option<ConnectionId>,
    pub model: Option<String>,
    pub project_id: Option<ProjectId>,
}
