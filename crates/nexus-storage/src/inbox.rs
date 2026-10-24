//! A register's inbox (`schema::V13`): a shared folder whose new documents
//! the agent reads and records on its own. One row per file content (its
//! SHA-256), so the same file is handled once even if it is renamed or
//! copied again. The store keeps what was seen and what came of it; the
//! Host decides when a file is ready and starts its task.

use nexus_protocol::{ConversationId, FolderId, RegisterId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_uuid};
use crate::{StorageError, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxRow {
    pub register_id: RegisterId,
    pub folder_id: Option<FolderId>,
    pub enabled: bool,
    pub revision: i64,
    /// The user's CV each new ad is compared with: a shared folder and a
    /// path in it.
    pub cv_folder_id: Option<FolderId>,
    pub cv_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxFileRow {
    pub id: i64,
    pub path: String,
    pub sha256: String,
    /// Waiting, Started, Failed or Skipped.
    pub status: String,
    pub reason: Option<String>,
    pub conversation_id: Option<ConversationId>,
    pub seen_at_ms: i64,
    pub started_at_ms: Option<i64>,
    /// Times its task was started.
    pub attempts: i64,
    /// Waiting to be tried again from then on.
    pub retry_at_ms: Option<i64>,
}

/// How a file's task ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxSettle {
    Done,
    /// Tried again from `at` on its own.
    RetryAt {
        at: i64,
        reason: String,
    },
    Failed(String),
}

const FILE_COLUMNS: &str =
    "id, path, sha256, status, reason, conversation_id, seen_at_ms, started_at_ms,
    attempts, retry_at_ms";
const MAX_REASON_CHARS: usize = 300;

/// A row as read, before ids are parsed.
type RawFile = (
    (i64, String, String, String),
    (Option<String>, Option<String>, i64, Option<i64>),
    (i64, Option<i64>),
);

fn raw_file(r: &rusqlite::Row) -> rusqlite::Result<RawFile> {
    Ok((
        (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?),
        (r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?),
        (r.get(8)?, r.get(9)?),
    ))
}

fn parse_file(
    (
        (id, path, sha256, status),
        (reason, conversation, seen_at_ms, started_at_ms),
        (attempts, retry_at_ms),
    ): RawFile,
) -> Result<InboxFileRow, StorageError> {
    Ok(InboxFileRow {
        id,
        path,
        sha256,
        status,
        reason,
        conversation_id: conversation
            .map(|c| Ok::<_, StorageError>(ConversationId(parse_uuid(&c)?)))
            .transpose()?,
        seen_at_ms,
        started_at_ms,
        attempts,
        retry_at_ms,
    })
}

fn load_inbox(tx: &Transaction, register: RegisterId) -> Result<InboxRow, StorageError> {
    type Raw = (Option<String>, bool, i64, Option<String>, Option<String>);
    let row: Option<Raw> = tx
        .query_row(
            "SELECT folder_id, enabled, revision, cv_folder_id, cv_path FROM register_inboxes WHERE register_id = ?1",
            [register.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let folder_id = |f: Option<String>| {
        f.map(|f| Ok::<_, StorageError>(FolderId(parse_uuid(&f)?)))
            .transpose()
    };
    Ok(match row {
        None => InboxRow {
            register_id: register,
            folder_id: None,
            enabled: false,
            revision: 0,
            cv_folder_id: None,
            cv_path: None,
        },
        Some((folder, enabled, revision, cv_folder, cv_path)) => InboxRow {
            register_id: register,
            folder_id: folder_id(folder)?,
            enabled,
            revision,
            cv_folder_id: folder_id(cv_folder)?,
            cv_path,
        },
    })
}

impl Store {
    /// The register's inbox (an empty, off one if never set).
    pub async fn inbox(&self, register: RegisterId) -> Result<InboxRow, StorageError> {
        self.call(move |conn| load_inbox(&conn.transaction()?, register))
            .await
    }

    /// Enabled inboxes with their folder.
    pub async fn active_inboxes(&self) -> Result<Vec<InboxRow>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let ids: Vec<String> = {
                let mut stmt = tx.prepare(
                    "SELECT register_id FROM register_inboxes WHERE enabled = 1 AND folder_id IS NOT NULL",
                )?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            ids.iter()
                .map(|id| load_inbox(&tx, RegisterId(parse_uuid(id)?)))
                .collect()
        })
        .await
    }

    /// Chooses the folder and turns the inbox on or off; refused if it
    /// changed since `revision` (0 for one never set).
    #[allow(clippy::too_many_arguments)]
    pub async fn save_inbox(
        &self,
        register: RegisterId,
        revision: i64,
        folder: Option<FolderId>,
        enabled: bool,
        cv_folder: Option<FolderId>,
        cv_path: Option<String>,
        now: i64,
    ) -> Result<InboxRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_inbox(&tx, register)?;
            if current.revision != revision {
                return Err(StorageError::RevisionConflict("inbox"));
            }
            tx.execute(
                "INSERT INTO register_inboxes (register_id, folder_id, enabled, revision, created_at_ms, updated_at_ms,
                     cv_folder_id, cv_path)
                 VALUES (?1, ?2, ?3, 1, ?4, ?4, ?5, ?6)
                 ON CONFLICT (register_id) DO UPDATE SET folder_id = ?2, enabled = ?3,
                     revision = revision + 1, updated_at_ms = ?4, cv_folder_id = ?5, cv_path = ?6",
                params![
                    register.to_string(),
                    folder.map(|f| f.to_string()),
                    enabled,
                    now,
                    cv_folder.map(|f| f.to_string()),
                    cv_path
                ],
            )?;
            insert_event(&tx, "InboxUpdated", &register.to_string(), revision + 1, json!({"enabled": enabled}), now)?;
            let row = load_inbox(&tx, register)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Newest first.
    pub async fn inbox_files(
        &self,
        register: RegisterId,
        limit: u32,
    ) -> Result<Vec<InboxFileRow>, StorageError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {FILE_COLUMNS} FROM inbox_files WHERE register_id = ?1 ORDER BY id DESC LIMIT ?2"
            ))?;
            let raw = stmt
                .query_map(params![register.to_string(), limit], raw_file)?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(parse_file).collect()
        })
        .await
    }

    /// The contents already seen (any status), to tell a new file.
    pub async fn inbox_known(&self, register: RegisterId) -> Result<Vec<String>, StorageError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare("SELECT sha256 FROM inbox_files WHERE register_id = ?1")?;
            let shas = stmt
                .query_map([register.to_string()], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(shas)
        })
        .await
    }

    /// How many of the register's inbox files the agent finished with.
    pub async fn inbox_done_count(&self, register: RegisterId) -> Result<u32, StorageError> {
        self.call(move |conn| {
            let n: i64 = conn.query_row(
                "SELECT count(*) FROM inbox_files WHERE register_id = ?1 AND status = 'Done'",
                [register.to_string()],
                |r| r.get(0),
            )?;
            Ok(u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }

    /// Records a newly seen file (once per content). Returns false when the
    /// same content was seen before.
    pub async fn inbox_seen(
        &self,
        register: RegisterId,
        path: String,
        sha256: String,
        size_bytes: i64,
        skip_reason: Option<String>,
        now: i64,
    ) -> Result<bool, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let status = if skip_reason.is_some() { "Skipped" } else { "Waiting" };
            let added = tx.execute(
                "INSERT INTO inbox_files (register_id, path, sha256, size_bytes, status, reason, seen_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (register_id, sha256) DO NOTHING",
                params![register.to_string(), path, sha256, size_bytes, status, skip_reason, now],
            )?;
            if added == 1 {
                insert_event(&tx, "InboxFileSeen", &register.to_string(), 1, json!({"status": status}), now)?;
            }
            tx.commit()?;
            Ok(added == 1)
        })
        .await
    }

    /// The oldest file waiting for its turn (a file to be tried again only
    /// once its time has come).
    pub async fn next_inbox_file(
        &self,
        register: RegisterId,
        now: i64,
    ) -> Result<Option<InboxFileRow>, StorageError> {
        self.call(move |conn| {
            conn.query_row(
                &format!(
                    "SELECT {FILE_COLUMNS} FROM inbox_files WHERE register_id = ?1 AND status = 'Waiting'
                     AND (retry_at_ms IS NULL OR retry_at_ms <= ?2) ORDER BY id LIMIT 1"
                ),
                params![register.to_string(), now],
                raw_file,
            )
            .optional()?
            .map(parse_file)
            .transpose()
        })
        .await
    }

    pub async fn inbox_waiting(&self, register: RegisterId) -> Result<u32, StorageError> {
        self.call(move |conn| {
            let n: i64 = conn.query_row(
                "SELECT count(*) FROM inbox_files WHERE register_id = ?1 AND status = 'Waiting'",
                [register.to_string()],
                |r| r.get(0),
            )?;
            Ok(u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }

    /// What came of a file's turn: its task, or why none was started.
    pub async fn inbox_outcome(
        &self,
        file_id: i64,
        conversation: Option<ConversationId>,
        failure: Option<String>,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let register: String = tx
                .query_row("SELECT register_id FROM inbox_files WHERE id = ?1", [file_id], |r| r.get(0))
                .optional()?
                .ok_or(StorageError::NotFound("inbox file"))?;
            let (status, reason) = match (&conversation, failure) {
                (Some(_), _) => ("Started", None),
                (None, reason) => ("Failed", reason.map(|r| r.chars().take(MAX_REASON_CHARS).collect::<String>())),
            };
            tx.execute(
                "UPDATE inbox_files SET status = ?1, reason = ?2, conversation_id = ?3, started_at_ms = ?4,
                 retry_at_ms = NULL, attempts = attempts + (?3 IS NOT NULL) WHERE id = ?5",
                params![status, reason, conversation.map(|c| c.to_string()), now, file_id],
            )?;
            insert_event(&tx, "InboxFileHandled", &register, 1, json!({"status": status}), now)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// A file that could not be started goes back in line.
    pub async fn retry_inbox_file(
        &self,
        register: RegisterId,
        file_id: i64,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let conversation: Option<String> = tx
                .query_row(
                    "SELECT conversation_id FROM inbox_files WHERE id = ?1",
                    [file_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let changed = tx.execute(
                "UPDATE inbox_files SET status = 'Waiting', reason = NULL, retry_at_ms = NULL WHERE id = ?1
                 AND register_id = ?2 AND status = 'Failed'",
                params![file_id, register.to_string()],
            )?;
            if changed == 0 {
                return Err(StorageError::NotFound("failed inbox file"));
            }
            // The user tries a failed item again: about its last turn.
            if let Some((task, _)) =
                crate::interventions::turn_of(&tx, conversation.as_deref(), None)?
            {
                let turn: Option<String> = tx
                    .query_row(
                        "SELECT id FROM runs WHERE task_id = ?1 AND kind = 'Agent'
                         ORDER BY created_at_ms DESC, id LIMIT 1",
                        [&task],
                        |r| r.get(0),
                    )
                    .optional()?;
                crate::interventions::record(
                    &tx,
                    crate::Actor::LocalUser,
                    &task,
                    turn.as_deref(),
                    None,
                    nexus_core::InterventionKind::Retry,
                    json!({"inbox_file": file_id}),
                    now,
                )?;
            }
            insert_event(&tx, "InboxFileRetried", &register.to_string(), 1, json!({}), now)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Files whose task was started and is not yet settled.
    pub async fn inbox_started(
        &self,
        register: RegisterId,
    ) -> Result<Vec<InboxFileRow>, StorageError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {FILE_COLUMNS} FROM inbox_files WHERE register_id = ?1 AND status = 'Started' ORDER BY id"
            ))?;
            let raw = stmt
                .query_map([register.to_string()], raw_file)?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(parse_file).collect()
        })
        .await
    }

    /// Records how a started file's task ended.
    pub async fn settle_inbox_file(
        &self,
        file_id: i64,
        settle: InboxSettle,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let register: String = tx
                .query_row(
                    "SELECT register_id FROM inbox_files WHERE id = ?1 AND status IN ('Started', 'Waiting')",
                    [file_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or(StorageError::NotFound("started inbox file"))?;
            let clip = |r: String| r.chars().take(MAX_REASON_CHARS).collect::<String>();
            let (status, reason, retry_at): (&str, Option<String>, Option<i64>) = match settle {
                InboxSettle::Done => ("Done", None, None),
                InboxSettle::RetryAt { at, reason } => ("Waiting", Some(clip(reason)), Some(at)),
                InboxSettle::Failed(reason) => ("Failed", Some(clip(reason)), None),
            };
            tx.execute(
                "UPDATE inbox_files SET status = ?1, reason = ?2, retry_at_ms = ?3 WHERE id = ?4",
                params![status, reason, retry_at, file_id],
            )?;
            insert_event(
                &tx,
                "InboxFileSettled",
                &register,
                1,
                json!({"status": status}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// How a task's latest reply stands: (status, error code, error message).
    pub async fn last_reply(
        &self,
        conversation: ConversationId,
    ) -> Result<Option<(String, Option<String>, Option<String>)>, StorageError> {
        self.call(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT status, error_code, error_message FROM messages WHERE conversation_id = ?1 AND role = 'Model'
                     ORDER BY seq DESC LIMIT 1",
                    [conversation.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?)
        })
        .await
    }

    /// SHA-256 of every document Nexus itself wrote (approved document
    /// runs): an inbox never treats them as new ads.
    pub async fn written_document_hashes(&self) -> Result<Vec<String>, StorageError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT json_extract(outcome_json, '$.sha256') FROM runs
                 WHERE outcome_json IS NOT NULL AND json_extract(outcome_json, '$.sha256') IS NOT NULL",
            )?;
            let shas = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(shas)
        })
        .await
    }
}

/// One inbox item and how the agent's work on it went (ADR-TASK-001).
#[derive(Debug, Clone, PartialEq)]
pub struct IntakeItemRow {
    pub status: String,
    pub reason: Option<String>,
    /// The latest run of the item's task.
    pub run_state: Option<String>,
    pub failure_cause: Option<String>,
    pub run_started_ms: Option<i64>,
    pub run_updated_ms: Option<i64>,
    /// Tokens of the task's replies, per (provider, model, free tier).
    pub usage: Vec<(String, String, bool, i64, i64)>,
    /// The agent's changes to records in this task and how many of them
    /// the user undid.
    pub agent_changes: i64,
    pub undone: i64,
    /// What the user did about the latest turn, oldest first.
    pub interventions: Vec<(nexus_core::InterventionKind, i64)>,
}

impl Store {
    /// Every item of a register's inbox (skipped files left out) with how
    /// its task went.
    pub async fn intake_handling(
        &self,
        register: RegisterId,
    ) -> Result<Vec<IntakeItemRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let files: Vec<(String, Option<String>, Option<String>)> = {
                let mut stmt = tx.prepare(
                    "SELECT status, reason, conversation_id FROM inbox_files
                     WHERE register_id = ?1 AND status <> 'Skipped' ORDER BY id",
                )?;
                stmt.query_map([register.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<Result<_, _>>()?
            };
            let mut items = Vec::with_capacity(files.len());
            for (status, reason, conversation) in files {
                let mut item = IntakeItemRow {
                    status,
                    reason,
                    run_state: None,
                    failure_cause: None,
                    run_started_ms: None,
                    run_updated_ms: None,
                    usage: Vec::new(),
                    agent_changes: 0,
                    undone: 0,
                    interventions: Vec::new(),
                };
                if let Some(conversation) = conversation {
                    if let Some((run, state, cause, started, updated)) = tx
                        .query_row(
                            "SELECT r.id, r.state, r.failure_cause, r.created_at_ms, r.updated_at_ms
                             FROM runs r JOIN tasks t ON t.id = r.task_id
                             WHERE t.conversation_id = ?1 AND r.kind = 'Agent'
                             ORDER BY r.created_at_ms DESC, r.id LIMIT 1",
                            [&conversation],
                            |r| {
                                Ok((
                                    r.get::<_, String>(0)?,
                                    r.get(1)?,
                                    r.get(2)?,
                                    r.get(3)?,
                                    r.get(4)?,
                                ))
                            },
                        )
                        .optional()?
                    {
                        item.run_state = Some(state);
                        item.failure_cause = cause;
                        item.run_started_ms = Some(started);
                        item.run_updated_ms = Some(updated);
                        let mut stmt = tx.prepare(
                            "SELECT kind, at_ms FROM interventions WHERE run_id = ?1 ORDER BY at_ms, id",
                        )?;
                        item.interventions = stmt
                            .query_map([&run], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .map(|(kind, at)| Ok((crate::repo::parse_state(&kind)?, at)))
                            .collect::<Result<_, StorageError>>()?;
                    }
                    let mut stmt = tx.prepare(
                        "SELECT c.provider, coalesce(m.model, ''), c.free_tier,
                                coalesce(sum(m.prompt_tokens), 0), coalesce(sum(m.output_tokens), 0)
                         FROM messages m JOIN provider_connections c ON c.id = m.connection_id
                         WHERE m.conversation_id = ?1 AND m.role = 'Model'
                         GROUP BY c.provider, m.model, c.free_tier",
                    )?;
                    item.usage = stmt
                        .query_map([&conversation], |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? == 1, r.get(3)?, r.get(4)?))
                        })?
                        .collect::<Result<_, _>>()?;
                    let (changes, undone): (i64, i64) = tx.query_row(
                        "SELECT count(*), coalesce(sum(undone_at_ms IS NOT NULL), 0)
                         FROM record_changes WHERE conversation_id = ?1 AND actor = 'Agent'",
                        [&conversation],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    item.agent_changes = changes;
                    item.undone = undone;
                }
                items.push(item);
            }
            Ok(items)
        })
        .await
    }
}
