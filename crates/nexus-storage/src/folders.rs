//! Folders the user shares with Nexus (`schema::V12`). A row is the user's
//! permission and nothing more: the Host reads documents from the folder
//! when the user or the agent needs them. Events never carry the path.

use nexus_protocol::FolderId;
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_uuid};
use crate::{StorageError, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    pub id: FolderId,
    pub label: String,
    pub path: String,
    pub can_write: bool,
    pub revision: i64,
    pub created_at_ms: i64,
}

impl Store {
    pub async fn folders(&self) -> Result<Vec<FolderRow>, StorageError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, label, path, can_write, revision, created_at_ms FROM folders ORDER BY created_at_ms, id",
            )?;
            let raw = stmt
                .query_map([], raw_folder)?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(parse).collect()
        })
        .await
    }

    pub async fn folder(&self, id: FolderId) -> Result<FolderRow, StorageError> {
        self.call(move |conn| load(&conn.transaction()?, id)).await
    }

    /// A label or path already shared is refused (`AlreadyShared`).
    pub async fn add_folder(
        &self,
        label: String,
        path: String,
        can_write: bool,
        now: i64,
    ) -> Result<FolderRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            refuse_duplicate(&tx, None, &label, &path)?;
            let id = FolderId::new();
            tx.execute(
                "INSERT INTO folders (id, label, path, can_write, revision, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
                params![id.to_string(), label, path, can_write, now],
            )?;
            insert_event(&tx, "FolderShared", &id.to_string(), 1, json!({"can_write": can_write}), now)?;
            let row = load(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    pub async fn update_folder(
        &self,
        id: FolderId,
        revision: i64,
        label: String,
        path: String,
        can_write: bool,
        now: i64,
    ) -> Result<FolderRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load(&tx, id)?;
            if current.revision != revision {
                return Err(StorageError::RevisionConflict("folder"));
            }
            refuse_duplicate(&tx, Some(id), &label, &path)?;
            tx.execute(
                "UPDATE folders SET label = ?1, path = ?2, can_write = ?3, revision = revision + 1, updated_at_ms = ?4
                 WHERE id = ?5",
                params![label, path, can_write, now, id.to_string()],
            )?;
            insert_event(&tx, "FolderUpdated", &id.to_string(), revision + 1, json!({"can_write": can_write}), now)?;
            let row = load(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    pub async fn remove_folder(&self, id: FolderId, now: i64) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load(&tx, id)?;
            tx.execute("DELETE FROM folders WHERE id = ?1", [id.to_string()])?;
            insert_event(
                &tx,
                "FolderUnshared",
                &id.to_string(),
                current.revision + 1,
                json!({}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
}

fn refuse_duplicate(
    tx: &Transaction,
    except: Option<FolderId>,
    label: &str,
    path: &str,
) -> Result<(), StorageError> {
    let other: Option<String> = tx
        .query_row(
            "SELECT id FROM folders WHERE (label = ?1 COLLATE NOCASE OR path = ?2 COLLATE NOCASE)
             AND id IS NOT ?3 LIMIT 1",
            params![label, path, except.map(|e| e.to_string())],
            |r| r.get(0),
        )
        .optional()?;
    match other {
        Some(_) => Err(StorageError::AlreadyShared),
        None => Ok(()),
    }
}

type RawFolder = (String, String, String, bool, i64, i64);

fn raw_folder(r: &rusqlite::Row) -> rusqlite::Result<RawFolder> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
    ))
}

fn parse(
    (id, label, path, can_write, revision, created_at_ms): RawFolder,
) -> Result<FolderRow, StorageError> {
    Ok(FolderRow {
        id: FolderId(parse_uuid(&id)?),
        label,
        path,
        can_write,
        revision,
        created_at_ms,
    })
}

fn load(tx: &Transaction, id: FolderId) -> Result<FolderRow, StorageError> {
    tx.query_row(
        "SELECT id, label, path, can_write, revision, created_at_ms FROM folders WHERE id = ?1",
        [id.to_string()],
        raw_folder,
    )
    .optional()?
    .map(parse)
    .transpose()?
    .ok_or(StorageError::NotFound("folder"))
}
