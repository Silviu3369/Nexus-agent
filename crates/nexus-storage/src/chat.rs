//! M2 persistence: provider connections (without secrets), the versioned
//! persona and conversations. One transaction per operation, with events,
//! like the M1 repository.

use nexus_core::{MessageRole, MessageStatus};
use nexus_protocol::{ConnectionId, ConversationId, MessageId, RunId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_state, parse_uuid};
use crate::{StorageError, Store};

const ACTIVE_CONNECTION: &str = "active_connection";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRow {
    pub id: ConnectionId,
    pub provider: String,
    pub display_name: String,
    pub model: Option<String>,
    pub free_tier: bool,
    /// Opaque name of the vault entry holding the key; never the key.
    pub secret_name: String,
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
    pub idempotency_key: String,
    pub text: String,
    pub connection_id: ConnectionId,
    pub model: String,
    pub persona_version: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Settlement {
    pub text: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub proposed_run_id: Option<RunId>,
}

pub const DEFAULT_PERSONA_NAME: &str = "Nexus";
pub const DEFAULT_PERSONA_TONE: &str = "calm, direct și prietenos";
pub const DEFAULT_PERSONA_STYLE: &str = "răspunsuri clare și concise; întrebi când ceva e ambiguu";

impl Store {
    /// Creates or updates a connection. The caller stores the key in the
    /// vault under `secret_name` *before* calling this.
    pub async fn save_connection(
        &self,
        id: Option<ConnectionId>,
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
                         revision, created_at_ms, updated_at_ms) VALUES (?1, 'Gemini', ?2, ?3, ?4, 1, ?5, ?5)",
                        params![id.to_string(), display_name, free_tier, secret_name, now],
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
                    id
                }
                None => {
                    let id = ConversationId::new();
                    let title: String = new.text.trim().chars().take(60).collect();
                    tx.execute(
                        "INSERT INTO conversations (id, title, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?3)",
                        params![id.to_string(), if title.is_empty() { "Conversație".to_string() } else { title }, now],
                    )?;
                    id
                }
            };
            let seq: i64 = tx.query_row(
                "SELECT coalesce(max(seq), 0) + 1 FROM messages WHERE conversation_id = ?1",
                [conversation_id.to_string()],
                |r| r.get(0),
            )?;
            let (user_id, model_id) = (MessageId::new(), MessageId::new());
            tx.execute(
                "INSERT INTO messages (id, conversation_id, seq, role, text, status, idempotency_key, persona_version,
                 created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, 'User', ?4, 'Complete', ?5, ?6, ?7, ?7)",
                params![user_id.to_string(), conversation_id.to_string(), seq, new.text, new.idempotency_key, new.persona_version, now],
            )?;
            tx.execute(
                "INSERT INTO messages (id, conversation_id, seq, role, text, status, connection_id, model, persona_version,
                 created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, 'Model', '', 'Streaming', ?4, ?5, ?6, ?7, ?7)",
                params![
                    model_id.to_string(),
                    conversation_id.to_string(),
                    seq + 1,
                    new.connection_id.to_string(),
                    new.model,
                    new.persona_version,
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
            tx.execute(
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
            tx.commit()?;
            Ok(n)
        })
        .await
    }

    pub async fn conversations(&self, limit: u32) -> Result<Vec<ConversationRow>, StorageError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, title, created_at_ms, updated_at_ms FROM conversations ORDER BY updated_at_ms DESC, id LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([limit], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|(id, title, created_at_ms, updated_at_ms)| {
                    Ok(ConversationRow { id: ConversationId(parse_uuid(&id)?), title, created_at_ms, updated_at_ms })
                })
                .collect()
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
                    "SELECT id, title, created_at_ms, updated_at_ms FROM conversations WHERE id = ?1",
                    [conversation_id.to_string()],
                    |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
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
            Ok((
                ConversationRow {
                    id: ConversationId(parse_uuid(&conversation.0)?),
                    title: conversation.1,
                    created_at_ms: conversation.2,
                    updated_at_ms: conversation.3,
                },
                messages,
            ))
        })
        .await
    }

    pub async fn message(&self, id: MessageId) -> Result<MessageRow, StorageError> {
        self.call(move |conn| load_message(&conn.transaction()?, id))
            .await
    }
}

fn load_connection(tx: &Transaction, id: ConnectionId) -> Result<ConnectionRow, StorageError> {
    tx.query_row(
        "SELECT id, provider, display_name, model, free_tier, secret_name, revision FROM provider_connections WHERE id = ?1",
        [id.to_string()],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, i64>(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        },
    )
    .optional()?
    .ok_or(StorageError::NotFound("connection"))
    .and_then(|(id, provider, display_name, model, free_tier, secret_name, revision)| {
        Ok(ConnectionRow {
            id: ConnectionId(parse_uuid(&id)?),
            provider,
            display_name,
            model,
            free_tier: free_tier != 0,
            secret_name,
            revision,
        })
    })
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
    error_code, error_message, prompt_tokens, output_tokens, proposed_run_id, created_at_ms";

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
