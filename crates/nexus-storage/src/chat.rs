//! M2 persistence: provider connections (without secrets), the versioned
//! persona and conversations. One transaction per operation, with events,
//! like the M1 repository.

use nexus_core::{FailureCause, MessageRole, MessageStatus, RunState, TaskOrigin, ThinkingLevel};
use nexus_protocol::{ConnectionId, ConversationId, MessageId, ProjectId, RunId, TaskId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_state, parse_uuid};
use crate::{StorageError, Store};

const ACTIVE_CONNECTION: &str = "active_connection";
const BACKUP_MODEL: &str = "backup_model";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRow {
    pub id: ConnectionId,
    pub provider: String,
    pub display_name: String,
    pub model: Option<String>,
    pub free_tier: bool,
    /// Opaque name of the vault entry holding the key; never the key.
    pub secret_name: String,
    pub thinking_level: ThinkingLevel,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaRow {
    pub version: i64,
    pub name: String,
    pub tone: String,
    pub style: String,
}

#[derive(Debug, Clone)]
pub struct ConversationRow {
    pub id: ConversationId,
    pub title: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub project_id: Option<ProjectId>,
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub seq: i64,
    pub role: MessageRole,
    pub text: String,
    pub status: MessageStatus,
    pub connection_id: Option<ConnectionId>,
    pub model: Option<String>,
    pub persona_version: Option<i64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub proposed_run_id: Option<RunId>,
    /// The agent's run (turn) this message belongs to (ADR-TASK-001).
    pub run_id: Option<RunId>,
    pub created_at_ms: i64,
}

/// The two rows a user turn creates: the user's message and the model's
/// placeholder that will be streamed into.
#[derive(Debug, Clone)]
pub struct Exchange {
    pub conversation_id: ConversationId,
    pub user_message: MessageRow,
    pub model_message: MessageRow,
    /// False when the idempotency key matched an earlier identical turn.
    pub created: bool,
}

#[derive(Debug, Clone)]
pub struct NewExchange {
    pub conversation_id: Option<ConversationId>,
    /// The project a new conversation joins (`None`: the default, first
    /// project); ignored for a follow-up.
    pub project_id: Option<ProjectId>,
    pub idempotency_key: String,
    pub text: String,
    pub connection_id: ConnectionId,
    pub model: String,
    pub persona_version: i64,
    /// The title of a new conversation (`None`: from the text).
    pub title: Option<String>,
    /// What started a new conversation's task; ignored for a follow-up.
    pub origin: TaskOrigin,
    /// The pack or flow it belongs to (an intake's register), if any.
    pub pack: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Settlement {
    pub text: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub proposed_run_id: Option<RunId>,
    /// The reply stopped on a proposal waiting for the user: what the model
    /// needs to continue once the user decides (kept only until then).
    pub pause: Option<Pause>,
}

#[derive(Debug, Clone)]
pub struct Pause {
    pub run_id: RunId,
    /// Opaque to the store; the host's own versioned format.
    pub state_json: String,
}

/// A task continuing after the user decided on what its reply proposed: a
/// new model message in the same conversation, after the reply that paused.
#[derive(Debug, Clone)]
pub struct Continuation {
    pub conversation_id: ConversationId,
    /// The user's request the task is answering.
    pub request: MessageRow,
    /// The reply that stopped on the proposal.
    pub paused_reply: MessageRow,
    /// The new Streaming message the continuation is written into.
    pub model_message: MessageRow,
    pub run_id: RunId,
    pub state_json: String,
}

pub const DEFAULT_PERSONA_NAME: &str = "Nexus";
pub const DEFAULT_PERSONA_TONE: &str = "calm, direct și prietenos";
pub const DEFAULT_PERSONA_STYLE: &str = "răspunsuri clare și concise; întrebi când ceva e ambiguu";

impl Store {
    /// Creates or updates a connection. The caller stores the key in the
    /// vault under `secret_name` *before* calling this. `provider` is set
    /// on creation only; a connection never changes provider.
    pub async fn save_connection(
        &self,
        id: Option<ConnectionId>,
        provider: &'static str,
        display_name: String,
        free_tier: bool,
        secret_name: String,
        now: i64,
    ) -> Result<ConnectionRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let id = match id {
                Some(id) => {
                    let changed = tx.execute(
                        "UPDATE provider_connections SET display_name = ?1, free_tier = ?2, revision = revision + 1,
                         updated_at_ms = ?3 WHERE id = ?4",
                        params![display_name, free_tier, now, id.to_string()],
                    )?;
                    if changed != 1 {
                        return Err(StorageError::NotFound("connection"));
                    }
                    id
                }
                None => {
                    let id = ConnectionId::new();
                    tx.execute(
                        "INSERT INTO provider_connections (id, provider, display_name, free_tier, secret_name,
                         revision, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                        params![id.to_string(), provider, display_name, free_tier, secret_name, now],
                    )?;
                    id
                }
            };
            let row = load_connection(&tx, id)?;
            insert_event(
                &tx,
                "ConnectionSaved",
                &id.to_string(),
                row.revision,
                json!({"provider": row.provider, "free_tier": row.free_tier}),
                now,
            )?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    pub async fn connections(&self) -> Result<Vec<ConnectionRow>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let ids: Vec<String> = {
                let mut stmt =
                    tx.prepare("SELECT id FROM provider_connections ORDER BY created_at_ms, id")?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            ids.iter()
                .map(|id| load_connection(&tx, ConnectionId(parse_uuid(id)?)))
                .collect()
        })
        .await
    }

    pub async fn connection(&self, id: ConnectionId) -> Result<ConnectionRow, StorageError> {
        self.call(move |conn| load_connection(&conn.transaction()?, id))
            .await
    }

    pub async fn set_connection_model(
        &self,
        id: ConnectionId,
        model: String,
        now: i64,
    ) -> Result<ConnectionRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE provider_connections SET model = ?1, revision = revision + 1, updated_at_ms = ?2 WHERE id = ?3",
                params![model, now, id.to_string()],
            )?;
            if changed != 1 {
                return Err(StorageError::NotFound("connection"));
            }
            let row = load_connection(&tx, id)?;
            insert_event(&tx, "ConnectionModelSelected", &id.to_string(), row.revision, json!({"model": row.model}), now)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    pub async fn set_thinking_level(
        &self,
        id: ConnectionId,
        level: ThinkingLevel,
        now: i64,
    ) -> Result<ConnectionRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE provider_connections SET thinking_level = ?1, revision = revision + 1, updated_at_ms = ?2 WHERE id = ?3",
                params![level.as_str(), now, id.to_string()],
            )?;
            if changed != 1 {
                return Err(StorageError::NotFound("connection"));
            }
            let row = load_connection(&tx, id)?;
            insert_event(
                &tx,
                "ConnectionThinkingLevelSet",
                &id.to_string(),
                row.revision,
                json!({"thinking_level": level.as_str()}),
                now,
            )?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Removes the connection; returns its secret name so the caller can
    /// delete the vault entry. Past messages keep their text, losing the link.
    pub async fn delete_connection(
        &self,
        id: ConnectionId,
        now: i64,
    ) -> Result<String, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let row = load_connection(&tx, id)?;
            tx.execute(
                "UPDATE messages SET connection_id = NULL WHERE connection_id = ?1",
                [id.to_string()],
            )?;
            tx.execute(
                "DELETE FROM provider_connections WHERE id = ?1",
                [id.to_string()],
            )?;
            tx.execute(
                "DELETE FROM settings WHERE key = ?1 AND json_extract(value_json, '$') = ?2",
                params![ACTIVE_CONNECTION, id.to_string()],
            )?;
            // A backup on a removed connection would point at nothing.
            tx.execute(
                "DELETE FROM settings WHERE key = ?1 AND json_extract(value_json, '$.connection_id') = ?2",
                params![BACKUP_MODEL, id.to_string()],
            )?;
            insert_event(
                &tx,
                "ConnectionRemoved",
                &id.to_string(),
                row.revision + 1,
                json!({}),
                now,
            )?;
            tx.commit()?;
            Ok(row.secret_name)
        })
        .await
    }

    pub async fn set_active_connection(
        &self,
        id: ConnectionId,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            load_connection(&tx, id)?;
            tx.execute(
                "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json",
                params![ACTIVE_CONNECTION, json!(id.to_string()).to_string()],
            )?;
            insert_event(
                &tx,
                "ActiveConnectionChanged",
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

    /// The connection the user chose explicitly; there is no automatic
    /// fallback to any other one (`CONTRACTE_NEXUS_RO.md` §11).
    pub async fn active_connection(&self) -> Result<Option<ConnectionRow>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let value: Option<String> = tx
                .query_row(
                    "SELECT json_extract(value_json, '$') FROM settings WHERE key = ?1",
                    [ACTIVE_CONNECTION],
                    |r| r.get(0),
                )
                .optional()?;
            match value {
                Some(id) => Ok(Some(load_connection(&tx, ConnectionId(parse_uuid(&id)?))?)),
                None => Ok(None),
            }
        })
        .await
    }

    /// The model to use only when the active one is overloaded, chosen by
    /// the user; `None` clears it. The connection must exist.
    pub async fn set_backup_model(
        &self,
        backup: Option<(ConnectionId, String)>,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let entity = match &backup {
                Some((id, model)) => {
                    load_connection(&tx, *id)?;
                    tx.execute(
                        "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
                         ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json",
                        params![
                            BACKUP_MODEL,
                            json!({"connection_id": id.to_string(), "model": model}).to_string()
                        ],
                    )?;
                    id.to_string()
                }
                None => {
                    tx.execute("DELETE FROM settings WHERE key = ?1", [BACKUP_MODEL])?;
                    BACKUP_MODEL.to_string()
                }
            };
            insert_event(
                &tx,
                "BackupModelSet",
                &entity,
                0,
                json!({"set": backup.is_some()}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn backup_model(&self) -> Result<Option<(ConnectionRow, String)>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let value: Option<(String, String)> = tx
                .query_row(
                    "SELECT json_extract(value_json, '$.connection_id'), json_extract(value_json, '$.model')
                     FROM settings WHERE key = ?1",
                    [BACKUP_MODEL],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match value {
                Some((id, model)) => Ok(Some((
                    load_connection(&tx, ConnectionId(parse_uuid(&id)?))?,
                    model,
                ))),
                None => Ok(None),
            }
        })
        .await
    }

    /// The current persona, creating version 1 with the defaults if none.
    pub async fn persona(&self, now: i64) -> Result<PersonaRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            if let Some(row) = current_persona(&tx)? {
                return Ok(row);
            }
            tx.execute(
                "INSERT INTO personas (version, name, tone, style, created_at_ms) VALUES (1, ?1, ?2, ?3, ?4)",
                params![DEFAULT_PERSONA_NAME, DEFAULT_PERSONA_TONE, DEFAULT_PERSONA_STYLE, now],
            )?;
            insert_event(&tx, "PersonaSaved", "persona", 1, json!({"version": 1}), now)?;
            let row = current_persona(&tx)?.expect("just inserted");
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Saves a new persona version; earlier versions stay so every message
    /// keeps a record of the one it was generated with.
    pub async fn save_persona(
        &self,
        name: String,
        tone: String,
        style: String,
        now: i64,
    ) -> Result<PersonaRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let next: i64 = tx.query_row("SELECT coalesce(max(version), 0) + 1 FROM personas", [], |r| r.get(0))?;
            tx.execute(
                "INSERT INTO personas (version, name, tone, style, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![next, name, tone, style, now],
            )?;
            insert_event(&tx, "PersonaSaved", "persona", next, json!({"version": next}), now)?;
            let row = current_persona(&tx)?.expect("just inserted");
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    pub async fn persona_version(&self, version: i64) -> Result<PersonaRow, StorageError> {
        self.call(move |conn| {
            conn.query_row(
                "SELECT version, name, tone, style FROM personas WHERE version = ?1",
                [version],
                |r| {
                    Ok(PersonaRow {
                        version: r.get(0)?,
                        name: r.get(1)?,
                        tone: r.get(2)?,
                        style: r.get(3)?,
                    })
                },
            )
            .optional()?
            .ok_or(StorageError::NotFound("persona"))
        })
        .await
    }

    /// Records a user turn and the model's Streaming placeholder in one
    /// transaction. The same idempotency key with the same text returns the
    /// original pair; with different text it is a conflict.
    pub async fn append_exchange(
        &self,
        new: NewExchange,
        now: i64,
    ) -> Result<Exchange, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT id, text FROM messages WHERE idempotency_key = ?1",
                    [&new.idempotency_key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((user_id, text)) = existing {
                if text != new.text {
                    return Err(StorageError::IdempotencyConflict(new.idempotency_key));
                }
                let user_message = load_message(&tx, MessageId(parse_uuid(&user_id)?))?;
                let model_message = load_message_at(&tx, user_message.conversation_id, user_message.seq + 1)?
                    .ok_or(StorageError::Corrupt("user turn without a model message".to_string()))?;
                return Ok(Exchange { conversation_id: user_message.conversation_id, user_message, model_message, created: false });
            }

            let conversation_id = match new.conversation_id {
                Some(id) => {
                    let found: Option<i64> = tx
                        .query_row("SELECT 1 FROM conversations WHERE id = ?1", [id.to_string()], |r| r.get(0))
                        .optional()?;
                    found.ok_or(StorageError::NotFound("conversation"))?;
                    tx.execute("UPDATE conversations SET updated_at_ms = ?1 WHERE id = ?2", params![now, id.to_string()])?;
                    // The user moved on: a reply still waiting on a decision
                    // in this task no longer continues after it, and its
                    // turn ends there.
                    tx.execute(
                        "UPDATE runs SET state = 'Cancelled', revision = revision + 1, updated_at_ms = ?1
                         WHERE kind = 'Agent' AND state = 'AwaitingApproval' AND id IN (
                             SELECT m.run_id FROM paused_turns p JOIN messages m ON m.id = p.message_id
                             WHERE p.conversation_id = ?2)",
                        params![now, id.to_string()],
                    )?;
                    tx.execute("DELETE FROM paused_turns WHERE conversation_id = ?1", [id.to_string()])?;
                    id
                }
                None => {
                    let id = ConversationId::new();
                    let project = match new.project_id {
                        Some(project) => project.to_string(),
                        None => tx
                            .query_row("SELECT id FROM projects ORDER BY created_at_ms, id LIMIT 1", [], |r| r.get(0))
                            .optional()?
                            .ok_or(StorageError::NotFound("project"))?,
                    };
                    tx.execute(
                        "INSERT INTO conversations (id, title, created_at_ms, updated_at_ms, project_id)
                         VALUES (?1, ?2, ?3, ?3, ?4)",
                        params![id.to_string(), new.title.clone().unwrap_or_else(|| title_from(&new.text)), now, project],
                    )?;
                    id
                }
            };
            let seq: i64 = tx.query_row(
                "SELECT coalesce(max(seq), 0) + 1 FROM messages WHERE conversation_id = ?1",
                [conversation_id.to_string()],
                |r| r.get(0),
            )?;
            let task = agent_task(&tx, conversation_id, new.origin, new.pack.as_deref(), now)?;
            let run = RunId::new();
            tx.execute(
                "INSERT INTO runs (id, task_id, state, revision, created_at_ms, updated_at_ms, kind)
                 VALUES (?1, ?2, 'Running', 1, ?3, ?3, 'Agent')",
                params![run.to_string(), task.to_string(), now],
            )?;
            let (user_id, model_id) = (MessageId::new(), MessageId::new());
            tx.execute(
                "INSERT INTO messages (id, conversation_id, seq, role, text, status, idempotency_key, persona_version,
                 run_id, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, 'User', ?4, 'Complete', ?5, ?6, ?7, ?8, ?8)",
                params![
                    user_id.to_string(),
                    conversation_id.to_string(),
                    seq,
                    new.text,
                    new.idempotency_key,
                    new.persona_version,
                    run.to_string(),
                    now
                ],
            )?;
            tx.execute(
                "INSERT INTO messages (id, conversation_id, seq, role, text, status, connection_id, model, persona_version,
                 run_id, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, 'Model', '', 'Streaming', ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    model_id.to_string(),
                    conversation_id.to_string(),
                    seq + 1,
                    new.connection_id.to_string(),
                    new.model,
                    new.persona_version,
                    run.to_string(),
                    now
                ],
            )?;
            insert_event(
                &tx,
                "MessageSent",
                &conversation_id.to_string(),
                seq,
                json!({"user_message": user_id.to_string(), "model_message": model_id.to_string(), "model": new.model}),
                now,
            )?;
            let exchange = Exchange {
                conversation_id,
                user_message: load_message(&tx, user_id)?,
                model_message: load_message(&tx, model_id)?,
                created: true,
            };
            tx.commit()?;
            Ok(exchange)
        })
        .await
    }

    /// Settles a Streaming model message exactly once; a second settlement
    /// (e.g. a cancel racing a completion) is refused, not applied twice.
    pub async fn settle_message(
        &self,
        id: MessageId,
        status: MessageStatus,
        settlement: Settlement,
        now: i64,
    ) -> Result<MessageRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_message(&tx, id)?;
            if !current.status.can_transition_to(status) {
                return Err(StorageError::IllegalTransition(nexus_core::IllegalTransition {
                    from: current.status.as_str(),
                    to: status.as_str(),
                }));
            }
            let settled = tx.execute(
                "UPDATE messages SET text = ?1, status = ?2, error_code = ?3, error_message = ?4, prompt_tokens = ?5,
                 output_tokens = ?6, proposed_run_id = ?7, updated_at_ms = ?8 WHERE id = ?9 AND status = 'Streaming'",
                params![
                    settlement.text,
                    status.as_str(),
                    settlement.error_code,
                    settlement.error_message,
                    settlement.prompt_tokens,
                    settlement.output_tokens,
                    settlement.proposed_run_id.map(|r| r.to_string()),
                    now,
                    id.to_string()
                ],
            )?;
            if settled == 1
                && let Some(run) = current.run_id
            {
                settle_turn(&tx, run, status, &settlement, now)?;
            }
            if settled == 1
                && status == MessageStatus::Complete
                && let Some(pause) = &settlement.pause
            {
                tx.execute(
                    "INSERT INTO paused_turns (message_id, conversation_id, run_id, state_json, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        id.to_string(),
                        current.conversation_id.to_string(),
                        pause.run_id.to_string(),
                        pause.state_json,
                        now
                    ],
                )?;
            }
            insert_event(
                &tx,
                "MessageSettled",
                &current.conversation_id.to_string(),
                current.seq,
                json!({"message": id.to_string(), "status": status.as_str(), "error_code": settlement.error_code}),
                now,
            )?;
            let row = load_message(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Replies waiting on a decision the user has now made (or that ended
    /// without one: expired, failed, cancelled, or left for review).
    pub async fn paused_ready(&self) -> Result<Vec<MessageId>, StorageError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.message_id FROM paused_turns p JOIN runs r ON r.id = p.run_id
                 WHERE r.state IN ('Succeeded', 'Failed', 'Cancelled', 'Denied', 'NeedsReview')
                 ORDER BY p.created_at_ms, p.message_id",
            )?;
            let ids: Vec<String> = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            ids.iter()
                .map(|id| Ok(MessageId(parse_uuid(id)?)))
                .collect()
        })
        .await
    }

    /// Starts the continuation of a reply that waited on the user: once,
    /// in one transaction (the waiting state is consumed with the new
    /// message, so a crash or a second caller cannot continue it twice).
    /// `None` when there is nothing to continue now: the decision is still
    /// pending, another reply is being written (tried again later), or the
    /// user has sent a new request since (then it is dropped).
    pub async fn begin_continuation(
        &self,
        reply: MessageId,
        now: i64,
    ) -> Result<Option<Continuation>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let paused: Option<(String, String, String)> = tx
                .query_row(
                    "SELECT p.run_id, p.state_json, r.state FROM paused_turns p JOIN runs r ON r.id = p.run_id
                     WHERE p.message_id = ?1",
                    [reply.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((run_id, state_json, run_state)) = paused else {
                return Ok(None);
            };
            let run_state: RunState = parse_state(&run_state)?;
            if !(run_state.is_terminal() || run_state == RunState::NeedsReview) {
                return Ok(None);
            }
            let paused_reply = load_message(&tx, reply)?;
            let conversation = paused_reply.conversation_id;
            let later_request: i64 = tx.query_row(
                "SELECT count(*) FROM messages WHERE conversation_id = ?1 AND seq > ?2 AND role = 'User'",
                params![conversation.to_string(), paused_reply.seq],
                |r| r.get(0),
            )?;
            let request_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM messages WHERE conversation_id = ?1 AND seq < ?2 AND role = 'User'
                     ORDER BY seq DESC LIMIT 1",
                    params![conversation.to_string(), paused_reply.seq],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(request_id) = request_id.filter(|_| later_request == 0) else {
                if let Some(turn) = paused_reply.run_id {
                    tx.execute(
                        "UPDATE runs SET state = 'Cancelled', revision = revision + 1, updated_at_ms = ?1
                         WHERE id = ?2 AND kind = 'Agent' AND state = 'AwaitingApproval'",
                        params![now, turn.to_string()],
                    )?;
                }
                tx.execute("DELETE FROM paused_turns WHERE message_id = ?1", [reply.to_string()])?;
                tx.commit()?;
                return Ok(None);
            };
            let writing: i64 = tx.query_row(
                "SELECT count(*) FROM messages WHERE conversation_id = ?1 AND status = 'Streaming'",
                [conversation.to_string()],
                |r| r.get(0),
            )?;
            if writing > 0 {
                return Ok(None);
            }
            let seq: i64 = tx.query_row(
                "SELECT coalesce(max(seq), 0) + 1 FROM messages WHERE conversation_id = ?1",
                [conversation.to_string()],
                |r| r.get(0),
            )?;
            let id = MessageId::new();
            tx.execute(
                "INSERT INTO messages (id, conversation_id, seq, role, text, status, idempotency_key, connection_id, model,
                 persona_version, run_id, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, 'Model', '', 'Streaming', ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    id.to_string(),
                    conversation.to_string(),
                    seq,
                    format!("continue:{reply}"),
                    paused_reply.connection_id.map(|c| c.to_string()),
                    paused_reply.model,
                    paused_reply.persona_version,
                    paused_reply.run_id.map(|r| r.to_string()),
                    now
                ],
            )?;
            if let Some(turn) = paused_reply.run_id {
                tx.execute(
                    "UPDATE runs SET state = 'Running', revision = revision + 1, updated_at_ms = ?1
                     WHERE id = ?2 AND kind = 'Agent' AND state = 'AwaitingApproval'",
                    params![now, turn.to_string()],
                )?;
            }
            tx.execute("DELETE FROM paused_turns WHERE message_id = ?1", [reply.to_string()])?;
            tx.execute(
                "UPDATE conversations SET updated_at_ms = ?1 WHERE id = ?2",
                params![now, conversation.to_string()],
            )?;
            insert_event(
                &tx,
                "MessageContinued",
                &conversation.to_string(),
                seq,
                json!({"after": reply.to_string(), "model_message": id.to_string(), "run": run_id}),
                now,
            )?;
            let continuation = Continuation {
                conversation_id: conversation,
                request: load_message(&tx, MessageId(parse_uuid(&request_id)?))?,
                paused_reply,
                model_message: load_message(&tx, id)?,
                run_id: RunId(parse_uuid(&run_id)?),
                state_json,
            };
            tx.commit()?;
            Ok(Some(continuation))
        })
        .await
    }

    /// Startup: a generation cannot survive a host restart; mark any
    /// Streaming message as interrupted rather than leave it hanging.
    pub async fn fail_interrupted_messages(&self, now: i64) -> Result<usize, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let n = tx.execute(
                "UPDATE messages SET status = 'Failed', error_code = 'Interrupted',
                 error_message = 'the host stopped during the reply', updated_at_ms = ?1 WHERE status = 'Streaming'",
                [now],
            )?;
            if n > 0 {
                insert_event(&tx, "MessagesInterrupted", "messages", 0, json!({"count": n}), now)?;
            }
            tx.execute(
                "UPDATE runs SET state = 'Failed', error_code = 'Interrupted',
                 error_message = 'the host stopped during the reply', failure_cause = 'HostInterrupted',
                 revision = revision + 1, updated_at_ms = ?1 WHERE kind = 'Agent' AND state = 'Running'",
                [now],
            )?;
            tx.commit()?;
            Ok(n)
        })
        .await
    }

    pub async fn conversations(&self, limit: u32) -> Result<Vec<ConversationRow>, StorageError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, title, created_at_ms, updated_at_ms, project_id FROM conversations
                 ORDER BY updated_at_ms DESC, id LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([limit], raw_conversation)?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter().map(conversation_from_raw).collect()
        })
        .await
    }

    /// The last `limit` messages of a conversation, oldest first.
    pub async fn messages(
        &self,
        conversation_id: ConversationId,
        limit: u32,
    ) -> Result<(ConversationRow, Vec<MessageRow>), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let conversation = tx
                .query_row(
                    "SELECT id, title, created_at_ms, updated_at_ms, project_id FROM conversations WHERE id = ?1",
                    [conversation_id.to_string()],
                    raw_conversation,
                )
                .optional()?
                .ok_or(StorageError::NotFound("conversation"))?;
            let ids: Vec<String> = {
                let mut stmt = tx.prepare(
                    "SELECT id FROM (SELECT id, seq FROM messages WHERE conversation_id = ?1 ORDER BY seq DESC LIMIT ?2)
                     ORDER BY seq",
                )?;
                stmt.query_map(params![conversation_id.to_string(), limit], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            let messages = ids
                .iter()
                .map(|id| load_message(&tx, MessageId(parse_uuid(id)?)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((conversation_from_raw(conversation)?, messages))
        })
        .await
    }

    pub async fn message(&self, id: MessageId) -> Result<MessageRow, StorageError> {
        self.call(move |conn| load_message(&conn.transaction()?, id))
            .await
    }

    /// Deletes a task for good: its messages and the steps of its replies.
    /// A reply still being written must be stopped first. Facts it proposed
    /// and notes it wrote stay (they are managed on their own) and only lose
    /// the link to it.
    pub async fn delete_conversation(
        &self,
        id: ConversationId,
        now: i64,
    ) -> Result<Deleted, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let found: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM conversations WHERE id = ?1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            found.ok_or(StorageError::NotFound("conversation"))?;
            let writing: i64 = tx.query_row(
                "SELECT count(*) FROM messages WHERE conversation_id = ?1 AND status = 'Streaming'",
                [id.to_string()],
                |r| r.get(0),
            )?;
            if writing > 0 {
                return Err(StorageError::ReplyInProgress);
            }
            let ids: Vec<String> = {
                let mut stmt = tx.prepare("SELECT id FROM messages WHERE conversation_id = ?1")?;
                stmt.query_map([id.to_string()], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            let messages = remove_messages(&tx, &ids)?;
            remove_conversation(&tx, id, messages, now)?;
            tx.commit()?;
            shrink_journal(conn);
            Ok(Deleted {
                conversation_id: id,
                messages,
                conversation_deleted: true,
            })
        })
        .await
    }

    /// Deletes one request and the reply to it (`message` names either), so
    /// the model no longer sees them in the task's history. Deleting the
    /// only exchange deletes the task; deleting the first one retitles the
    /// task from the request that is now first, so the title never keeps
    /// deleted text.
    pub async fn delete_exchange(
        &self,
        message: MessageId,
        now: i64,
    ) -> Result<Deleted, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let picked = load_message(&tx, message)?;
            let conversation = picked.conversation_id;
            // The request and every reply to it: the reply, and the
            // continuations after the user's decisions (up to the next request).
            let start: i64 = match picked.role {
                MessageRole::User => picked.seq,
                MessageRole::Model => tx
                    .query_row(
                        "SELECT max(seq) FROM messages WHERE conversation_id = ?1 AND seq < ?2 AND role = 'User'",
                        params![conversation.to_string(), picked.seq],
                        |r| r.get::<_, Option<i64>>(0),
                    )?
                    .unwrap_or(picked.seq),
            };
            let end: Option<i64> = tx.query_row(
                "SELECT min(seq) FROM messages WHERE conversation_id = ?1 AND seq > ?2 AND role = 'User'",
                params![conversation.to_string(), start],
                |r| r.get(0),
            )?;
            let pair: Vec<MessageRow> = {
                let mut stmt = tx.prepare(
                    "SELECT id FROM messages WHERE conversation_id = ?1 AND seq >= ?2 AND seq < ?3 ORDER BY seq",
                )?;
                let ids: Vec<String> = stmt
                    .query_map(
                        params![conversation.to_string(), start, end.unwrap_or(i64::MAX)],
                        |r| r.get(0),
                    )?
                    .collect::<Result<_, _>>()?;
                ids.iter()
                    .map(|id| load_message(&tx, MessageId(parse_uuid(id)?)))
                    .collect::<Result<_, _>>()?
            };
            if pair.iter().any(|m| m.status == MessageStatus::Streaming) {
                return Err(StorageError::ReplyInProgress);
            }
            let first_seq: i64 = tx.query_row(
                "SELECT min(seq) FROM messages WHERE conversation_id = ?1",
                [conversation.to_string()],
                |r| r.get(0),
            )?;
            let ids: Vec<String> = pair.iter().map(|m| m.id.to_string()).collect();
            let messages = remove_messages(&tx, &ids)?;
            let remaining: i64 = tx.query_row(
                "SELECT count(*) FROM messages WHERE conversation_id = ?1",
                [conversation.to_string()],
                |r| r.get(0),
            )?;
            let conversation_deleted = remaining == 0;
            if conversation_deleted {
                remove_conversation(&tx, conversation, messages, now)?;
            } else {
                if pair.iter().any(|m| m.seq == first_seq) {
                    let first: Option<String> = tx
                        .query_row(
                            "SELECT text FROM messages WHERE conversation_id = ?1 AND role = 'User'
                             ORDER BY seq LIMIT 1",
                            [conversation.to_string()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    tx.execute(
                        "UPDATE conversations SET title = ?1 WHERE id = ?2",
                        params![
                            title_from(first.as_deref().unwrap_or("")),
                            conversation.to_string()
                        ],
                    )?;
                }
                insert_event(
                    &tx,
                    "ExchangeDeleted",
                    &conversation.to_string(),
                    0,
                    json!({"messages": messages}),
                    now,
                )?;
            }
            tx.commit()?;
            shrink_journal(conn);
            Ok(Deleted {
                conversation_id: conversation,
                messages,
                conversation_deleted,
            })
        })
        .await
    }
}

