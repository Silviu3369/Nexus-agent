//! The user's skills (`schema::V23`, ADR-PACK-001): instructions in the
//! open Agent Skills format, imported as text. A skill is on only while the
//! content the user turned on is the content stored; importing the same name
//! again with other text keeps the skill but turns it off until the user
//! reads it. Events carry the name and the hash, never the text.

use nexus_protocol::SkillId;
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_uuid};
use crate::{StorageError, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRow {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    /// Declared by the skill; Nexus ignores it (a skill approves nothing).
    pub allowed_tools: Option<String>,
    /// It ships scripts, which Nexus never runs.
    pub has_scripts: bool,
    /// Files of the skill that were not imported, and why.
    pub skipped: Vec<String>,
    pub content_sha256: String,
    /// On: the user turned on exactly this content.
    pub enabled: bool,
    /// The user turned on other content, which has since been replaced.
    pub changed_since_enabled: bool,
    pub revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFileRow {
    /// Relative to the skill's folder, with `/` between parts (`SKILL.md`
    /// for the instructions themselves).
    pub path: String,
    pub text: String,
}

/// A skill read and checked by the Host, ready to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSkill {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<String>,
    pub has_scripts: bool,
    pub skipped: Vec<String>,
    pub files: Vec<SkillFileRow>,
    pub content_sha256: String,
}

const COLUMNS: &str = "id, name, description, license, compatibility, allowed_tools, has_scripts, skipped_json, \
                       content_sha256, enabled_sha256, revision, created_at_ms, updated_at_ms";

impl Store {
    pub async fn skills(&self) -> Result<Vec<SkillRow>, StorageError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM skills ORDER BY name"))?;
            let raw = stmt
                .query_map([], raw_skill)?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(parse).collect()
        })
        .await
    }

    /// One skill with all its files.
    pub async fn skill(&self, id: SkillId) -> Result<(SkillRow, Vec<SkillFileRow>), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let row = load(&tx, id)?;
            let files = files(&tx, id)?;
            Ok((row, files))
        })
        .await
    }

    /// Adds the skill, or replaces the one with its name (its files and
    /// details). It stays on only if the new content is the one turned on.
    pub async fn import_skill(&self, new: NewSkill, now: i64) -> Result<SkillRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let existing: Option<(String, i64, String)> = tx
                .query_row(
                    "SELECT id, revision, content_sha256 FROM skills WHERE name = ?1",
                    [&new.name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let skipped = serde_json::to_string(&new.skipped)
                .map_err(|e| StorageError::Corrupt(e.to_string()))?;
            let (id, revision, changed) = match existing {
                Some((id, revision, old_sha)) => {
                    tx.execute(
                        "UPDATE skills SET description = ?1, license = ?2, compatibility = ?3, allowed_tools = ?4,
                         has_scripts = ?5, skipped_json = ?6, content_sha256 = ?7, revision = revision + 1,
                         updated_at_ms = ?8 WHERE id = ?9",
                        params![
                            new.description,
                            new.license,
                            new.compatibility,
                            new.allowed_tools,
                            new.has_scripts,
                            skipped,
                            new.content_sha256,
                            now,
                            id
                        ],
                    )?;
                    tx.execute("DELETE FROM skill_files WHERE skill_id = ?1", [&id])?;
                    (
                        SkillId(parse_uuid(&id)?),
                        revision + 1,
                        old_sha != new.content_sha256,
                    )
                }
                None => {
                    let id = SkillId::new();
                    tx.execute(
                        "INSERT INTO skills (id, name, description, license, compatibility, allowed_tools,
                         has_scripts, skipped_json, content_sha256, enabled_sha256, revision, created_at_ms,
                         updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, 1, ?10, ?10)",
                        params![
                            id.to_string(),
                            new.name,
                            new.description,
                            new.license,
                            new.compatibility,
                            new.allowed_tools,
                            new.has_scripts,
                            skipped,
                            new.content_sha256,
                            now
                        ],
                    )?;
                    (id, 1, true)
                }
            };
            for file in &new.files {
                tx.execute(
                    "INSERT INTO skill_files (skill_id, path, text) VALUES (?1, ?2, ?3)",
                    params![id.to_string(), file.path, file.text],
                )?;
            }
            insert_event(
                &tx,
                "SkillImported",
                &id.to_string(),
                revision,
                json!({"name": new.name, "sha256": new.content_sha256, "changed": changed}),
                now,
            )?;
            let row = load(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Turns the skill on for exactly the content stored now, or off.
    pub async fn set_skill_enabled(
        &self,
        id: SkillId,
        revision: i64,
        enabled: bool,
        now: i64,
    ) -> Result<SkillRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load(&tx, id)?;
            if current.revision != revision {
                return Err(StorageError::RevisionConflict("skill"));
            }
            tx.execute(
                "UPDATE skills SET enabled_sha256 = CASE WHEN ?1 THEN content_sha256 END,
                 revision = revision + 1, updated_at_ms = ?2 WHERE id = ?3",
                params![enabled, now, id.to_string()],
            )?;
            insert_event(
                &tx,
                if enabled {
                    "SkillEnabled"
                } else {
                    "SkillDisabled"
                },
                &id.to_string(),
                revision + 1,
                json!({"name": current.name, "sha256": current.content_sha256}),
                now,
            )?;
            let row = load(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// For good, with its files; the steps that used it keep its name.
    pub async fn remove_skill(&self, id: SkillId, now: i64) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load(&tx, id)?;
            tx.execute("DELETE FROM skills WHERE id = ?1", [id.to_string()])?;
            insert_event(
                &tx,
                "SkillRemoved",
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

type RawSkill = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    String,
    String,
    Option<String>,
    i64,
    i64,
    i64,
);

fn raw_skill(r: &rusqlite::Row) -> rusqlite::Result<RawSkill> {
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
    ))
}

fn parse(
    (
        id,
        name,
        description,
        license,
        compatibility,
        allowed_tools,
        has_scripts,
        skipped,
        content_sha256,
        enabled_sha256,
        revision,
        created_at_ms,
        updated_at_ms,
    ): RawSkill,
) -> Result<SkillRow, StorageError> {
    let enabled = enabled_sha256.as_deref() == Some(content_sha256.as_str());
    Ok(SkillRow {
        id: SkillId(parse_uuid(&id)?),
        name,
        description,
        license,
        compatibility,
        allowed_tools,
        has_scripts,
        skipped: serde_json::from_str(&skipped)
            .map_err(|e| StorageError::Corrupt(e.to_string()))?,
        changed_since_enabled: enabled_sha256.is_some() && !enabled,
        enabled,
        content_sha256,
        revision,
        created_at_ms,
        updated_at_ms,
    })
}

fn load(tx: &Transaction, id: SkillId) -> Result<SkillRow, StorageError> {
    tx.query_row(
        &format!("SELECT {COLUMNS} FROM skills WHERE id = ?1"),
        [id.to_string()],
        raw_skill,
    )
    .optional()?
    .map(parse)
    .transpose()?
    .ok_or(StorageError::NotFound("skill"))
}

fn files(tx: &Transaction, id: SkillId) -> Result<Vec<SkillFileRow>, StorageError> {
    let mut stmt =
        tx.prepare("SELECT path, text FROM skill_files WHERE skill_id = ?1 ORDER BY path")?;
    let rows = stmt
        .query_map([id.to_string()], |r| {
            Ok(SkillFileRow {
                path: r.get(0)?,
                text: r.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
