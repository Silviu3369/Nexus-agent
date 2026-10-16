//! Projects and memory (`schema::V6`). A project is the unit of work: its
//! conversations, and the facts remembered for it. A memory fact is either
//! about the user (Personal, visible in every project) or about one project.
//! The agent can only *propose* a fact; the user decides. Every write is one
//! transaction with its event; events never carry a fact's content.

use nexus_core::{MemoryScope, MemorySource, MemoryStatus};
use nexus_protocol::{ConversationId, MemoryId, MessageId, ProjectId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_state, parse_uuid};
use crate::{StorageError, Store};

const ACTIVE_PROJECT: &str = "active_project";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: ProjectId,
    pub name: String,
    pub description: String,
    pub revision: i64,
    pub created_at_ms: i64,
    pub conversations: i64,
    pub memory_items: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRow {
    pub id: MemoryId,
    pub scope: MemoryScope,
    pub project_id: Option<ProjectId>,
    pub project_name: Option<String>,
    pub content: String,
    pub status: MemoryStatus,
    pub source: MemorySource,
    pub edited_by_user: bool,
    pub conversation_id: Option<ConversationId>,
    pub conversation_title: Option<String>,
    pub message_id: Option<MessageId>,
    /// Hosts the agent had read in that reply before proposing.
    pub influences: Vec<String>,
    pub revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub decided_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewMemory {
    pub scope: MemoryScope,
    /// Required for a Project fact, absent for a Personal one.
    pub project_id: Option<ProjectId>,
    pub content: String,
    pub source: MemorySource,
    pub conversation_id: Option<ConversationId>,
    pub message_id: Option<MessageId>,
    pub influences: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    /// Words to find (diacritics ignored, prefix match); `None` lists all.
    pub text: Option<String>,
    pub scope: Option<MemoryScope>,
    /// Facts usable in this project: its own and the personal ones.
    pub project_id: Option<ProjectId>,
    pub status: Option<MemoryStatus>,
    pub limit: u32,
}

const MEMORY_COLUMNS: &str = "m.id, m.scope, m.project_id, p.name, m.content, m.status, m.source,
    m.edited_by_user, m.conversation_id, c.title, m.message_id, m.influences_json, m.revision,
    m.created_at_ms, m.updated_at_ms, m.decided_at_ms";
const MEMORY_FROM: &str = "FROM memory_items m
    LEFT JOIN projects p ON p.id = m.project_id
    LEFT JOIN conversations c ON c.id = m.conversation_id";

impl Store {
    pub async fn projects(&self) -> Result<Vec<ProjectRow>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let ids: Vec<String> = {
                let mut stmt = tx.prepare("SELECT id FROM projects ORDER BY created_at_ms, id")?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            ids.iter()
                .map(|id| load_project(&tx, ProjectId(parse_uuid(id)?)))
                .collect()
        })
        .await
    }

    pub async fn project(&self, id: ProjectId) -> Result<ProjectRow, StorageError> {
        self.call(move |conn| load_project(&conn.transaction()?, id))
            .await
    }

    pub async fn create_project(
        &self,
        name: String,
        description: String,
        now: i64,
    ) -> Result<ProjectRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let id = ProjectId::new();
            tx.execute(
                "INSERT INTO projects (id, name, description, revision, created_at_ms) VALUES (?1, ?2, ?3, 1, ?4)",
                params![id.to_string(), name, description, now],
            )?;
            insert_event(&tx, "ProjectCreated", &id.to_string(), 1, json!({}), now)?;
            let row = load_project(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    pub async fn update_project(
        &self,
        id: ProjectId,
        name: String,
        description: String,
        now: i64,
    ) -> Result<ProjectRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE projects SET name = ?1, description = ?2, revision = revision + 1 WHERE id = ?3",
                params![name, description, id.to_string()],
            )?;
            if changed != 1 {
                return Err(StorageError::NotFound("project"));
            }
            let row = load_project(&tx, id)?;
            insert_event(&tx, "ProjectUpdated", &id.to_string(), row.revision, json!({}), now)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// The project new tasks go to: the one the user chose, else the first
    /// (default) project.
    pub async fn active_project(&self) -> Result<ProjectId, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let chosen: Option<String> = tx
                .query_row(
                    "SELECT json_extract(s.value_json, '$') FROM settings s
                     JOIN projects p ON p.id = json_extract(s.value_json, '$') WHERE s.key = ?1",
                    [ACTIVE_PROJECT],
                    |r| r.get(0),
                )
                .optional()?;
            let id = match chosen {
                Some(id) => id,
                None => tx
                    .query_row(
                        "SELECT id FROM projects ORDER BY created_at_ms, id LIMIT 1",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or(StorageError::NotFound("project"))?,
            };
            Ok(ProjectId(parse_uuid(&id)?))
        })
        .await
    }

    pub async fn set_active_project(&self, id: ProjectId, now: i64) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            load_project(&tx, id)?;
            tx.execute(
                "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json",
                params![ACTIVE_PROJECT, json!(id.to_string()).to_string()],
            )?;
            insert_event(
                &tx,
                "ActiveProjectChanged",
                &id.to_string(),
                0,
                json!({}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// The project a conversation belongs to.
    pub async fn conversation_project(
        &self,
        conversation_id: ConversationId,
    ) -> Result<ProjectId, StorageError> {
        self.call(move |conn| {
            let id: Option<String> = conn
                .query_row(
                    "SELECT project_id FROM conversations WHERE id = ?1",
                    [conversation_id.to_string()],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(StorageError::NotFound("conversation"))?;
            let id =
                id.ok_or_else(|| StorageError::Corrupt("conversation without project".into()))?;
            Ok(ProjectId(parse_uuid(&id)?))
        })
        .await
    }

    /// A fact the user wrote (Active at once) or the agent proposed
    /// (Proposed, waiting for the user).
    pub async fn add_memory(&self, new: NewMemory, now: i64) -> Result<MemoryRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            if let Some(project) = new.project_id {
                load_project(&tx, project)?;
            }
            let id = MemoryId::new();
            let status = match new.source {
                MemorySource::User => MemoryStatus::Active,
                MemorySource::Agent => MemoryStatus::Proposed,
            };
            tx.execute(
                "INSERT INTO memory_items (id, scope, project_id, content, status, source, conversation_id,
                 message_id, influences_json, revision, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?10)",
                params![
                    id.to_string(),
                    new.scope.as_str(),
                    new.project_id.map(|p| p.to_string()),
                    new.content,
                    status.as_str(),
                    new.source.as_str(),
                    new.conversation_id.map(|c| c.to_string()),
                    new.message_id.map(|m| m.to_string()),
                    json!(new.influences).to_string(),
                    now
                ],
            )?;
            let kind = match new.source {
                MemorySource::User => "MemoryAdded",
                MemorySource::Agent => "MemoryProposed",
            };
            insert_event(
                &tx,
                kind,
                &id.to_string(),
                1,
                json!({"scope": new.scope.as_str(), "project": new.project_id.map(|p| p.to_string())}),
                now,
            )?;
            let row = load_memory(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Corrects a fact's wording; refused if it changed since the caller
    /// read it (`expected_revision`).
    pub async fn edit_memory(
        &self,
        id: MemoryId,
        expected_revision: i64,
        content: String,
        now: i64,
    ) -> Result<MemoryRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_memory(&tx, id)?;
            if current.revision != expected_revision {
                return Err(StorageError::RevisionConflict("memory"));
            }
            tx.execute(
                "UPDATE memory_items SET content = ?1, edited_by_user = (source = 'Agent'),
                 revision = revision + 1, updated_at_ms = ?2 WHERE id = ?3",
                params![content, now, id.to_string()],
            )?;
            insert_event(
                &tx,
                "MemoryEdited",
                &id.to_string(),
                current.revision + 1,
                json!({}),
                now,
            )?;
            let row = load_memory(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Approves or rejects a proposal, exactly once, as the user saw it.
    pub async fn decide_memory(
        &self,
        id: MemoryId,
        expected_revision: i64,
        approve: bool,
        now: i64,
    ) -> Result<MemoryRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_memory(&tx, id)?;
            if current.revision != expected_revision {
                return Err(StorageError::RevisionConflict("memory"));
            }
            let next = if approve {
                MemoryStatus::Active
            } else {
                MemoryStatus::Rejected
            };
            if !current.status.can_transition_to(next) {
                return Err(StorageError::IllegalTransition(
                    nexus_core::IllegalTransition {
                        from: current.status.as_str(),
                        to: next.as_str(),
                    },
                ));
            }
            tx.execute(
                "UPDATE memory_items SET status = ?1, revision = revision + 1, updated_at_ms = ?2,
                 decided_at_ms = ?2 WHERE id = ?3 AND status = 'Proposed'",
                params![next.as_str(), now, id.to_string()],
            )?;
            insert_event(
                &tx,
                "MemoryDecided",
                &id.to_string(),
                current.revision + 1,
                json!({"status": next.as_str()}),
                now,
            )?;
            let row = load_memory(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Removes the fact for good (and from the search index).
    pub async fn delete_memory(&self, id: MemoryId, now: i64) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_memory(&tx, id)?;
            tx.execute("DELETE FROM memory_items WHERE id = ?1", [id.to_string()])?;
            insert_event(
                &tx,
                "MemoryDeleted",
                &id.to_string(),
                current.revision + 1,
                json!({}),
                now,
            )?;
            tx.commit()?;
            crate::chat::shrink_journal(conn);
            Ok(())
        })
        .await
    }

    pub async fn memory(&self, id: MemoryId) -> Result<MemoryRow, StorageError> {
        self.call(move |conn| load_memory(&conn.transaction()?, id))
            .await
    }

    pub async fn search_memory(
        &self,
        filter: MemoryFilter,
    ) -> Result<Vec<MemoryRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let mut sql = format!("SELECT {MEMORY_COLUMNS} {MEMORY_FROM}");
            let mut clauses: Vec<String> = Vec::new();
            let mut args: Vec<String> = Vec::new();
            let fts = filter.text.as_deref().and_then(fts_query);
            if let Some(query) = &fts {
                sql.push_str(" JOIN memory_fts f ON f.rowid = m.rowid");
                args.push(query.clone());
                clauses.push(format!("memory_fts MATCH ?{}", args.len()));
            }
            if let Some(scope) = filter.scope {
                args.push(scope.as_str().to_string());
                clauses.push(format!("m.scope = ?{}", args.len()));
            }
            if let Some(project) = filter.project_id {
                args.push(project.to_string());
                clauses.push(format!(
                    "(m.scope = 'Personal' OR m.project_id = ?{})",
                    args.len()
                ));
            }
            if let Some(status) = filter.status {
                args.push(status.as_str().to_string());
                clauses.push(format!("m.status = ?{}", args.len()));
            }
            if !clauses.is_empty() {
                sql.push_str(" WHERE ");
                sql.push_str(&clauses.join(" AND "));
            }
            sql.push_str(if fts.is_some() {
                " ORDER BY bm25(memory_fts), m.updated_at_ms DESC"
            } else {
                " ORDER BY m.updated_at_ms DESC, m.id"
            });
            args.push(filter.limit.clamp(1, 500).to_string());
            sql.push_str(&format!(" LIMIT ?{}", args.len()));
            let mut stmt = tx.prepare(&sql)?;
            let raws = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), raw_memory)?
                .collect::<Result<Vec<_>, _>>()?;
            raws.into_iter().map(memory_from_raw).collect()
        })
        .await
    }

    /// The approved facts sent with a request in `project`: the personal
    /// ones and the project's own, newest first, within the given budget.
    pub async fn memory_context(
        &self,
        project: ProjectId,
        max_items: usize,
        max_chars: usize,
    ) -> Result<Vec<MemoryRow>, StorageError> {
        let all = self
            .search_memory(MemoryFilter {
                project_id: Some(project),
                status: Some(MemoryStatus::Active),
                limit: 500,
                ..MemoryFilter::default()
            })
            .await?;
        let mut used = 0usize;
        Ok(all
            .into_iter()
            .take(max_items)
            .take_while(|m| {
                used += m.content.chars().count();
                used <= max_chars
            })
            .collect())
    }
}