/// What a deletion removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deleted {
    pub conversation_id: ConversationId,
    pub messages: usize,
    /// The whole task is gone (deleted, or its last exchange was).
    pub conversation_deleted: bool,
}

/// A task is named after its first request.
fn title_from(text: &str) -> String {
    let title: String = text.trim().chars().take(60).collect();
    if title.is_empty() {
        "Untitled task".to_string()
    } else {
        title
    }
}

/// Removes messages with their steps; facts proposed in them keep their
/// task but lose the message.
fn remove_messages(tx: &Transaction, ids: &[String]) -> Result<usize, StorageError> {
    let mut turns: Vec<String> = Vec::new();
    for id in ids {
        let turn: Option<String> = tx
            .query_row("SELECT run_id FROM messages WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        if let Some(turn) = turn
            && !turns.contains(&turn)
        {
            turns.push(turn);
        }
    }
    for id in ids {
        tx.execute(
            "UPDATE memory_items SET message_id = NULL WHERE message_id = ?1",
            [id],
        )?;
        tx.execute("DELETE FROM agent_steps WHERE message_id = ?1", [id])?;
        tx.execute("DELETE FROM messages WHERE id = ?1", [id])?;
    }
    for turn in turns {
        tx.execute(
            "DELETE FROM runs WHERE id = ?1 AND kind = 'Agent'
             AND NOT EXISTS (SELECT 1 FROM messages WHERE run_id = ?1)",
            [&turn],
        )?;
    }
    Ok(ids.len())
}

fn remove_conversation(
    tx: &Transaction,
    id: ConversationId,
    messages: usize,
    now: i64,
) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE memory_items SET conversation_id = NULL, message_id = NULL WHERE conversation_id = ?1",
        [id.to_string()],
    )?;
    tx.execute(
        "DELETE FROM tasks WHERE conversation_id = ?1
         AND NOT EXISTS (SELECT 1 FROM runs WHERE runs.task_id = tasks.id)",
        [id.to_string()],
    )?;
    tx.execute("DELETE FROM conversations WHERE id = ?1", [id.to_string()])?;
    insert_event(
        tx,
        "ConversationDeleted",
        &id.to_string(),
        0,
        json!({"messages": messages}),
        now,
    )?;
    Ok(())
}

