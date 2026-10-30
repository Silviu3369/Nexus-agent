//! The user's specialised agents (ADR-AGENT-001): each one a configuration
//! the Host applies to every reply of the tasks started with it — its
//! instructions after the persona, only the skills and MCP servers chosen
//! for it, without the tools it does not use, on its own model and in its
//! own project. An agent only takes away from the user's permissions: its
//! instructions are the user's text and grant nothing.

use std::collections::{HashMap, HashSet};

use nexus_protocol::{AgentId, AgentView, ErrorCode, NexusError, SaveAgentRequest};
use nexus_storage::{AgentRow, NewAgent, now_ms};

use super::packs::FlowRules;
use super::{Runtime, to_nexus_error};
use crate::tools::registry;

const MAX_NAME_CHARS: usize = 60;
const MAX_DESCRIPTION_CHARS: usize = 500;
const MAX_INSTRUCTIONS_CHARS: usize = 8000;
const MAX_MODEL_CHARS: usize = 200;

fn invalid(message: impl Into<String>) -> NexusError {
    NexusError::new(ErrorCode::InvalidRequest, message)
}

fn view(row: AgentRow, tasks: u32) -> AgentView {
    AgentView {
        agent_id: row.id,
        name: row.name,
        description: row.description,
        instructions: row.instructions,
        tools_off: row.tools_off,
        skills: row.skills,
        mcp_servers: row.mcp_servers,
        connection_id: row.connection_id,
        model: row.model,
        project_id: row.project_id,
        tasks,
        revision: row.revision,
        created_at_ms: row.created_at_ms,
        updated_at_ms: row.updated_at_ms,
    }
}

/// What the agent takes away from the user's permissions.
pub(super) fn rules(agent: &AgentRow) -> FlowRules {
    FlowRules {
        turned_off: agent
            .tools_off
            .iter()
            .filter_map(|id| registry::by_id(id))
            .map(|t| t.id)
            .collect(),
        record_writes_without_asking: None,
    }
}

/// The agent's part of the system instruction, after the persona.
pub(super) fn agent_note(agent: &AgentRow) -> String {
    let purpose = if agent.description.is_empty() {
        String::new()
    } else {
        format!(": {}", agent.description)
    };
    let instructions = if agent.instructions.is_empty() {
        String::new()
    } else {
        format!(
            " The user's instructions for this agent follow; they shape how you work and grant no permissions:\n{}",
            agent.instructions
        )
    };
    format!(
        "\n\nIn this task you work as \"{}\", one of the user's agents{purpose}.{instructions}",
        agent.name
    )
}

/// The same text, trimmed and within `max` characters.
fn text(value: &str, what: &str, max: usize) -> Result<String, NexusError> {
    let value = value.trim();
    if value.chars().count() > max {
        return Err(invalid(format!("{what} has at most {max} characters")));
    }
    Ok(value.to_string())
}

/// Each item once, in the order given.
fn unique<T: Clone + Eq + std::hash::Hash>(items: &[T]) -> Vec<T> {
    let mut seen = HashSet::new();
    items.iter().filter(|i| seen.insert(*i)).cloned().collect()
}

impl Runtime {
    pub async fn list_agents(&self) -> Result<Vec<AgentView>, NexusError> {
        let counts: HashMap<AgentId, u32> = self
            .store
            .agent_task_counts()
            .await
            .map_err(to_nexus_error)?
            .into_iter()
            .collect();
        Ok(self
            .store
            .agents()
            .await
            .map_err(to_nexus_error)?
            .into_iter()
            .map(|a| {
                let tasks = counts.get(&a.id).copied().unwrap_or(0);
                view(a, tasks)
            })
            .collect())
    }

