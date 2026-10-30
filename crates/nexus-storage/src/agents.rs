//! The user's specialised agents (`schema::V24`, ADR-AGENT-001): each one a
//! configuration the Host applies to the tasks started with it. Each agent
//! is an actor under its own name, so what it wrote names it; an agent is
//! archived, never deleted. Events carry the name and the revision, never
//! the instructions.

use nexus_protocol::{AgentId, ConnectionId, ConversationId, McpServerId, ProjectId};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_uuid};
use crate::{StorageError, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub id: AgentId,
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// Tools (registry ids) the agent does not use, whatever the user allows.
    pub tools_off: Vec<String>,
    /// The user's skills (by name) the agent may use.
    pub skills: Vec<String>,
    /// The MCP servers the agent may use.
    pub mcp_servers: Vec<McpServerId>,
    /// Its own model: a connection and a model on it (else the active one).
    pub connection_id: Option<ConnectionId>,
    pub model: Option<String>,
    /// Its tasks go to this project and use its memory.
    pub project_id: Option<ProjectId>,
    pub revision: i64,
    pub archived_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// An agent as the Host checked it, ready to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAgent {
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

const COLUMNS: &str = "id, name, description, instructions, tools_off_json, skills_json, mcp_servers_json, \
                       connection_id, model, project_id, revision, archived_at_ms, created_at_ms, updated_at_ms";

type RawAgent = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
    i64,
    i64,
);

fn raw_agent(r: &rusqlite::Row) -> rusqlite::Result<RawAgent> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
    ))
}

fn list(text: &str) -> Result<Vec<String>, StorageError> {
    serde_json::from_str(text).map_err(|e| StorageError::Corrupt(format!("agent list: {e}")))
}

fn parse(raw: RawAgent) -> Result<AgentRow, StorageError> {
    let (
        id,
        name,
        description,
        instructions,
        tools_off,
        skills,
        mcp_servers,
        connection_id,
        model,
        project_id,
        revision,
        archived_at_ms,
        created_at_ms,
        updated_at_ms,
    ) = raw;
    Ok(AgentRow {
        id: AgentId(parse_uuid(&id)?),
        name,
        description,
        instructions,
        tools_off: list(&tools_off)?,
        skills: list(&skills)?,
        mcp_servers: list(&mcp_servers)?
            .iter()
            .map(|s| parse_uuid(s).map(McpServerId))
            .collect::<Result<_, _>>()?,
        connection_id: connection_id
            .as_deref()
            .map(parse_uuid)
            .transpose()?
            .map(ConnectionId),
        model,
        project_id: project_id
            .as_deref()
            .map(parse_uuid)
            .transpose()?
            .map(ProjectId),
        revision,
        archived_at_ms,
        created_at_ms,
        updated_at_ms,
    })
}

fn load(conn: &Connection, id: AgentId) -> Result<AgentRow, StorageError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM agents WHERE id = ?1"),
        [id.to_string()],
        raw_agent,
    )
    .optional()?
    .map(parse)
    .transpose()?
    .ok_or(StorageError::NotFound("agent"))
}

fn json_list(items: &[String]) -> String {
    serde_json::Value::from(items.to_vec()).to_string()
}

/// The actor an agent acts as.
fn reference(id: AgentId) -> String {
    format!("agent:{id}")
}

/// The actor of the agent a run's task was started with; `None` for the
/// default agent.
pub(crate) fn actor_of_run(tx: &Transaction, run: &str) -> Result<Option<String>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT a.actor_id FROM runs r JOIN tasks t ON t.id = r.task_id
             JOIN agents a ON a.id = t.agent_id WHERE r.id = ?1",
            [run],
            |r| r.get(0),
        )
        .optional()?)
}

