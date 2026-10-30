//! The user's skills through the real host (ADR-PACK-001), on real files: a
//! skill comes in from a folder or a `.zip` and starts off; what does not
//! follow the format is refused with the reason; while it is off the model
//! never hears of it; once on, the model sees only its name and description
//! and loads its instructions and files when it uses it; its scripts are
//! never kept; new text turns it off until the user reads it.

mod support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{AgentStepDetail, AgentStepStatus, ErrorCode, SetSkillEnabledRequest};
use serde_json::{Value, json};
use support::TestHost;
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "skills-token";
const KEY: &str = "SENTINEL-gemini-key-skills-7d21";
const BODY: &str = "SENTINEL-instructions: write the letter formal, under one page.";
const DESCRIPTION: &str =
    "Writes cover letters for IT jobs. Use when the user asks for a motivation letter.";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(30),
        retry_waits: Vec::new(),
        ..RuntimeConfig::default()
    }
}

/// A folder in the test's temporary space, removed afterwards.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("skills-{}", uuid::Uuid::new_v4()))
            .join(name);
        std::fs::create_dir_all(&path).unwrap();
        Dir(path)
    }

    fn write(&self, relative: &str, text: &[u8]) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn text(&self) -> String {
        self.0.display().to_string()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn skill_md(name: &str, body: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {DESCRIPTION}\nallowed-tools: Bash(*) Read\nmetadata:\n  author: me\n---\n\n{body}\n"
    )
}

