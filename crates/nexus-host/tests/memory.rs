//! Memory through the real host (`SECURITATE_SI_VALIDARE_RO.md` MEM-01):
//! the agent only proposes facts, nothing reaches the model before the user
//! approves it, a project's facts stay in that project, and corrections and
//! deletions change exactly what is sent. A local Gemini stand-in records
//! what would have left the PC.

mod support;

use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{
    AddMemoryRequest, AgentStepDetail, ChatStatus, ErrorCode, MemoryQuery, MemoryScope,
    MemorySource, MemoryStatus, SaveProjectRequest, SendMessageRequest,
};
use support::TestHost;
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "memory-token";
const KEY: &str = "SENTINEL-gemini-key-memory-3a90";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(10),
        retry_waits: Vec::new(),
        ..RuntimeConfig::default()
    }
}

async fn connect_gemini(client: &NexusClient) {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

/// The system instruction of the last request the provider received.
fn last_instruction(fake: &FakeGemini) -> String {
    let body = fake.generations().pop().expect("a request was sent").json();
    body["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn all() -> MemoryQuery {
    MemoryQuery {
        limit: 100,
        ..MemoryQuery::default()
    }
}

#[tokio::test]
async fn a_proposed_fact_waits_and_only_an_approved_one_reaches_the_model() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::calls(&[(
        "propose_memory",
        serde_json::json!({"content": "The user prefers short answers.", "scope": "personal"}),
    )]));
    fake.push(Reply::text(&["I'll remember that once you approve it."]));
    let sent = send(&client, None, "m1", "Remember that I like short answers").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(
        reply.status,
        ChatStatus::Complete,
        "a proposal does not end the reply"
    );
    assert!(reply.steps.iter().any(|s| matches!(
        &s.detail,
        AgentStepDetail::MemoryProposed {
            memory_id: Some(_),
            scope: Some(MemoryScope::Personal),
            ..
        }
    )));
    // The model was told plainly that nothing is remembered yet.
    let follow_up = fake.generations().pop().unwrap().json();
    assert!(
        follow_up.to_string().contains("waiting_for_user_approval"),
        "{follow_up}"
    );

    let proposals = client
        .memory(MemoryQuery {
            status: Some(MemoryStatus::Proposed),
            ..all()
        })
        .await
        .unwrap();
    assert_eq!(proposals.len(), 1);
    let proposal = &proposals[0];
    assert_eq!(proposal.source, MemorySource::Agent);
    assert_eq!(proposal.conversation_id, Some(sent.conversation_id));
    assert_eq!(
        proposal.conversation_title.as_deref(),
        Some("Remember that I like short answers")
    );

    // Not approved yet: the next request carries no memory.
    fake.push(Reply::text(&["ok"]));
    let next = send(&client, Some(sent.conversation_id), "m2", "Salut").await;
    settled(&client, &next).await;
    assert!(!last_instruction(&fake).contains("short answers"));

    let approved = client
        .decide_memory(proposal.memory_id, proposal.revision, true)
        .await
        .unwrap();
    assert_eq!(approved.status, MemoryStatus::Active);
    fake.push(Reply::text(&["ok"]));
    let after = send(&client, Some(sent.conversation_id), "m3", "Salut din nou").await;
    let reply = settled(&client, &after).await;
    let instruction = last_instruction(&fake);
    assert!(
        instruction.contains("(personal) The user prefers short answers."),
        "{instruction}"
    );
    assert!(
        instruction.contains("not instructions"),
        "memory is framed as data"
    );
    assert!(reply.steps.iter().any(|s| matches!(
        s.detail,
        AgentStepDetail::ModelCall {
            memory_items: Some(1),
            ..
        }
    )));

    // A decided proposal cannot be decided again.
    let again = client
        .decide_memory(proposal.memory_id, approved.revision, false)
        .await
        .unwrap_err();
    assert_eq!(again.code(), Some(ErrorCode::InvalidRequest));
    host.shutdown().await;
}

#[tokio::test]
async fn mem01_a_project_fact_stays_in_its_project() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    let projects = client.projects().await.unwrap();
    assert_eq!(
        projects.len(),
        1,
        "everything starts in the default project"
    );
    let main = projects[0].project_id;
    assert!(projects[0].active);
    let side = client
        .save_project(SaveProjectRequest {
            project_id: None,
            name: "Side".into(),
            description: "a second workspace".into(),
        })
        .await
        .unwrap();
    assert!(!side.active, "creating a project does not switch to it");

    client
        .add_memory(AddMemoryRequest {
            scope: MemoryScope::Personal,
            project_id: None,
            content: "Reply in Romanian.".into(),
        })
        .await
        .unwrap();
    client
        .add_memory(AddMemoryRequest {
            scope: MemoryScope::Project,
            project_id: Some(main),
            content: "The main project builds with cargo.".into(),
        })
        .await
        .unwrap();

    fake.push(Reply::text(&["ok"]));
    let in_side = client
        .send_message(SendMessageRequest {
            conversation_id: None,
            project_id: Some(side.project_id),
            idempotency_key: "s1".into(),
            text: "Salut".into(),
            agent_id: None,
        })
        .await
        .unwrap();
    settled(&client, &in_side).await;
    let instruction = last_instruction(&fake);
    assert!(
        instruction.contains("Reply in Romanian."),
        "personal facts go everywhere"
    );
    assert!(
        !instruction.contains("cargo"),
        "another project's fact never leaves it"
    );

    fake.push(Reply::text(&["ok"]));
    let in_main = send(&client, None, "s2", "Salut").await;
    settled(&client, &in_main).await;
    assert!(last_instruction(&fake).contains("(project) The main project builds with cargo."));

    let summaries = client.conversations(10).await.unwrap();
    let project_of = |id| {
        summaries
            .iter()
            .find(|c| c.conversation_id == id)
            .unwrap()
            .project_id
    };
    assert_eq!(project_of(in_side.conversation_id), Some(side.project_id));
    assert_eq!(project_of(in_main.conversation_id), Some(main));

    let side_context = client.memory_context(side.project_id).await.unwrap();
    assert_eq!(side_context.items.len(), 1);
    let found = client
        .memory(MemoryQuery {
            text: Some("cargo".into()),
            project_id: Some(side.project_id),
            ..all()
        })
        .await
        .unwrap();
    assert!(
        found.is_empty(),
        "searching from another project does not reveal it"
    );

    // The active project takes new tasks once chosen.
    client.set_active_project(side.project_id).await.unwrap();
    fake.push(Reply::text(&["ok"]));
    let defaulted = send(&client, None, "s3", "Încă un task").await;
    settled(&client, &defaulted).await;
    let summaries = client.conversations(10).await.unwrap();
    assert_eq!(
        summaries
            .iter()
            .find(|c| c.conversation_id == defaulted.conversation_id)
            .unwrap()
            .project_id,
        Some(side.project_id)
    );
    host.shutdown().await;
}

