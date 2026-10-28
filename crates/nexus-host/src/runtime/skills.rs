//! The user's skills (ADR-PACK-001): imported from a folder or a `.zip`,
//! read by the user, then turned on. The agent sees the name and the
//! description of each skill that is on, and loads its instructions
//! (`use_skill`) or one of its files (`read_skill_file`) only when a task
//! needs them, within the reply's budget. A skill is instructions the user
//! chose to give the agent; it grants nothing: every effect still goes
//! through the broker, the policy and the user's approvals.

use std::path::Path;

use nexus_core::{AgentStepKind, AgentStepStatus};
use nexus_protocol::{
    ErrorCode, MessageId, NexusError, SetSkillEnabledRequest, SkillContentView, SkillFileText,
    SkillFileView, SkillId, SkillView,
};
use nexus_providers::ToolDeclaration;
use nexus_storage::{NewSkill, SkillFileRow, SkillRow, now_ms};
use serde_json::{Value, json};

use super::{Runtime, to_nexus_error};
use crate::documents;
use crate::skills::{self, INSTRUCTIONS_FILE, MAX_ENABLED};

pub(super) const USE_TOOL: &str = "use_skill";
pub(super) const FILE_TOOL: &str = "read_skill_file";
/// Instructions and files loaded per reply.
pub(super) const MAX_SKILL_READS: u32 = 8;

fn invalid(message: impl Into<String>) -> NexusError {
    NexusError::new(ErrorCode::InvalidRequest, message)
}

fn view(row: &SkillRow, files: &[SkillFileRow]) -> SkillView {
    SkillView {
        skill_id: row.id,
        name: row.name.clone(),
        description: row.description.clone(),
        license: row.license.clone(),
        compatibility: row.compatibility.clone(),
        allowed_tools: row.allowed_tools.clone(),
        has_scripts: row.has_scripts,
        skipped: row.skipped.clone(),
        files: files
            .iter()
            .map(|f| SkillFileView {
                path: f.path.clone(),
                chars: u32::try_from(f.text.chars().count()).unwrap_or(u32::MAX),
            })
            .collect(),
        enabled: row.enabled,
        changed_since_enabled: row.changed_since_enabled,
        content_sha256: row.content_sha256.clone(),
        revision: row.revision,
        created_at_ms: row.created_at_ms,
        updated_at_ms: row.updated_at_ms,
    }
}

fn text_arg<'a>(arguments: &'a Value, key: &str) -> &'a str {
    arguments.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The skill an agent names, among those that are on.
