//! Who does what (ADR-IDENTITY-001, `schema::V18`): every actor has a
//! stable id, found by what it is. The Host names the actor from what it
//! knows (the dashboard's session is the local user; its own work belongs
//! to the agent, an automation, the inbox or itself), never from a payload.

use nexus_core::ActorKind;
use nexus_protocol::{ActorId, AutomationId, ConversationId};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::repo::parse_state;
use crate::{StorageError, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    LocalUser,
    Agent,
    Host,
    Inbox,
    Automation(AutomationId),
}

impl Actor {
    fn reference(self) -> String {
        match self {
            Actor::LocalUser => "user:local".to_string(),
            Actor::Agent => "agent".to_string(),
            Actor::Host => "host".to_string(),
            Actor::Inbox => "intake:inbox".to_string(),
            Actor::Automation(id) => format!("automation:{id}"),
        }
    }
}

/// An actor as views show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorRow {
    pub kind: ActorKind,
    pub name: String,
}

/// The id of `actor`. Every actor the Host names exists from version 18 on
/// (an automation's from its creation).
pub(crate) fn id_of(conn: &Connection, actor: Actor) -> Result<String, StorageError> {
    conn.query_row(
        "SELECT id FROM actors WHERE ref = ?1",
        [actor.reference()],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| StorageError::Corrupt(format!("no actor {}", actor.reference())))
}

/// The actor with id `id`, if any.
pub(crate) fn load(conn: &Connection, id: Option<&str>) -> Result<Option<ActorRow>, StorageError> {
    let Some(id) = id else {
        return Ok(None);
    };
    let (kind, name): (String, String) =
        conn.query_row("SELECT kind, name FROM actors WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    Ok(Some(ActorRow {
        kind: parse_state(&kind)?,
        name,
    }))
}

/// A new automation acts under its own name.
pub(crate) fn add_automation(
    tx: &Transaction,
    automation: AutomationId,
    name: &str,
    now: i64,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO actors (id, kind, ref, name, created_at_ms) VALUES (?1, 'Automation', ?2, ?3, ?4)",
        params![
            ActorId::new().to_string(),
            Actor::Automation(automation).reference(),
            name,
            now
        ],
    )?;
    Ok(())
}

/// An automation renamed keeps its actor, under the new name.
pub(crate) fn rename(tx: &Transaction, actor: Actor, name: &str) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE actors SET name = ?1 WHERE ref = ?2",
        params![name, actor.reference()],
    )?;
    Ok(())
}

/// A deleted automation's actor stays, disabled, so what it did still
/// names it.
pub(crate) fn disable(tx: &Transaction, actor: Actor, now: i64) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE actors SET disabled_at_ms = ?1 WHERE ref = ?2 AND disabled_at_ms IS NULL",
        params![now, actor.reference()],
    )?;
    Ok(())
}

impl Store {
    /// Who asked for the task of a conversation: the user, an automation or
    /// the inbox (`None` for an automation deleted before version 18).
    pub async fn task_requester(
        &self,
        conversation: ConversationId,
    ) -> Result<Option<ActorRow>, StorageError> {
        self.call(move |conn| {
            let id: Option<String> = conn
                .query_row(
                    "SELECT requested_by FROM tasks WHERE conversation_id = ?1",
                    [conversation.to_string()],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            load(conn, id.as_deref())
        })
        .await
    }
}
