//! First-party packs (ADR-PACK-001): what one workflow needs, as data built
//! into the Host — its registers, its skills (instructions in the open
//! Agent Skills format) and its intake, with the rules for the tasks the
//! intake starts. A pack has no code and comes only with Nexus. Its rules
//! can only tighten the user's: a tool turned off for its tasks, fewer
//! register writes without asking; nothing a pack says turns a tool on.

use std::sync::OnceLock;

use nexus_core::policy::MAX_BATCH_ITEMS;
use nexus_protocol::{PackFlowView, PackSkillView, PackView};
use serde::Deserialize;

use super::Runtime;
use super::records::RegisterSchema;
use crate::skills;
use crate::tools::registry;

const JOBS_MANIFEST: &str = include_str!("../../packs/jobs/pack.json");
const JOBS_SKILLS: [(&str, &str); 1] = [(
    "job-ad-intake",
    include_str!("../../packs/jobs/skills/job-ad-intake/SKILL.md"),
)];

/// `pack.json`, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    version: u32,
    name: String,
    description: String,
    registers: Vec<PackRegister>,
    skills: Vec<String>,
    intake: Option<IntakeManifest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntakeManifest {
    register: String,
    skill: String,
    title: String,
    label: String,
    turned_off: Vec<String>,
    record_writes_without_asking: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackRegister {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) schema: RegisterSchema,
}

#[derive(Debug)]
pub(super) struct Pack {
    pub(super) id: String,
    pub(super) version: u32,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) registers: Vec<PackRegister>,
    pub(super) skills: Vec<PackSkill>,
    pub(super) intake: Option<Intake>,
}

#[derive(Debug)]
pub(super) struct PackSkill {
    pub(super) name: String,
    pub(super) description: String,
    /// The whole `SKILL.md`, as the user reads it.
    pub(super) text: String,
    /// What follows its header: what the agent is given.
    pub(super) instructions: String,
}

/// The task a pack starts for each new file in its register's inbox, and
/// the rules that task runs under.
#[derive(Debug)]
pub(super) struct Intake {
    pub(super) register: String,
    pub(super) skill: String,
    /// Names the task, before the file's name.
    pub(super) title: String,
    /// What the rules apply to, as the user reads it.
    pub(super) label: String,
    pub(super) rules: FlowRules,
}

/// A pack's rules for one flow. They only tighten the user's: the user's
/// rules still hold, and these take away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FlowRules {
    /// Tools never offered in this flow, whatever the user allows.
    pub(super) turned_off: Vec<&'static str>,
    /// At most this many register writes without asking (fewer, if the
    /// user set fewer).
    pub(super) record_writes_without_asking: Option<u32>,
}

impl Pack {
    pub(super) fn skill(&self, name: &str) -> Option<&PackSkill> {
        self.skills.iter().find(|s| s.name == name)
    }
}

impl Intake {
    pub(super) fn view(&self) -> PackFlowView {
        PackFlowView {
            label: self.label.clone(),
            skill: self.skill.clone(),
            turned_off: self
                .rules
                .turned_off
                .iter()
                .filter_map(|id| registry::by_id(id))
                .map(|t| t.label.to_string())
                .collect(),
            record_writes_without_asking: self.rules.record_writes_without_asking,
        }
    }
}

fn check_schema(key: &str, schema: &RegisterSchema) -> Result<(), String> {
    let has = |name: &str| schema.fields.iter().any(|f| f.key == name);
    for (i, f) in schema.fields.iter().enumerate() {
        if schema.fields[..i].iter().any(|g| g.key == f.key) {
            return Err(format!("{key}: the field {:?} is there twice", f.key));
        }
    }
    let named = [
        Some(&schema.status_field),
        schema.follow_up_field.as_ref(),
        schema.key_field.as_ref(),
    ];
    for name in named.into_iter().flatten().chain(&schema.title_fields) {
        if !has(name) {
            return Err(format!("{key}: no field {name:?}"));
        }
    }
    let status = schema
        .fields
        .iter()
        .find(|f| f.key == schema.status_field)
        .map(|f| &f.choices);
    let known = |s: &String| status.is_some_and(|c| c.contains(s));
    if !known(&schema.default_status) || !schema.closed_statuses.iter().all(known) {
        return Err(format!("{key}: a status the status field does not offer"));
    }
    Ok(())
}

