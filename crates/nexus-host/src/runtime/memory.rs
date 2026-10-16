//! Projects and memory over RPC. The user reads, writes, corrects, decides
//! on and deletes facts here; the agent only proposes them (`chat.rs`).
//! Validation happens here, before storage: lengths, and a fact's scope
//! matching whether it names a project.

use nexus_core::{MemoryScope, MemorySource, MemoryStatus};
use nexus_protocol::{
    AddMemoryRequest, ErrorCode, MemoryContextView, MemoryId, MemoryItemView, MemoryQuery,
    NexusError, ProjectId, ProjectView, SaveProjectRequest,
};
use nexus_storage::{MemoryFilter, MemoryRow, NewMemory, ProjectRow, now_ms};

use super::chat::{MEMORY_CONTEXT_CHARS, MEMORY_CONTEXT_ITEMS};
use super::{Runtime, to_nexus_error};

const MAX_PROJECT_NAME: usize = 100;
const MAX_PROJECT_DESCRIPTION: usize = 1000;
const MAX_FACT_CHARS: usize = 1000;
const MAX_QUERY_CHARS: usize = 200;

fn invalid(message: impl Into<String>) -> NexusError {
    NexusError::new(ErrorCode::InvalidRequest, message)
}

impl Runtime {
    pub async fn list_projects(&self) -> Result<Vec<ProjectView>, NexusError> {
        let active = self.store.active_project().await.map_err(to_nexus_error)?;
        let rows = self.store.projects().await.map_err(to_nexus_error)?;
        Ok(rows.iter().map(|r| project_view(r, active)).collect())
    }

    pub async fn save_project(
        &self,
        request: SaveProjectRequest,
    ) -> Result<ProjectView, NexusError> {
        let name = request.name.trim().to_string();
        if name.is_empty() || name.chars().count() > MAX_PROJECT_NAME {
            return Err(invalid(format!(
                "a project name must have 1..={MAX_PROJECT_NAME} characters"
            )));
        }
        let description = request.description.trim().to_string();
        if description.chars().count() > MAX_PROJECT_DESCRIPTION {
            return Err(invalid(format!(
                "a description must have at most {MAX_PROJECT_DESCRIPTION} characters"
            )));
        }
        let row = match request.project_id {
            Some(id) => {
                self.store
                    .update_project(id, name, description, now_ms())
                    .await
            }
            None => self.store.create_project(name, description, now_ms()).await,
        }
        .map_err(to_nexus_error)?;
        let active = self.store.active_project().await.map_err(to_nexus_error)?;
        Ok(project_view(&row, active))
    }

    pub async fn set_active_project(&self, id: ProjectId) -> Result<ProjectView, NexusError> {
        self.store
            .set_active_project(id, now_ms())
            .await
            .map_err(to_nexus_error)?;
        let row = self.store.project(id).await.map_err(to_nexus_error)?;
        Ok(project_view(&row, id))
    }

    pub async fn list_memory(&self, query: MemoryQuery) -> Result<Vec<MemoryItemView>, NexusError> {
        let text = query
            .text
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        if text
            .as_ref()
            .is_some_and(|t| t.chars().count() > MAX_QUERY_CHARS)
        {
            return Err(invalid(format!(
                "a search has at most {MAX_QUERY_CHARS} characters"
            )));
        }
        let rows = self
            .store
            .search_memory(MemoryFilter {
                text,
                scope: query.scope.map(from_scope),
                project_id: query.project_id,
                status: query.status.map(from_status),
                limit: query.limit.clamp(1, 500),
            })
            .await
            .map_err(to_nexus_error)?;
        Ok(rows.iter().map(memory_view).collect())
    }