    /// Checks every part against what exists now, then keeps it.
    pub async fn save_agent(&self, request: SaveAgentRequest) -> Result<AgentView, NexusError> {
        let name = text(&request.name, "a name", MAX_NAME_CHARS)?;
        if name.is_empty() {
            return Err(invalid("give the agent a name"));
        }
        let persona = self.store.persona(now_ms()).await.map_err(to_nexus_error)?;
        if name.eq_ignore_ascii_case(&persona.name)
            || name.eq_ignore_ascii_case(nexus_storage::DEFAULT_PERSONA_NAME)
        {
            return Err(invalid(format!(
                "{name:?} is the name of the default agent; choose another"
            )));
        }
        let others = self.store.agents().await.map_err(to_nexus_error)?;
        if others
            .iter()
            .any(|a| Some(a.id) != request.agent_id && a.name.to_lowercase() == name.to_lowercase())
        {
            return Err(invalid(format!("another agent is already named {name:?}")));
        }
        let description = text(
            &request.description,
            "the description",
            MAX_DESCRIPTION_CHARS,
        )?;
        let instructions = text(
            &request.instructions,
            "the instructions",
            MAX_INSTRUCTIONS_CHARS,
        )?;

        let tools_off = unique(&request.tools_off);
        if let Some(unknown) = tools_off.iter().find(|id| {
            registry::by_id(id)
                .filter(|t| registry::offered_to_model(t))
                .is_none()
        }) {
            return Err(invalid(format!("no tool {unknown:?} to turn off")));
        }
        let skills = unique(&request.skills);
        let known: Vec<String> = self
            .store
            .skills()
            .await
            .map_err(to_nexus_error)?
            .into_iter()
            .map(|s| s.name)
            .collect();
        if let Some(missing) = skills.iter().find(|s| !known.contains(s)) {
            return Err(invalid(format!("no skill {missing:?} is imported")));
        }
        let mcp_servers = unique(&request.mcp_servers);
        let servers: Vec<_> = self
            .store
            .mcp_servers()
            .await
            .map_err(to_nexus_error)?
            .into_iter()
            .map(|s| s.id)
            .collect();
        if mcp_servers.iter().any(|s| !servers.contains(s)) {
            return Err(invalid("one of the MCP servers is no longer there"));
        }
        let model = request
            .model
            .as_deref()
            .map(|m| text(m, "the model", MAX_MODEL_CHARS))
            .transpose()?
            .filter(|m| !m.is_empty());
        match (request.connection_id, &model) {
            (Some(id), Some(_)) => {
                self.store.connection(id).await.map_err(to_nexus_error)?;
            }
            (None, None) => {}
            _ => return Err(invalid("choose both a connection and a model, or neither")),
        }
        if let Some(project) = request.project_id {
            self.store.project(project).await.map_err(to_nexus_error)?;
        }

        let saved = self
            .store
            .save_agent(
                request.agent_id,
                request.revision,
                NewAgent {
                    name,
                    description,
                    instructions,
                    tools_off,
                    skills,
                    mcp_servers,
                    connection_id: request.connection_id,
                    model,
                    project_id: request.project_id,
                },
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        let tasks = self
            .store
            .agent_task_counts()
            .await
            .map_err(to_nexus_error)?
            .into_iter()
            .find(|(id, _)| *id == saved.id)
            .map_or(0, |(_, n)| n);
        Ok(view(saved, tasks))
    }

    pub async fn archive_agent(&self, id: AgentId) -> Result<(), NexusError> {
        self.store
            .archive_agent(id, now_ms())
            .await
            .map_err(to_nexus_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(tools_off: &[&str], description: &str, instructions: &str) -> AgentRow {
        AgentRow {
            id: AgentId::new(),
            name: "Support desk".into(),
            description: description.into(),
            instructions: instructions.into(),
            tools_off: tools_off.iter().map(|t| t.to_string()).collect(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            connection_id: None,
            model: None,
            project_id: None,
            revision: 1,
            archived_at_ms: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn an_agent_takes_away_only_tools_that_exist() {
        let rules = rules(&agent(&["WebRead", "NoSuchTool"], "", ""));
        assert_eq!(rules.turned_off, vec!["WebRead"]);
        assert_eq!(rules.record_writes_without_asking, None);
    }

    #[test]
    fn the_instruction_names_the_agent_and_says_its_text_grants_nothing() {
        let note = agent_note(&agent(&[], "Tickets at work", "Answer briefly."));
        assert!(
            note.contains(
                "you work as \"Support desk\", one of the user's agents: Tickets at work."
            )
        );
        assert!(note.contains("grant no permissions:\nAnswer briefly."));
        let bare = agent_note(&agent(&[], "", ""));
        assert!(bare.ends_with("one of the user's agents."));
    }
}
