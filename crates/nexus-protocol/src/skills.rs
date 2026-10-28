//! DTOs for the user's skills (ADR-PACK-001): instructions in the open Agent
//! Skills format (`SKILL.md`). A skill is text the agent may follow; it
//! grants nothing, its scripts never run and its `allowed-tools` is ignored.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::SkillId;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SkillView {
    pub skill_id: SkillId,
    pub name: String,
    /// What it does and when to use it: all the model sees until it uses
    /// the skill.
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    /// What the skill declares; ignored by Nexus.
    pub allowed_tools: Option<String>,
    /// It ships scripts, which Nexus never runs.
    pub has_scripts: bool,
    /// Files that were not imported, with the reason.
    pub skipped: Vec<String>,
    pub files: Vec<SkillFileView>,
    /// On: the agent may use exactly the content shown.
    pub enabled: bool,
    /// It was on, then its text was replaced: off until the user reads it.
    pub changed_since_enabled: bool,
    pub content_sha256: String,
    pub revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SkillFileView {
    /// Relative to the skill's folder (`SKILL.md` for the instructions).
    pub path: String,
    pub chars: u32,
}

/// A skill with every file's text, to read before turning it on.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SkillContentView {
    pub skill: SkillView,
    pub files: Vec<SkillFileText>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SkillFileText {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SetSkillEnabledRequest {
    pub skill_id: SkillId,
    /// The revision the user read; a skill changed since is refused.
    pub revision: i64,
    pub enabled: bool,
}
