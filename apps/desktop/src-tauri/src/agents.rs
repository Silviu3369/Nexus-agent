//! Bridge commands for the user's agents (ADR-AGENT-001). The host checks
//! every part of an agent against what exists; an agent only takes away
//! from the user's permissions.

use nexus_protocol::{AgentId, AgentView, SaveAgentRequest};

use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn list_agents(connection: State<'_>) -> Result<Vec<AgentView>, BridgeError> {
    connection.call(async |c| c.agents().await).await
}

#[tauri::command]
pub async fn save_agent(
    connection: State<'_>,
    request: SaveAgentRequest,
) -> Result<AgentView, BridgeError> {
    connection.call(async |c| c.save_agent(request).await).await
}

#[tauri::command]
pub async fn archive_agent(connection: State<'_>, agent_id: AgentId) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.archive_agent(agent_id).await)
        .await
}
