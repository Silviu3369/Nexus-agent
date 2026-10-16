//! Bridge commands for projects and memory. They carry the user's choices
//! to the host; every check (lengths, scope, revisions) is made there.

use nexus_protocol::{
    AddMemoryRequest, MemoryContextView, MemoryId, MemoryItemView, MemoryQuery, MemoryScope,
    MemoryStatus, ProjectId, ProjectView, SaveProjectRequest,
};

use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn list_projects(connection: State<'_>) -> Result<Vec<ProjectView>, BridgeError> {
    connection.call(async |c| c.projects().await).await
}

/// `project_id: None` creates a project.
#[tauri::command]
pub async fn save_project(
    connection: State<'_>,
    project_id: Option<ProjectId>,
    name: String,
    description: String,
) -> Result<ProjectView, BridgeError> {
    connection
        .call(async |c| {
            c.save_project(SaveProjectRequest {
                project_id,
                name,
                description,
            })
            .await
        })
        .await
}

#[tauri::command]
pub async fn set_active_project(
    connection: State<'_>,
    project_id: ProjectId,
) -> Result<ProjectView, BridgeError> {
    connection
        .call(async |c| c.set_active_project(project_id).await)
        .await
}

#[tauri::command]
pub async fn list_memory(
    connection: State<'_>,
    text: Option<String>,
    scope: Option<MemoryScope>,
    project_id: Option<ProjectId>,
    status: Option<MemoryStatus>,
    limit: u32,
) -> Result<Vec<MemoryItemView>, BridgeError> {
    connection
        .call(async |c| {
            c.memory(MemoryQuery {
                text,
                scope,
                project_id,
                status,
                limit,
            })
            .await
        })
        .await
}

#[tauri::command]
pub async fn add_memory(
    connection: State<'_>,
    scope: MemoryScope,
    project_id: Option<ProjectId>,
    content: String,
) -> Result<MemoryItemView, BridgeError> {
    connection
        .call(async |c| {
            c.add_memory(AddMemoryRequest {
                scope,
                project_id,
                content,
            })
            .await
        })
        .await
}

#[tauri::command]
pub async fn edit_memory(
    connection: State<'_>,
    memory_id: MemoryId,
    revision: i64,
    content: String,
) -> Result<MemoryItemView, BridgeError> {
    connection
        .call(async |c| c.edit_memory(memory_id, revision, content).await)
        .await
}

/// `revision` is the one the user saw; the host refuses a stale decision.
#[tauri::command]
pub async fn decide_memory(
    connection: State<'_>,
    memory_id: MemoryId,
    revision: i64,
    approve: bool,
) -> Result<MemoryItemView, BridgeError> {
    connection
        .call(async |c| c.decide_memory(memory_id, revision, approve).await)
        .await
}

#[tauri::command]
pub async fn delete_memory(connection: State<'_>, memory_id: MemoryId) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.delete_memory(memory_id).await)
        .await
}

#[tauri::command]
pub async fn memory_context(
    connection: State<'_>,
    project_id: ProjectId,
) -> Result<MemoryContextView, BridgeError> {
    connection
        .call(async |c| c.memory_context(project_id).await)
        .await
}
