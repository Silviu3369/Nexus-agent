//! The user's skills (`schema::V23`, ADR-PACK-001) on a real SQLite file:
//! a skill is on only for the content the user turned on, a new import of
//! the same name with other text turns it off, a stale decision is refused,
//! events never carry the text, and the migration keeps every step.

mod common;

use std::path::PathBuf;

use nexus_core::AgentStepKind;
use nexus_storage::{NewExchange, NewSkill, SUPPORTED_VERSION, SkillFileRow, StorageError, Store};
use serde_json::json;

struct Env {
    store: Store,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("nexus-skills-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

const TEXT: &str = "SENTINEL-skill-text: write the letter in Dutch.";

fn skill(name: &str, text: &str, sha: char) -> NewSkill {
    NewSkill {
        name: name.into(),
        description: "Writes cover letters for IT jobs.".into(),
        license: None,
        compatibility: None,
        allowed_tools: Some("Bash(git:*) Read".into()),
        has_scripts: true,
        skipped: vec!["scripts/run.py: scripts never run in Nexus".into()],
        files: vec![
            SkillFileRow {
                path: "SKILL.md".into(),
                text: text.into(),
            },
            SkillFileRow {
                path: "references/style.md".into(),
                text: "Formal, short.".into(),
            },
        ],
        content_sha256: sha.to_string().repeat(64),
    }
}

#[tokio::test]
async fn a_skill_is_on_only_for_the_content_turned_on() {
    let env = env();
    let s = &env.store;
    let added = s
        .import_skill(skill("cv-letter", TEXT, 'a'), 1)
        .await
        .unwrap();
    assert!(!added.enabled, "an imported skill starts off");
    assert!(!added.changed_since_enabled);
    assert!(added.has_scripts);
    assert_eq!(added.allowed_tools.as_deref(), Some("Bash(git:*) Read"));
    assert_eq!(added.skipped.len(), 1);
    let (_, files) = s.skill(added.id).await.unwrap();
    assert_eq!(
        files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["SKILL.md", "references/style.md"]
    );

    let on = s
        .set_skill_enabled(added.id, added.revision, true, 2)
        .await
        .unwrap();
    assert!(on.enabled);
    // A decision on a revision the user did not read is refused.
    assert!(matches!(
        s.set_skill_enabled(added.id, added.revision, false, 3)
            .await,
        Err(StorageError::RevisionConflict(_))
    ));

    // The same text again keeps it on; other text turns it off.
    let same = s
        .import_skill(skill("cv-letter", TEXT, 'a'), 4)
        .await
        .unwrap();
    assert_eq!(same.id, added.id);
    assert!(same.enabled);
    let changed = s
        .import_skill(skill("cv-letter", "Other text.", 'b'), 5)
        .await
        .unwrap();
    assert_eq!(changed.id, added.id, "replaced, not added twice");
    assert!(!changed.enabled);
    assert!(changed.changed_since_enabled);
    let (_, files) = s.skill(added.id).await.unwrap();
    assert_eq!(files[0].text, "Other text.");

    let reviewed = s
        .set_skill_enabled(changed.id, changed.revision, true, 6)
        .await
        .unwrap();
    assert!(reviewed.enabled && !reviewed.changed_since_enabled);
    let off = s
        .set_skill_enabled(reviewed.id, reviewed.revision, false, 7)
        .await
        .unwrap();
    assert!(!off.enabled && !off.changed_since_enabled);

    s.remove_skill(off.id, 8).await.unwrap();
    assert!(s.skills().await.unwrap().is_empty());
    assert!(matches!(
        s.skill(off.id).await,
        Err(StorageError::NotFound("skill"))
    ));

    let events = s
        .call(|c| {
            let mut stmt =
                c.prepare("SELECT kind, payload_json FROM events WHERE kind LIKE 'Skill%'")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "SkillImported",
            "SkillEnabled",
            "SkillImported",
            "SkillImported",
            "SkillEnabled",
            "SkillDisabled",
            "SkillRemoved"
        ]
    );
    for (_, payload) in &events {
        assert!(!payload.contains("SENTINEL"), "{payload}");
    }
}

#[tokio::test]
async fn migrating_to_v23_keeps_every_step_and_accepts_the_new_one() {
    let env = env();
    let s = &env.store;
    s.persona(0).await.unwrap();
    s.ensure_default_project(0).await.unwrap();
    let connection = s
        .save_connection(None, "Gemini", "g".into(), false, "gemini-v23".into(), 1)
        .await
        .unwrap()
        .id;
    let ex = s
        .append_exchange(
            NewExchange {
                requester: nexus_storage::Requester::User,
                pack: None,
                conversation_id: None,
                project_id: None,
                idempotency_key: "m".into(),
                text: "hi".into(),
                connection_id: connection,
                model: "m".into(),
                persona_version: 1,
                title: None,
            },
            2,
        )
        .await
        .unwrap();
    s.begin_step(
        ex.model_message.id,
        AgentStepKind::DocumentRead,
        json!({"chars": 7}),
        3,
    )
    .await
    .unwrap();
    s.call(|c| {
        common::undo_v23(c)?;
        Ok(())
    })
    .await
    .unwrap();
    let path = env.dir.join("nexus.db");
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let steps = reopened
        .steps_for_conversation(ex.conversation_id)
        .await
        .unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, AgentStepKind::DocumentRead);
    reopened
        .begin_step(ex.model_message.id, AgentStepKind::SkillUsed, json!({}), 4)
        .await
        .unwrap();
    assert!(reopened.skills().await.unwrap().is_empty());
}
