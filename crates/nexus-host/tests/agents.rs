//! The user's agents through the real host (ADR-AGENT-001): an agent is
//! checked against what exists; a task started with it gets its
//! instructions after the persona, only its skills, none of the tools it
//! does not use, its own model and its project; a follow-up stays with it;
//! a task without one is Nexus as before; an archived agent starts nothing
//! new; what it wrote names it.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{
    AgentView, ErrorCode, RecordQuery, SaveAgentRequest, SaveProjectRequest, SendMessageRequest,
    SetSkillEnabledRequest,
};
use serde_json::{Value, json};
use support::TestHost;
use support::chat::{gemini, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "agents-token";
const KEY: &str = "SENTINEL-gemini-key-agents-5e70";
const HELPDESK: &str = "SENTINEL-support-instructions: answer with the ticket number first.";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(30),
        retry_waits: Vec::new(),
        ..RuntimeConfig::default()
    }
}

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let path =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("agents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Dir(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Imports a skill from a folder and turns it on, as the user would.
async fn skill_on(client: &NexusClient, dir: &Dir, name: &str) {
    let folder = dir.0.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\nDo it well."),
    )
    .unwrap();
    let skill = client
        .import_skill(folder.display().to_string())
        .await
        .unwrap();
    client.skill(skill.skill_id).await.unwrap();
    client
        .set_skill_enabled(SetSkillEnabledRequest {
            skill_id: skill.skill_id,
            revision: skill.revision,
            enabled: true,
        })
        .await
        .unwrap();
}

fn request(name: &str) -> SaveAgentRequest {
    SaveAgentRequest {
        agent_id: None,
        revision: 0,
        name: name.into(),
        description: String::new(),
        instructions: String::new(),
        tools_off: Vec::new(),
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        connection_id: None,
        model: None,
        project_id: None,
    }
}

async fn ask(
    client: &NexusClient,
    conversation: Option<nexus_protocol::ConversationId>,
    agent: Option<&AgentView>,
    key: &str,
) -> nexus_protocol::SendMessageResponse {
    client
        .send_message(SendMessageRequest {
            conversation_id: conversation,
            project_id: None,
            idempotency_key: key.into(),
            text: "How do I reset an MFA method?".into(),
            agent_id: agent.map(|a| a.agent_id),
        })
        .await
        .unwrap()
}