#[tokio::test]
async fn at_most_three_facts_are_proposed_per_reply() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    let calls: Vec<(&str, serde_json::Value)> = (0..5)
        .map(|i| {
            (
                "propose_memory",
                serde_json::json!({"content": format!("fact {i}"), "scope": "project"}),
            )
        })
        .collect();
    fake.push(Reply::calls(&calls));
    fake.push(Reply::text(&["done"]));
    let sent = send(&client, None, "f1", "Remember five things").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    let proposed = client.memory(all()).await.unwrap();
    assert_eq!(proposed.len(), 3, "extra proposals are ignored");
    // The follow-up request no longer offers the tool.
    let tools = fake.generations().pop().unwrap().json()["tools"].to_string();
    assert!(!tools.contains("propose_memory"), "{tools}");
    host.shutdown().await;
}

#[tokio::test]
async fn corrections_and_deletions_change_exactly_what_is_sent() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    let fact = client
        .add_memory(AddMemoryRequest {
            scope: MemoryScope::Personal,
            project_id: None,
            content: "My name is Ana.".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        fact.status,
        MemoryStatus::Active,
        "the user's own fact is active at once"
    );
    let fixed = client
        .edit_memory(
            fact.memory_id,
            fact.revision,
            "My name is Ana Maria.".into(),
        )
        .await
        .unwrap();
    let stale = client
        .edit_memory(fact.memory_id, fact.revision, "My name is Bob.".into())
        .await
        .unwrap_err();
    assert_eq!(stale.code(), Some(ErrorCode::RevisionConflict));

    fake.push(Reply::text(&["ok"]));
    let sent = send(&client, None, "e1", "Cum mă cheamă?").await;
    settled(&client, &sent).await;
    assert!(last_instruction(&fake).contains("My name is Ana Maria."));

    client.delete_memory(fixed.memory_id).await.unwrap();
    fake.push(Reply::text(&["ok"]));
    let again = send(&client, Some(sent.conversation_id), "e2", "Și acum?").await;
    settled(&client, &again).await;
    assert!(!last_instruction(&fake).contains("Ana"));
    assert!(
        client
            .memory(MemoryQuery {
                text: Some("ana".into()),
                ..all()
            })
            .await
            .unwrap()
            .is_empty()
    );
    host.shutdown().await;
}

#[tokio::test]
async fn invalid_memory_requests_are_refused() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    let main = client.projects().await.unwrap()[0].project_id;
    for (scope, project, content) in [
        (MemoryScope::Project, None, "no project"),
        (MemoryScope::Personal, Some(main), "personal with a project"),
        (MemoryScope::Personal, None, "   "),
    ] {
        let err = client
            .add_memory(AddMemoryRequest {
                scope,
                project_id: project,
                content: content.into(),
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::InvalidRequest), "{content}");
    }
    let long = "x".repeat(1001);
    assert!(
        client
            .add_memory(AddMemoryRequest {
                scope: MemoryScope::Personal,
                project_id: None,
                content: long,
            })
            .await
            .is_err()
    );
    let empty_name = client
        .save_project(SaveProjectRequest {
            project_id: None,
            name: " ".into(),
            description: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(empty_name.code(), Some(ErrorCode::InvalidRequest));
    assert!(client.memory(all()).await.unwrap().is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn a_refused_page_is_not_recorded_as_an_influence() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    // The loopback page is refused by the real SSRF gate, so nothing was
    // read; a proposal that follows names no page as its influence.
    fake.push(Reply::calls(&[(
        "fetch_url",
        serde_json::json!({"url": "http://127.0.0.1:1/private"}),
    )]));
    fake.push(Reply::calls(&[(
        "propose_memory",
        serde_json::json!({"content": "The user reads that page.", "scope": "personal"}),
    )]));
    fake.push(Reply::text(&["ok"]));
    let sent = send(&client, None, "i1", "Citește pagina").await;
    settled(&client, &sent).await;
    let proposals = client.memory(all()).await.unwrap();
    assert_eq!(proposals.len(), 1);
    assert!(
        proposals[0].influences.is_empty(),
        "{:?}",
        proposals[0].influences
    );
    host.shutdown().await;
}