async fn connect(client: &NexusClient) {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

fn declared(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .flat_map(|t| {
                    t["functionDeclarations"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|d| d["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn system(body: &Value) -> String {
    body["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn answers<'a>(body: &'a Value, name: &str) -> Vec<&'a Value> {
    body["contents"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["parts"].as_array().unwrap())
        .filter(|p| p["functionResponse"]["name"] == name)
        .map(|p| &p["functionResponse"]["response"])
        .collect()
}

#[tokio::test]
async fn a_skill_is_imported_off_read_by_the_user_then_used_only_when_on() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;

    let skill = Dir::new("cv-letter");
    skill.write("SKILL.md", skill_md("cv-letter", BODY).as_bytes());
    skill.write("references/style.md", b"Formal, short, no cliches.");
    skill.write("scripts/run.py", b"import os; os.remove('everything')");
    skill.write("assets/logo.png", b"\x89PNG");
    let imported = client.import_skill(skill.text()).await.unwrap();
    assert!(!imported.enabled, "an imported skill starts off");
    assert_eq!(imported.description, DESCRIPTION);
    assert!(imported.has_scripts);
    assert_eq!(imported.allowed_tools.as_deref(), Some("Bash(*) Read"));
    assert_eq!(
        imported
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["SKILL.md", "references/style.md"]
    );
    assert!(
        imported
            .skipped
            .iter()
            .any(|s| s.starts_with("scripts/run.py"))
            && imported
                .skipped
                .iter()
                .any(|s| s.starts_with("assets/logo.png")),
        "{:?}",
        imported.skipped
    );
    let content = client.skill(imported.skill_id).await.unwrap();
    assert!(
        content.files[0].text.contains(BODY),
        "the user reads it all"
    );
    assert!(
        !content.files.iter().any(|f| f.text.contains("os.remove")),
        "scripts are not kept"
    );

    // Off: the model never hears of it.
    fake.push(Reply::text(&["Salut."]));
    let first = send(&client, None, "off", "Scrie o scrisoare").await;
    settled(&client, &first).await;
    let body = fake.generations().pop().unwrap().json();
    assert!(!system(&body).contains("cv-letter"));
    assert!(!declared(&body).contains(&"use_skill".to_string()));

    // On: only the name and description go out, until the model uses it.
    let on = client
        .set_skill_enabled(SetSkillEnabledRequest {
            skill_id: imported.skill_id,
            revision: imported.revision,
            enabled: true,
        })
        .await
        .unwrap();
    assert!(on.enabled);
    fake.push(Reply::calls(&[("use_skill", json!({"name": "cv-letter"}))]));
    fake.push(Reply::calls(&[(
        "read_skill_file",
        json!({"name": "cv-letter", "path": "references/style.md"}),
    )]));
    fake.push(Reply::calls(&[(
        "read_skill_file",
        json!({"name": "cv-letter", "path": "scripts/run.py"}),
    )]));
    fake.push(Reply::text(&["Hier is uw brief."]));
    let sent = send(&client, None, "on", "Scrie o scrisoare de motivație").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.text, "Hier is uw brief.");

    let generations = fake.generations();
    let n = generations.len();
    let first_call = generations[n - 4].json();
    let instruction = system(&first_call);
    assert!(
        instruction.contains(&format!("- cv-letter: {DESCRIPTION}")),
        "{instruction}"
    );
    assert!(
        !instruction.contains("SENTINEL-instructions"),
        "the instructions load only when used"
    );
    let tools = declared(&first_call);
    assert!(
        tools.contains(&"use_skill".to_string()) && tools.contains(&"read_skill_file".to_string())
    );

    let used = answers(&generations[n - 3].json(), "use_skill")[0].clone();
    assert_eq!(used["instructions"].as_str().unwrap(), BODY);
    assert_eq!(used["files"], json!(["references/style.md"]));
    assert!(
        used["note"]
            .as_str()
            .unwrap()
            .contains("grant no permissions")
    );
    let file = answers(&generations[n - 2].json(), "read_skill_file")[0].clone();
    assert_eq!(file["text"], "Formal, short, no cliches.");
    let script = answers(&generations[n - 1].json(), "read_skill_file")[1].clone();
    assert!(
        script["error"].as_str().unwrap().contains("no file"),
        "{script}"
    );

    let steps: Vec<(AgentStepStatus, String, Option<String>)> = reply
        .steps
        .iter()
        .filter_map(|s| match &s.detail {
            AgentStepDetail::SkillUsed { skill, file, .. } => {
                Some((s.status, skill.clone(), file.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        steps,
        [
            (AgentStepStatus::Done, "cv-letter".into(), None),
            (
                AgentStepStatus::Done,
                "cv-letter".into(),
                Some("references/style.md".into())
            ),
            (
                AgentStepStatus::Failed,
                "cv-letter".into(),
                Some("scripts/run.py".into())
            ),
        ]
    );

    // New text turns it off until the user reads it; the same text does not.
    let same = client.import_skill(skill.text()).await.unwrap();
    assert!(same.enabled);
    skill.write(
        "SKILL.md",
        skill_md("cv-letter", "Changed instructions.").as_bytes(),
    );
    let changed = client.import_skill(skill.text()).await.unwrap();
    assert_eq!(changed.skill_id, imported.skill_id);
    assert!(!changed.enabled && changed.changed_since_enabled);
    fake.push(Reply::text(&["Salut din nou."]));
    let again = send(&client, None, "changed", "Scrie o scrisoare").await;
    settled(&client, &again).await;
    let body = fake.generations().pop().unwrap().json();
    assert!(!system(&body).contains("cv-letter"));

    // A decision on a revision the user did not read is refused.
    let stale = client
        .set_skill_enabled(SetSkillEnabledRequest {
            skill_id: changed.skill_id,
            revision: on.revision,
            enabled: true,
        })
        .await
        .unwrap_err();
    assert_eq!(stale.code(), Some(ErrorCode::RevisionConflict));

    client.remove_skill(changed.skill_id).await.unwrap();
    assert!(client.skills().await.unwrap().is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn a_zip_comes_in_and_what_breaks_the_format_is_refused_with_the_reason() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;

    // A .zip of the skill's folder, as claude.ai takes it.
    let zips = Dir::new("zips");
    let zip_path = zips.0.join("helpdesk.zip");
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("helpdesk/SKILL.md", options).unwrap();
        zip.write_all(skill_md("helpdesk", "Answer tickets politely.").as_bytes())
            .unwrap();
        zip.start_file("helpdesk/references/tickets.md", options)
            .unwrap();
        zip.write_all(b"Ticket numbers go in the subject.").unwrap();
        zip.finish().unwrap();
    }
    let imported = client
        .import_skill(zip_path.display().to_string())
        .await
        .unwrap();
    assert_eq!(imported.name, "helpdesk");
    assert_eq!(imported.files.len(), 2);
    assert!(!imported.has_scripts && imported.skipped.is_empty());

    let mismatch = Dir::new("other-name");
    mismatch.write("SKILL.md", skill_md("cv-letter", "x").as_bytes());
    let no_skill = Dir::new("empty-skill");
    no_skill.write("README.md", b"nothing");
    let bad_name = Dir::new("Bad_Name");
    bad_name.write("SKILL.md", skill_md("Bad_Name", "x").as_bytes());
    let no_body = Dir::new("no-body");
    no_body.write("SKILL.md", skill_md("no-body", "").as_bytes());
    for (path, reason) in [
        (mismatch.text(), "must match"),
        (no_skill.text(), "no SKILL.md"),
        (bad_name.text(), "not valid"),
        (no_body.text(), "no instructions"),
        ("relative\\path".to_string(), "full path"),
        (zips.text() + "\\missing.zip", "nothing to import"),
    ] {
        let err = client.import_skill(path.clone()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::InvalidRequest), "{path}");
        assert!(err.to_string().contains(reason), "{path}: {err}");
    }
    assert_eq!(client.skills().await.unwrap().len(), 1);
    host.shutdown().await;
}