fn declared(body: &Value) -> Vec<String> {
    body["tools"][0]["functionDeclarations"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t["name"].as_str().map(str::to_string))
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

#[tokio::test]
async fn a_task_started_with_an_agent_runs_as_that_agent_and_nexus_stays_as_it_was() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
    let skills = Dir::new();
    skill_on(&client, &skills, "cv-letter").await;
    skill_on(&client, &skills, "helpdesk-tickets").await;
    let work = client
        .save_project(SaveProjectRequest {
            project_id: None,
            name: "Work".into(),
            description: String::new(),
        })
        .await
        .unwrap();

    // What does not exist, or names the default agent, is refused.
    let refused = |r: Result<AgentView, nexus_client::CallError>| r.unwrap_err().code();
    assert_eq!(
        refused(client.save_agent(request("nexus")).await),
        Some(ErrorCode::InvalidRequest)
    );
    let mut bad = request("Support desk");
    bad.tools_off = vec!["RunAnything".into()];
    assert_eq!(
        refused(client.save_agent(bad).await),
        Some(ErrorCode::InvalidRequest)
    );
    let mut bad = request("Support desk");
    bad.skills = vec!["not-imported".into()];
    assert_eq!(
        refused(client.save_agent(bad).await),
        Some(ErrorCode::InvalidRequest)
    );
    let mut bad = request("Support desk");
    bad.connection_id = Some(saved.connection_id);
    assert_eq!(
        refused(client.save_agent(bad).await),
        Some(ErrorCode::InvalidRequest)
    );

    let mut helpdesk = request("  Support desk ");
    helpdesk.description = "Tickets and Microsoft 365 at work.".into();
    helpdesk.instructions = HELPDESK.into();
    helpdesk.tools_off = vec!["WebRead".into(), "WebRead".into()];
    helpdesk.skills = vec!["helpdesk-tickets".into()];
    helpdesk.connection_id = Some(saved.connection_id);
    helpdesk.model = Some("gemini-helpdesk".into());
    helpdesk.project_id = Some(work.project_id);
    let helpdesk = client.save_agent(helpdesk).await.unwrap();
    assert_eq!(helpdesk.name, "Support desk");
    assert_eq!(helpdesk.tools_off, ["WebRead"], "each tool once");
    assert_eq!(
        refused(client.save_agent(request("support desk")).await),
        Some(ErrorCode::InvalidRequest),
        "two agents cannot share a name"
    );

    // Its task: its instructions, its skill, no web, its model, its project.
    fake.push(Reply::calls(&[(
        "fetch_url",
        json!({"url": "https://learn.microsoft.com/"}),
    )]));
    fake.push(Reply::text(&["Ticket first."]));
    let sent = ask(&client, None, Some(&helpdesk), "h1").await;
    settled(&client, &sent).await;
    let calls = fake.generations();
    let first = calls[0].json();
    let instruction = system(&first);
    assert!(instruction.contains("You are Nexus"), "the persona stays");
    assert!(instruction.contains("you work as \"Support desk\""));
    assert!(instruction.contains(HELPDESK));
    assert!(instruction.contains("helpdesk-tickets") && !instruction.contains("cv-letter"));
    assert!(!declared(&first).contains(&"fetch_url".to_string()));
    assert!(
        calls[0].target.contains("models/gemini-helpdesk:"),
        "{}",
        calls[0].target
    );
    let refusal = calls[1].json().to_string();
    assert!(
        refusal.contains("The user set up this agent without this tool"),
        "{refusal}"
    );
    let task = client.conversation(sent.conversation_id).await.unwrap();
    assert_eq!(task.agent.as_deref(), Some("Support desk"));
    assert_eq!(task.project_id, Some(work.project_id));
    assert!(
        task.messages[1]
            .steps
            .iter()
            .all(|s| s.agent.as_deref() == Some("Support desk"))
    );
    assert!(
        task.messages[1]
            .steps
            .iter()
            .all(|s| !matches!(s.detail, nexus_protocol::AgentStepDetail::Fetch { .. })),
        "no page was read"
    );

    // A follow-up stays with its agent, whatever the request names.
    fake.push(Reply::text(&["Still the ticket first."]));
    let again = ask(&client, Some(sent.conversation_id), None, "h2").await;
    settled(&client, &again).await;
    let follow = fake.generations()[2].json();
    assert!(system(&follow).contains(HELPDESK));
    assert!(
        fake.generations()[2]
            .target
            .contains("models/gemini-helpdesk:")
    );

    // Nexus, without an agent, is as before: every skill on, the web, the
    // active model.
    fake.push(Reply::text(&["Plain Nexus."]));
    let plain = ask(&client, None, None, "n1").await;
    settled(&client, &plain).await;
    let body = fake.generations()[3].json();
    assert!(!system(&body).contains("you work as"));
    assert!(system(&body).contains("cv-letter") && system(&body).contains("helpdesk-tickets"));
    assert!(declared(&body).contains(&"fetch_url".to_string()));
    assert!(
        fake.generations()[3]
            .target
            .contains(&format!("models/{MODEL}:"))
    );
    let list = client.conversations(10).await.unwrap();
    assert_eq!(
        list.iter()
            .find(|c| c.conversation_id == sent.conversation_id)
            .and_then(|c| c.agent.as_deref()),
        Some("Support desk")
    );
    assert_eq!(client.agents().await.unwrap()[0].tasks, 1);

    // What it wrote names it.
    let jobs = client.registers().await.unwrap().remove(0);
    let mut writer = request("Jobs");
    writer.skills = vec!["cv-letter".into()];
    let writer = client.save_agent(writer).await.unwrap();
    fake.push(Reply::calls(&[(
        "add_record",
        json!({"register": "job_applications", "values": {"company": "Acme", "role": "Helpdesk"}}),
    )]));
    fake.push(Reply::text(&["Saved."]));
    let saved_by = ask(&client, None, Some(&writer), "j1").await;
    settled(&client, &saved_by).await;
    let record = client
        .records(RecordQuery {
            register_id: jobs.register_id,
            statuses: Vec::new(),
            text: None,
            follow_up_due: false,
            limit: 0,
        })
        .await
        .unwrap()
        .remove(0);
    let history = client
        .record_detail(record.record_id)
        .await
        .unwrap()
        .history;
    assert_eq!(history[0].agent.as_deref(), Some("Jobs"));

    // Archived: nothing new starts with it; its task keeps its name.
    client.archive_agent(helpdesk.agent_id).await.unwrap();
    let err = client
        .send_message(SendMessageRequest {
            conversation_id: None,
            project_id: None,
            idempotency_key: "h3".into(),
            text: "Another ticket".into(),
            agent_id: Some(helpdesk.agent_id),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(ErrorCode::InvalidRequest));
    assert_eq!(
        client
            .agents()
            .await
            .unwrap()
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["Jobs"]
    );
    assert_eq!(
        client
            .conversation(sent.conversation_id)
            .await
            .unwrap()
            .agent
            .as_deref(),
        Some("Support desk")
    );
    host.shutdown().await;
}