/// After a deletion: copies the journal into the database file and empties
/// it, so deleted text (already overwritten in place, `secure_delete`) does
/// not linger in older journal frames either. Best effort: a reader holding
/// an old snapshot only delays it to the next deletion.
pub(crate) fn shrink_journal(conn: &rusqlite::Connection) {
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        r.get::<_, i64>(0)
    });
}

fn load_connection(tx: &Transaction, id: ConnectionId) -> Result<ConnectionRow, StorageError> {
    tx.query_row(
        "SELECT id, provider, display_name, model, free_tier, secret_name, thinking_level, revision
         FROM provider_connections WHERE id = ?1",
        [id.to_string()],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, i64>(4)?,
                r.get(5)?,
                r.get::<_, String>(6)?,
                r.get(7)?,
            ))
        },
    )
    .optional()?
    .ok_or(StorageError::NotFound("connection"))
    .and_then(
        |(id, provider, display_name, model, free_tier, secret_name, thinking_level, revision)| {
            Ok(ConnectionRow {
                id: ConnectionId(parse_uuid(&id)?),
                provider,
                display_name,
                model,
                free_tier: free_tier != 0,
                secret_name,
                thinking_level: parse_state(&thinking_level)?,
                revision,
            })
        },
    )
}

fn current_persona(tx: &Transaction) -> Result<Option<PersonaRow>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT version, name, tone, style FROM personas ORDER BY version DESC LIMIT 1",
            [],
            |r| {
                Ok(PersonaRow {
                    version: r.get(0)?,
                    name: r.get(1)?,
                    tone: r.get(2)?,
                    style: r.get(3)?,
                })
            },
        )
        .optional()?)
}