/// Turns what the user typed into a safe FTS5 query: every word is quoted
/// (so no FTS syntax can be injected) and matched as a prefix.
fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(12)
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn load_project(tx: &Transaction, id: ProjectId) -> Result<ProjectRow, StorageError> {
    tx.query_row(
        "SELECT p.name, p.description, p.revision, p.created_at_ms,
            (SELECT count(*) FROM conversations c WHERE c.project_id = p.id),
            (SELECT count(*) FROM memory_items m WHERE m.project_id = p.id AND m.status = 'Active')
         FROM projects p WHERE p.id = ?1",
        [id.to_string()],
        |r| {
            Ok(ProjectRow {
                id,
                name: r.get(0)?,
                description: r.get(1)?,
                revision: r.get(2)?,
                created_at_ms: r.get(3)?,
                conversations: r.get(4)?,
                memory_items: r.get(5)?,
            })
        },
    )
    .optional()?
    .ok_or(StorageError::NotFound("project"))
}

type RawMemory = (
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
);

fn raw_memory(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawMemory> {
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
        r.get(14)?,
        r.get(15)?,
    ))
}

fn memory_from_raw(raw: RawMemory) -> Result<MemoryRow, StorageError> {
    let (
        id,
        scope,
        project_id,
        project_name,
        content,
        status,
        source,
        edited,
        conversation_id,
        conversation_title,
        message_id,
        influences,
        revision,
        created_at_ms,
        updated_at_ms,
        decided_at_ms,
    ) = raw;
    Ok(MemoryRow {
        id: MemoryId(parse_uuid(&id)?),
        scope: parse_state(&scope)?,
        project_id: project_id
            .as_deref()
            .map(parse_uuid)
            .transpose()?
            .map(ProjectId),
        project_name,
        content,
        status: parse_state(&status)?,
        source: parse_state(&source)?,
        edited_by_user: edited != 0,
        conversation_id: conversation_id
            .as_deref()
            .map(parse_uuid)
            .transpose()?
            .map(ConversationId),
        conversation_title,
        message_id: message_id
            .as_deref()
            .map(parse_uuid)
            .transpose()?
            .map(MessageId),
        influences: serde_json::from_str(&influences)
            .map_err(|e| StorageError::Corrupt(format!("memory influences: {e}")))?,
        revision,
        created_at_ms,
        updated_at_ms,
        decided_at_ms,
    })
}

fn load_memory(tx: &Transaction, id: MemoryId) -> Result<MemoryRow, StorageError> {
    let sql = format!("SELECT {MEMORY_COLUMNS} {MEMORY_FROM} WHERE m.id = ?1");
    let raw = tx
        .query_row(&sql, [id.to_string()], raw_memory)
        .optional()?
        .ok_or(StorageError::NotFound("memory"))?;
    memory_from_raw(raw)
}

#[cfg(test)]
mod tests {
    use super::fts_query;

    #[test]
    fn user_text_becomes_quoted_prefix_terms() {
        assert_eq!(
            fts_query("nota zilei").as_deref(),
            Some("\"nota\"* \"zilei\"*")
        );
        assert_eq!(
            fts_query("a\" OR x NEAR(").as_deref(),
            Some("\"a\"* \"OR\"* \"x\"* \"NEAR\"*")
        );
        assert_eq!(fts_query("  ,. "), None);
    }
}