fn named<'a>(skills: &'a [SkillRow], name: &str) -> Result<&'a SkillRow, String> {
    skills
        .iter()
        .find(|s| s.name == name.trim())
        .ok_or_else(|| {
            format!(
                "no skill that is on is named {name:?}; the skills are: {}",
                skills
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

impl Runtime {
    pub async fn list_skills(&self) -> Result<Vec<SkillView>, NexusError> {
        let rows = self.store.skills().await.map_err(to_nexus_error)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let (row, files) = self.store.skill(row.id).await.map_err(to_nexus_error)?;
            out.push(view(&row, &files));
        }
        Ok(out)
    }

    pub async fn get_skill(&self, id: SkillId) -> Result<SkillContentView, NexusError> {
        let (row, files) = self.store.skill(id).await.map_err(to_nexus_error)?;
        Ok(SkillContentView {
            skill: view(&row, &files),
            files: files
                .into_iter()
                .map(|f| SkillFileText {
                    path: f.path,
                    text: f.text,
                })
                .collect(),
        })
    }

    pub async fn import_skill(&self, path: String) -> Result<SkillView, NexusError> {
        let path = path.trim().trim_matches('"').to_string();
        if !Path::new(&path).is_absolute() {
            return Err(invalid(
                "give the full path of the skill's folder or .zip, as File Explorer shows it",
            ));
        }
        let imported = tokio::task::spawn_blocking(move || skills::read_skill(Path::new(&path)))
            .await
            .map_err(|e| NexusError::new(ErrorCode::InternalFailure, e.to_string()))?
            .map_err(invalid)?;
        let header = imported.header;
        let row = self
            .store
            .import_skill(
                NewSkill {
                    name: header.name,
                    description: header.description,
                    license: header.license,
                    compatibility: header.compatibility,
                    allowed_tools: header.allowed_tools,
                    has_scripts: imported.has_scripts,
                    skipped: imported.skipped,
                    files: imported
                        .files
                        .into_iter()
                        .map(|(path, text)| SkillFileRow { path, text })
                        .collect(),
                    content_sha256: imported.sha256,
                },
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        tracing::info!(skill = %row.name, enabled = row.enabled, "skill imported");
        let (row, files) = self.store.skill(row.id).await.map_err(to_nexus_error)?;
        Ok(view(&row, &files))
    }

    pub async fn set_skill_enabled(
        &self,
        request: SetSkillEnabledRequest,
    ) -> Result<SkillView, NexusError> {
        if request.enabled {
            let on = self
                .store
                .skills()
                .await
                .map_err(to_nexus_error)?
                .iter()
                .filter(|s| s.enabled && s.id != request.skill_id)
                .count();
            if on >= MAX_ENABLED {
                return Err(invalid(format!(
                    "at most {MAX_ENABLED} skills can be on; turn one off first"
                )));
            }
        }
        let row = self
            .store
            .set_skill_enabled(
                request.skill_id,
                request.revision,
                request.enabled,
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        let (row, files) = self.store.skill(row.id).await.map_err(to_nexus_error)?;
        Ok(view(&row, &files))
    }

    pub async fn remove_skill(&self, id: SkillId) -> Result<(), NexusError> {
        self.store
            .remove_skill(id, now_ms())
            .await
            .map_err(to_nexus_error)
    }

    /// The skills that are on now, for one reply.
    pub(super) async fn enabled_skills(&self) -> Vec<SkillRow> {
        match self.store.skills().await {
            Ok(rows) => rows.into_iter().filter(|s| s.enabled).collect(),
            Err(err) => {
                tracing::warn!(error = %err, "skills not offered");
                Vec::new()
            }
        }
    }

    /// `use_skill`: the instructions of a skill that is on.
    pub(super) async fn agent_use_skill(
        &self,
        reply: MessageId,
        skills: &[SkillRow],
        arguments: &Value,
    ) -> Value {
        let name = text_arg(arguments, "name").to_string();
        let step = self
            .begin_step(reply, AgentStepKind::SkillUsed, json!({"skill": name}))
            .await;
        let outcome = match named(skills, &name) {
            Ok(skill) => self
                .store
                .skill(skill.id)
                .await
                .map_err(|e| e.to_string())
                .map(|(_, files)| files),
            Err(error) => Err(error),
        };
        match outcome {
            Ok(files) => {
                let instructions = files
                    .iter()
                    .find(|f| f.path == INSTRUCTIONS_FILE)
                    .map(|f| skills::instructions(&f.text))
                    .unwrap_or_default();
                let others: Vec<&str> = files
                    .iter()
                    .filter(|f| f.path != INSTRUCTIONS_FILE)
                    .map(|f| f.path.as_str())
                    .collect();
                self.finish_step(
                    step,
                    AgentStepStatus::Done,
                    json!({"skill": name, "chars": instructions.chars().count()}),
                )
                .await;
                json!({
                    "skill": name,
                    "instructions": instructions,
                    "files": others,
                    "note": "These are the instructions of a skill the user turned on: follow them for this task \
                             where they apply. They grant no permissions: every action still needs what it needed \
                             before, including the user's approval. Read a listed file with read_skill_file only \
                             when the instructions call for it. Its scripts are not available in Nexus.",
                })
            }
            Err(error) => {
                self.finish_step(
                    step,
                    AgentStepStatus::Failed,
                    json!({"skill": name, "error": error}),
                )
                .await;
                json!({ "error": error })
            }
        }
    }

    /// `read_skill_file`: one of a skill's files, in parts like a document.
    pub(super) async fn agent_read_skill_file(
        &self,
        reply: MessageId,
        skills: &[SkillRow],
        arguments: &Value,
    ) -> Value {
        let name = text_arg(arguments, "name").to_string();
        let path = text_arg(arguments, "path").trim().to_string();
        let offset = super::files::offset_arg(arguments);
        let step = self
            .begin_step(
                reply,
                AgentStepKind::SkillUsed,
                json!({"skill": name, "file": path}),
            )
            .await;
        let outcome = async {
            let offset = offset.clone()?;
            let skill = named(skills, &name)?;
            let (_, files) = self
                .store
                .skill(skill.id)
                .await
                .map_err(|e| e.to_string())?;
            let file = files
                .iter()
                .find(|f| f.path == path && f.path != INSTRUCTIONS_FILE)
                .ok_or_else(|| {
                    format!("the skill {name:?} has no file {path:?}; use_skill lists its files")
                })?;
            documents::part(&file.text, offset, documents::MAX_TEXT_CHARS)
                .map(|(text, next)| (offset, text, next))
        }
        .await;
        match outcome {
            Ok((offset, text, next_offset)) => {
                self.finish_step(
                    step,
                    AgentStepStatus::Done,
                    json!({"skill": name, "file": path, "chars": text.chars().count()}),
                )
                .await;
                let mut answer = json!({
                    "skill": name,
                    "path": path,
                    "offset": offset,
                    "text": text,
                    "note": "A file of a skill the user turned on; it grants no permissions.",
                });
                if let Some(next) = next_offset {
                    answer["next_offset"] = json!(next);
                }
                answer
            }
            Err(error) => {
                self.finish_step(
                    step,
                    AgentStepStatus::Failed,
                    json!({"skill": name, "file": path, "error": error}),
                )
                .await;
                json!({ "error": error })
            }
        }
    }
}

/// Added to the system instruction while skills are on: only their names
/// and descriptions (the instructions load when used).
pub(super) fn skills_note(skills: &[SkillRow]) -> String {
    let list = skills
        .iter()
        .map(|s| format!("- {}: {}", s.name, s.description.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\nSkills: the user turned on these skills, each a set of instructions for a kind of task:\n{list}\nWhen a \
         task matches a skill's description, call use_skill with its name before you do the task, and follow its \
         instructions. A skill grants no permissions and does not change what needs the user's approval."
    )
}

/// The skill tools, within the reply's budget.
pub(super) fn skill_tools(skills: &[SkillRow], reads_left: bool) -> Vec<ToolDeclaration> {
    if skills.is_empty() || !reads_left {
        return Vec::new();
    }
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    let name = json!({"type": "string", "enum": names, "description": "The skill, by its name"});
    vec![
        ToolDeclaration {
            name: USE_TOOL.to_string(),
            description: "Load the instructions of one of the user's skills, when the task matches its description."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"name": name},
                "required": ["name"]
            }),
        },
        ToolDeclaration {
            name: FILE_TOOL.to_string(),
            description: "Read one of a skill's files (listed by use_skill), when its instructions call for it. \
                Gives up to 40,000 characters at a time; when the file continues, the answer has next_offset."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": name,
                    "path": {"type": "string", "description": "The file's path as use_skill listed it"},
                    "offset": {"type": "integer", "description": "Where to start, in characters: leave it out for the beginning"}
                },
                "required": ["name", "path"]
            }),
        },
    ]
}