const MESSAGE_COLUMNS: &str =
    "id, conversation_id, seq, role, text, status, connection_id, model, persona_version,
    error_code, error_message, prompt_tokens, output_tokens, proposed_run_id, created_at_ms, run_id";

type RawMessage = (
    String,
    String,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    i64,
    Option<String>,
);

fn raw_message(r: &rusqlite::Row) -> rusqlite::Result<RawMessage> {
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

fn into_message(raw: RawMessage) -> Result<MessageRow, StorageError> {
    Ok(MessageRow {
        id: MessageId(parse_uuid(&raw.0)?),
        conversation_id: ConversationId(parse_uuid(&raw.1)?),
        seq: raw.2,
        role: parse_state(&raw.3)?,
        text: raw.4,
        status: parse_state(&raw.5)?,
        connection_id: raw
            .6
            .as_deref()
            .map(parse_uuid)
            .transpose()?
            .map(ConnectionId),
        model: raw.7,
        persona_version: raw.8,
        error_code: raw.9,
        error_message: raw.10,
        prompt_tokens: raw.11,
        output_tokens: raw.12,
        proposed_run_id: raw.13.as_deref().map(parse_uuid).transpose()?.map(RunId),
        run_id: raw.15.as_deref().map(parse_uuid).transpose()?.map(RunId),
        created_at_ms: raw.14,
    })
}

fn load_message(tx: &Transaction, id: MessageId) -> Result<MessageRow, StorageError> {
    let raw = tx
        .query_row(
            &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1"),
            [id.to_string()],
            raw_message,
        )
        .optional()?
        .ok_or(StorageError::NotFound("message"))?;
    into_message(raw)
}

fn load_message_at(
    tx: &Transaction,
    conversation_id: ConversationId,
    seq: i64,
) -> Result<Option<MessageRow>, StorageError> {
    tx.query_row(
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE conversation_id = ?1 AND seq = ?2"),
        params![conversation_id.to_string(), seq],
        raw_message,
    )
    .optional()?
    .map(into_message)
    .transpose()
}

type RawConversation = (String, String, i64, i64, Option<String>);

fn raw_conversation(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawConversation> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}

fn conversation_from_raw(raw: RawConversation) -> Result<ConversationRow, StorageError> {
    let (id, title, created_at_ms, updated_at_ms, project_id) = raw;
    Ok(ConversationRow {
        id: ConversationId(parse_uuid(&id)?),
        title,
        created_at_ms,
        updated_at_ms,
        project_id: project_id
            .as_deref()
            .map(parse_uuid)
            .transpose()?
            .map(ProjectId),
    })
}

/// The task of a conversation (ADR-TASK-001), created with its first
/// request. A conversation from before version 15 already has one.
fn agent_task(
    tx: &Transaction,
    conversation: ConversationId,
    origin: TaskOrigin,
    pack: Option<&str>,
    now: i64,
) -> Result<TaskId, StorageError> {
    let found: Option<String> = tx
        .query_row(
            "SELECT id FROM tasks WHERE conversation_id = ?1",
            [conversation.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = found {
        return Ok(TaskId(parse_uuid(&id)?));
    }
    let project: String = tx.query_row(
        "SELECT coalesce(c.project_id, (SELECT id FROM projects ORDER BY created_at_ms, id LIMIT 1))
         FROM conversations c WHERE c.id = ?1",
        [conversation.to_string()],
        |r| r.get(0),
    )?;
    let id = TaskId::new();
    tx.execute(
        "INSERT INTO tasks (id, project_id, idempotency_key, request_json, request_sha256, created_at_ms,
                            origin, conversation_id, pack)
         VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8)",
        params![
            id.to_string(),
            project,
            format!("agent:{conversation}"),
            json!({"conversation_id": conversation.to_string()}).to_string(),
            now,
            origin.as_str(),
            conversation.to_string(),
            pack
        ],
    )?;
    Ok(id)
}

/// A reply settled: its turn waits on the user (a proposal), succeeded,
/// failed with a cause, or was stopped; an effect the reply proposed
/// becomes a child run of the turn.
fn settle_turn(
    tx: &Transaction,
    run: RunId,
    status: MessageStatus,
    settlement: &Settlement,
    now: i64,
) -> Result<(), StorageError> {
    let (state, cause) = match status {
        MessageStatus::Complete if settlement.pause.is_some() => (RunState::AwaitingApproval, None),
        MessageStatus::Complete => (RunState::Succeeded, None),
        MessageStatus::Cancelled => (RunState::Cancelled, None),
        MessageStatus::Failed | MessageStatus::Streaming => (
            RunState::Failed,
            Some(FailureCause::from_error_code(
                settlement.error_code.as_deref().unwrap_or(""),
            )),
        ),
    };
    tx.execute(
        "UPDATE runs SET state = ?1, error_code = ?2, error_message = ?3, failure_cause = ?4,
         revision = revision + 1, updated_at_ms = ?5 WHERE id = ?6 AND kind = 'Agent' AND state = 'Running'",
        params![
            state.as_str(),
            settlement.error_code,
            settlement.error_message,
            cause.map(FailureCause::as_str),
            now,
            run.to_string()
        ],
    )?;
    let proposed = settlement
        .proposed_run_id
        .into_iter()
        .chain(settlement.pause.as_ref().map(|p| p.run_id));
    for effect in proposed {
        tx.execute(
            "UPDATE runs SET parent_run_id = ?1 WHERE id = ?2 AND kind = 'Effect' AND parent_run_id IS NULL",
            params![run.to_string(), effect.to_string()],
        )?;
        tx.execute(
            "UPDATE tasks SET origin = 'Agent'
             WHERE origin = 'Direct' AND id = (SELECT task_id FROM runs WHERE id = ?1)",
            [effect.to_string()],
        )?;
    }
    Ok(())
}