fn load(manifest: &str, skill_files: &[(&str, &str)]) -> Result<Pack, String> {
    let m: Manifest = serde_json::from_str(manifest).map_err(|e| format!("pack.json: {e}"))?;
    skills::check_name(&m.id).map_err(|e| format!("pack id: {e}"))?;
    let mut pack_skills = Vec::new();
    for name in &m.skills {
        let text = skill_files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| *t)
            .ok_or_else(|| format!("{}: no SKILL.md for {name:?}", m.id))?;
        let (header, instructions) =
            skills::parse_skill_md(text).map_err(|e| format!("{name}: {e}"))?;
        if header.name != *name {
            return Err(format!("{name}: its SKILL.md is named {:?}", header.name));
        }
        pack_skills.push(PackSkill {
            name: header.name,
            description: header.description,
            // As written, whatever line endings the checkout gave it.
            text: text.replace("\r\n", "\n"),
            instructions,
        });
    }
    for r in &m.registers {
        check_schema(&r.key, &r.schema)?;
    }
    let intake = match m.intake {
        None => None,
        Some(i) => {
            if !m.registers.iter().any(|r| r.key == i.register) {
                return Err(format!(
                    "intake: {:?} is not a register of the pack",
                    i.register
                ));
            }
            if !m.skills.contains(&i.skill) {
                return Err(format!("intake: {:?} is not a skill of the pack", i.skill));
            }
            let turned_off = i
                .turned_off
                .iter()
                .map(|id| {
                    registry::by_id(id)
                        .filter(|t| registry::offered_to_model(t))
                        .map(|t| t.id)
                        .ok_or_else(|| format!("intake: no tool {id:?} to turn off"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if i.record_writes_without_asking
                .is_some_and(|n| n as usize > MAX_BATCH_ITEMS)
            {
                return Err(format!(
                    "intake: at most {MAX_BATCH_ITEMS} register writes without asking"
                ));
            }
            Some(Intake {
                register: i.register,
                skill: i.skill,
                title: i.title,
                label: i.label,
                rules: FlowRules {
                    turned_off,
                    record_writes_without_asking: i.record_writes_without_asking,
                },
            })
        }
    };
    Ok(Pack {
        id: m.id,
        version: m.version,
        name: m.name,
        description: m.description,
        registers: m.registers,
        skills: pack_skills,
        intake,
    })
}

/// The packs built into this version of Nexus.
pub(super) fn builtin() -> &'static [Pack] {
    static PACKS: OnceLock<Vec<Pack>> = OnceLock::new();
    PACKS.get_or_init(|| {
        vec![
            load(JOBS_MANIFEST, &JOBS_SKILLS)
                .expect("the Jobs pack built into Nexus is valid (tested)"),
        ]
    })
}

pub(super) fn by_id(id: &str) -> Option<&'static Pack> {
    builtin().iter().find(|p| p.id == id)
}

/// The pack whose intake fills this register, with its intake.
pub(super) fn intake_of(register: &str) -> Option<(&'static Pack, &'static Intake)> {
    builtin().iter().find_map(|p| {
        p.intake
            .as_ref()
            .filter(|i| i.register == register)
            .map(|i| (p, i))
    })
}

pub(super) fn view(pack: &Pack) -> PackView {
    PackView {
        pack_id: pack.id.clone(),
        name: pack.name.clone(),
        version: pack.version,
        description: pack.description.clone(),
        registers: pack.registers.iter().map(|r| r.name.clone()).collect(),
        skills: pack
            .skills
            .iter()
            .map(|s| PackSkillView {
                name: s.name.clone(),
                description: s.description.clone(),
                text: s.text.clone(),
            })
            .collect(),
        intake: pack.intake.as_ref().map(Intake::view),
    }
}

impl Runtime {
    /// The packs built into this version, as the user reads them.
    pub fn list_packs(&self) -> Vec<PackView> {
        builtin().iter().map(view).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jobs_pack_loads_with_its_register_skill_and_intake() {
        let jobs = by_id("jobs").expect("built in");
        assert_eq!(
            jobs.registers[0].key,
            super::super::records::JOB_APPLICATIONS
        );
        let statuses = &jobs.registers[0]
            .schema
            .fields
            .iter()
            .find(|f| f.key == "status")
            .unwrap()
            .choices;
        assert_eq!(
            statuses,
            &[
                "To apply",
                "Applied",
                "Interviewing",
                "Offer",
                "Rejected",
                "Withdrawn"
            ],
            "the statuses the user's applications already have"
        );
        let intake = jobs.intake.as_ref().unwrap();
        let skill = jobs.skill(&intake.skill).unwrap();
        assert!(skill.instructions.starts_with("# Job ad intake"));
        assert!(skill.text.starts_with("---\nname: job-ad-intake"));
        assert!(intake.rules.turned_off.contains(&"WebRead"));
        assert_eq!(intake.rules.record_writes_without_asking, Some(2));
        let (pack, found) = intake_of("job_applications").unwrap();
        assert_eq!(
            (pack.id.as_str(), found.skill.as_str()),
            ("jobs", "job-ad-intake")
        );
        let shown = found.view();
        assert!(
            shown
                .turned_off
                .contains(&"Read public web pages".to_string())
        );

        // A checkout with Windows line endings reads the same.
        let lf = JOBS_SKILLS[0].1.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        let loaded = load(JOBS_MANIFEST, &[("job-ad-intake", crlf.as_str())]).unwrap();
        assert_eq!(loaded.skills[0].text, lf);
        assert_eq!(loaded.skills[0].instructions, skill.instructions);
    }

    #[test]
    fn a_pack_that_names_what_does_not_exist_is_refused() {
        let good = JOBS_MANIFEST;
        let swap = |from: &str, to: &str| {
            assert!(good.contains(from), "{from}");
            load(&good.replacen(from, to, 1), &JOBS_SKILLS).unwrap_err()
        };
        assert!(swap("\"WebRead\"", "\"RunAnything\"").contains("no tool"));
        assert!(
            swap("\"skill\": \"job-ad-intake\"", "\"skill\": \"other\"").contains("not a skill")
        );
        assert!(
            swap(
                "\"record_writes_without_asking\": 2",
                "\"record_writes_without_asking\": 99"
            )
            .contains("at most")
        );
        assert!(
            swap(
                "\"default_status\": \"To apply\"",
                "\"default_status\": \"Maybe\""
            )
            .contains("status")
        );
        assert!(
            swap(
                "\"version\": 1,",
                "\"version\": 1, \"allow\": [\"WebRead\"],"
            )
            .contains("unknown field")
        );
        let wrong_name = [(
            "job-ad-intake",
            JOBS_SKILLS[0]
                .1
                .replacen("name: job-ad-intake", "name: other", 1),
        )];
        let wrong: Vec<(&str, &str)> = wrong_name.iter().map(|(n, t)| (*n, t.as_str())).collect();
        assert!(load(good, &wrong).unwrap_err().contains("is named"));
    }
}