    pub async fn add_memory(
        &self,
        request: AddMemoryRequest,
    ) -> Result<MemoryItemView, NexusError> {
        let content = checked_fact(&request.content)?;
        let scope = from_scope(request.scope);
        match (scope, request.project_id) {
            (MemoryScope::Project, Some(project)) => {
                self.store.project(project).await.map_err(to_nexus_error)?;
            }
            (MemoryScope::Project, None) => {
                return Err(invalid("a project fact needs its project"));
            }
            (MemoryScope::Personal, Some(_)) => {
                return Err(invalid("a personal fact belongs to no project"));
            }
            (MemoryScope::Personal, None) => {}
        }
        let row = self
            .store
            .add_memory(
                NewMemory {
                    scope,
                    project_id: request.project_id,
                    content,
                    source: MemorySource::User,
                    conversation_id: None,
                    message_id: None,
                    influences: Vec::new(),
                },
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        Ok(memory_view(&row))
    }

    pub async fn edit_memory(
        &self,
        id: MemoryId,
        revision: i64,
        content: String,
    ) -> Result<MemoryItemView, NexusError> {
        let content = checked_fact(&content)?;
        let row = self
            .store
            .edit_memory(id, revision, content, now_ms())
            .await
            .map_err(to_nexus_error)?;
        Ok(memory_view(&row))
    }

    pub async fn decide_memory(
        &self,
        id: MemoryId,
        revision: i64,
        approve: bool,
    ) -> Result<MemoryItemView, NexusError> {
        let row = self
            .store
            .decide_memory(id, revision, approve, now_ms())
            .await
            .map_err(to_nexus_error)?;
        tracing::info!(memory = %id, approved = approve, "memory proposal decided");
        Ok(memory_view(&row))
    }

    pub async fn delete_memory(&self, id: MemoryId) -> Result<(), NexusError> {
        self.store
            .delete_memory(id, now_ms())
            .await
            .map_err(to_nexus_error)
    }

    pub async fn memory_context(
        &self,
        project: ProjectId,
    ) -> Result<MemoryContextView, NexusError> {
        self.store.project(project).await.map_err(to_nexus_error)?;
        let items = self
            .store
            .memory_context(project, MEMORY_CONTEXT_ITEMS, MEMORY_CONTEXT_CHARS)
            .await
            .map_err(to_nexus_error)?;
        Ok(MemoryContextView {
            project_id: project,
            items: items.iter().map(memory_view).collect(),
            max_items: MEMORY_CONTEXT_ITEMS as u32,
            max_chars: MEMORY_CONTEXT_CHARS as u32,
        })
    }
}

fn checked_fact(content: &str) -> Result<String, NexusError> {
    let content = content.trim();
    if content.is_empty() || content.chars().count() > MAX_FACT_CHARS {
        return Err(invalid(format!(
            "a fact must have 1..={MAX_FACT_CHARS} characters"
        )));
    }
    Ok(content.to_string())
}

fn project_view(row: &ProjectRow, active: ProjectId) -> ProjectView {
    ProjectView {
        project_id: row.id,
        name: row.name.clone(),
        description: row.description.clone(),
        active: row.id == active,
        conversations: u32::try_from(row.conversations).unwrap_or(u32::MAX),
        memory_items: u32::try_from(row.memory_items).unwrap_or(u32::MAX),
        created_at_ms: row.created_at_ms,
    }
}

fn memory_view(row: &MemoryRow) -> MemoryItemView {
    MemoryItemView {
        memory_id: row.id,
        scope: to_scope(row.scope),
        project_id: row.project_id,
        project_name: row.project_name.clone(),
        content: row.content.clone(),
        status: match row.status {
            MemoryStatus::Proposed => nexus_protocol::MemoryStatus::Proposed,
            MemoryStatus::Active => nexus_protocol::MemoryStatus::Active,
            MemoryStatus::Rejected => nexus_protocol::MemoryStatus::Rejected,
        },
        source: match row.source {
            MemorySource::User => nexus_protocol::MemorySource::User,
            MemorySource::Agent => nexus_protocol::MemorySource::Agent,
        },
        edited_by_user: row.edited_by_user,
        conversation_id: row.conversation_id,
        conversation_title: row.conversation_title.clone(),
        influences: row.influences.clone(),
        revision: row.revision,
        created_at_ms: row.created_at_ms,
        updated_at_ms: row.updated_at_ms,
        decided_at_ms: row.decided_at_ms,
    }
}

fn to_scope(scope: MemoryScope) -> nexus_protocol::MemoryScope {
    match scope {
        MemoryScope::Personal => nexus_protocol::MemoryScope::Personal,
        MemoryScope::Project => nexus_protocol::MemoryScope::Project,
    }
}

fn from_scope(scope: nexus_protocol::MemoryScope) -> MemoryScope {
    match scope {
        nexus_protocol::MemoryScope::Personal => MemoryScope::Personal,
        nexus_protocol::MemoryScope::Project => MemoryScope::Project,
    }
}

fn from_status(status: nexus_protocol::MemoryStatus) -> MemoryStatus {
    match status {
        nexus_protocol::MemoryStatus::Proposed => MemoryStatus::Proposed,
        nexus_protocol::MemoryStatus::Active => MemoryStatus::Active,
        nexus_protocol::MemoryStatus::Rejected => MemoryStatus::Rejected,
    }
}