impl Store {
    /// The agents in use (not archived), by name.
    pub async fn agents(&self) -> Result<Vec<AgentRow>, StorageError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM agents WHERE archived_at_ms IS NULL ORDER BY lower(name)"
            ))?;
            let raw = stmt
                .query_map([], raw_agent)?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(parse).collect()
        })
        .await
    }

    /// One agent, archived or not.
    pub async fn agent(&self, id: AgentId) -> Result<AgentRow, StorageError> {
        self.call(move |conn| load(conn, id)).await
    }

    /// The agent a conversation's task was started with (`None`: the
    /// default agent).
    pub async fn conversation_agent(
        &self,
        conversation: ConversationId,
    ) -> Result<Option<AgentRow>, StorageError> {
        self.call(move |conn| {
            let id: Option<String> = conn
                .query_row(
                    "SELECT agent_id FROM tasks WHERE conversation_id = ?1",
                    [conversation.to_string()],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            id.map(|id| load(conn, AgentId(parse_uuid(&id)?)))
                .transpose()
        })
        .await
    }

    /// Tasks started with each agent.
    pub async fn agent_task_counts(&self) -> Result<Vec<(AgentId, u32)>, StorageError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT agent_id, count(*) FROM tasks WHERE agent_id IS NOT NULL GROUP BY agent_id",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|(id, n)| Ok((AgentId(parse_uuid(&id)?), n)))
                .collect()
        })
        .await
    }

    /// Adds an agent (`id` `None`), with its own actor, or changes the
    /// revision the user edited.
    pub async fn save_agent(
        &self,
        id: Option<AgentId>,
        revision: i64,
        new: NewAgent,
        now: i64,
    ) -> Result<AgentRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let fields = (
                json_list(&new.tools_off),
                json_list(&new.skills),
                json_list(
                    &new.mcp_servers
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                ),
                new.connection_id.map(|c| c.to_string()),
                new.project_id.map(|p| p.to_string()),
            );
            let (id, revision) = match id {
                Some(id) => {
                    let current = load(&tx, id)?;
                    if current.revision != revision || current.archived_at_ms.is_some() {
                        return Err(StorageError::RevisionConflict("agent"));
                    }
                    tx.execute(
                        "UPDATE agents SET name = ?1, description = ?2, instructions = ?3, tools_off_json = ?4,
                         skills_json = ?5, mcp_servers_json = ?6, connection_id = ?7, model = ?8, project_id = ?9,
                         revision = revision + 1, updated_at_ms = ?10 WHERE id = ?11",
                        params![
                            new.name,
                            new.description,
                            new.instructions,
                            fields.0,
                            fields.1,
                            fields.2,
                            fields.3,
                            new.model,
                            fields.4,
                            now,
                            id.to_string()
                        ],
                    )?;
                    tx.execute(
                        "UPDATE actors SET name = ?1 WHERE ref = ?2",
                        params![new.name, reference(id)],
                    )?;
                    (id, current.revision + 1)
                }
                None => {
                    let id = AgentId::new();
                    let actor = nexus_protocol::ActorId::new().to_string();
                    tx.execute(
                        "INSERT INTO actors (id, kind, ref, name, created_at_ms) VALUES (?1, 'Agent', ?2, ?3, ?4)",
                        params![actor, reference(id), new.name, now],
                    )?;
                    tx.execute(
                        "INSERT INTO agents (id, name, description, instructions, tools_off_json, skills_json,
                         mcp_servers_json, connection_id, model, project_id, actor_id, revision, created_at_ms,
                         updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?12)",
                        params![
                            id.to_string(),
                            new.name,
                            new.description,
                            new.instructions,
                            fields.0,
                            fields.1,
                            fields.2,
                            fields.3,
                            new.model,
                            fields.4,
                            actor,
                            now
                        ],
                    )?;
                    (id, 1)
                }
            };
            insert_event(
                &tx,
                "AgentSaved",
                &id.to_string(),
                revision,
                json!({"name": new.name}),
                now,
            )?;
            let row = load(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Archives the agent: no new task starts with it; what it did keeps
    /// its name, and its tasks stay with it.
    pub async fn archive_agent(&self, id: AgentId, now: i64) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load(&tx, id)?;
            if current.archived_at_ms.is_some() {
                return Ok(());
            }
            tx.execute(
                "UPDATE agents SET archived_at_ms = ?1, revision = revision + 1, updated_at_ms = ?1 WHERE id = ?2",
                params![now, id.to_string()],
            )?;
            tx.execute(
                "UPDATE actors SET disabled_at_ms = ?1 WHERE ref = ?2 AND disabled_at_ms IS NULL",
                params![now, reference(id)],
            )?;
            insert_event(
                &tx,
                "AgentArchived",
                &id.to_string(),
                current.revision + 1,
                json!({"name": current.name}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
