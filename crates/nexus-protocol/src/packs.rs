//! DTOs for the packs built into Nexus (ADR-PACK-001): what each one adds
//! (registers, skills) and the rules for the tasks its intake starts, which
//! can only tighten the user's.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PackView {
    pub pack_id: String,
    pub name: String,
    pub version: u32,
    pub description: String,
    /// The registers it keeps, by name.
    pub registers: Vec<String>,
    pub skills: Vec<PackSkillView>,
    pub intake: Option<PackFlowView>,
}

/// A skill of a pack: part of Nexus, read-only.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PackSkillView {
    pub name: String,
    pub description: String,
    /// The whole `SKILL.md`.
    pub text: String,
}

/// The tasks a pack's intake starts, one per new file, and what they may do
/// beyond the user's own rules.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PackFlowView {
    /// What it applies to, e.g. "A new job ad in the inbox".
    pub label: String,
    /// The skill whose instructions the task follows.
    pub skill: String,
    /// Tools never offered to these tasks, by their labels.
    pub turned_off: Vec<String>,
    /// At most this many register writes without asking (fewer, if the
    /// user set fewer).
    pub record_writes_without_asking: Option<u32>,
}
