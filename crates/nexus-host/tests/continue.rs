//! A task continues after the user decides on what its reply proposed
//! (`PLAN_NEXUS_RO.md`, step 4): the reply that proposed a note stops, the
//! note waits for the user, and once the user decides the model hears the
//! outcome as that call's result and carries on — once, even across a host
//! restart, and without keeping external data while it waits. A local
//! Gemini stand-in records exactly what would have left the PC.

mod support;

use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{
    AgentStepDetail, ApprovalDecision, ChatMessageView, ChatRole, ChatStatus, ConversationId,
    DecideApprovalRequest, RunId, RunStatus, RunView, SaveMcpServerRequest,
};
use serde_json::{Value, json};
use support::TestHost;
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply, signature_for};
use support::fake_mcp::{FakeMcp, tool};

const TOKEN: &str = "continue-token";
const KEY: &str = "SENTINEL-gemini-key-continue-2b8d";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(10),
        retry_waits: Vec::new(),
        allow_loopback_mcp: true,
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

async fn wait_run(client: &NexusClient, run: RunId, wanted: RunStatus) -> RunView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let view = client.run(run).await.unwrap();
        if view.status == wanted {
            return view;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run stayed {:?}, expected {wanted:?}",
            view.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn decide(client: &NexusClient, run: RunId, decision: ApprovalDecision) {
    let pending = wait_run(client, run, RunStatus::AwaitingApproval)
        .await
        .pending_approval
        .expect("it waits for the user");
    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision,
        })
        .await
        .unwrap();
}

/// The conversation once it has `count` messages and none is streaming.
async fn messages(client: &NexusClient, id: ConversationId, count: usize) -> Vec<ChatMessageView> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let view = client.conversation(id).await.unwrap();
        if view.messages.len() == count
            && view
                .messages
                .iter()
                .all(|m| m.status != ChatStatus::Streaming)
        {
            return view.messages;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the conversation has {} messages: {:?}",
            view.messages.len(),
            view.messages
                .iter()
                .map(|m| (m.role, m.status, m.text.clone()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn roles(body: &Value) -> Vec<&str> {
    body["contents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["role"].as_str().unwrap())
        .collect()
}

/// The function response the request carries for `name`.
fn answer<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["contents"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["parts"].as_array().unwrap())
        .find(|p| p["functionResponse"]["name"] == name)
        .map(|p| &p["functionResponse"]["response"])
        .unwrap_or_else(|| panic!("no answer for {name} in {body}"))
}

fn propose(title: &str) -> Reply {
    Reply::calls(&[("propose_note", json!({"title": title, "body": "step 1"}))])
}

#[tokio::test]
async fn an_approved_note_lets_the_task_continue_with_its_outcome() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(propose("Plan"));
    fake.push(Reply::text(&["Saved it as Plan."]));
    let sent = send(&client, None, "c1", "Save the plan").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    let run = reply.proposed_run_id.expect("the note waits as a run");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        fake.generations().len(),
        1,
        "nothing continues before the user decides"
    );

    decide(&client, run, ApprovalDecision::Approve).await;
    wait_run(&client, run, RunStatus::Succeeded).await;
    let all = messages(&client, sent.conversation_id, 3).await;
    let continued = &all[2];
    assert_eq!(continued.role, ChatRole::Model);
    assert_eq!(continued.status, ChatStatus::Complete);
    assert_eq!(continued.text, "Saved it as Plan.");
    assert!(
        continued
            .steps
            .iter()
            .any(|s| matches!(s.detail, AgentStepDetail::ModelCall { .. })),
        "the continuation's model call is its own step"
    );

    let generations = fake.generations();
    assert_eq!(generations.len(), 2);
    let body = generations[1].json();
    assert_eq!(roles(&body), ["user", "model", "user"]);
    let call = &body["contents"][1]["parts"][0];
    assert_eq!(call["functionCall"]["name"], "propose_note");
    assert_eq!(
        call["thoughtSignature"],
        signature_for("propose_note").as_str(),
        "the call goes back with the signature it came with"
    );
    let outcome = answer(&body, "propose_note");
    assert_eq!(outcome["status"], "written");
    assert!(outcome["file"].as_str().unwrap().ends_with(".md"));
    assert!(
        body["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("continue the task")
    );

    // The next request sees the continuation as the model's answer.
    fake.push(Reply::text(&["You're welcome."]));
    let next = send(&client, Some(sent.conversation_id), "c2", "Thanks").await;
    settled(&client, &next).await;
    let body = fake.generations().pop().unwrap().json();
    assert_eq!(roles(&body), ["user", "model", "user"]);
    assert_eq!(body["contents"][1]["parts"][0]["text"], "Saved it as Plan.");

    // Deleting the request takes its reply and the continuation with it.
    let deleted = client.delete_exchange(sent.reply_message_id).await.unwrap();
    assert_eq!(deleted.messages, 3);
    assert!(!deleted.conversation_deleted);
    let left = client.conversation(sent.conversation_id).await.unwrap();
    assert_eq!(left.messages.len(), 2);
    host.shutdown().await;
}

#[tokio::test]
async fn a_rejected_note_is_reported_and_the_task_goes_on() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(propose("Plan"));
    fake.push(Reply::text(&["Understood, I won't save it."]));
    let sent = send(&client, None, "r1", "Save the plan").await;
    let run = settled(&client, &sent).await.proposed_run_id.unwrap();
    decide(&client, run, ApprovalDecision::Deny).await;
    let all = messages(&client, sent.conversation_id, 3).await;
    assert_eq!(all[2].text, "Understood, I won't save it.");
    let body = fake.generations().pop().unwrap().json();
    assert_eq!(answer(&body, "propose_note")["status"], "rejected");
    host.shutdown().await;
}

#[tokio::test]
async fn a_new_request_before_the_decision_means_the_task_does_not_continue() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(propose("Plan"));
    fake.push(Reply::text(&["Something else, then."]));
    let sent = send(&client, None, "m1", "Save the plan").await;
    let run = settled(&client, &sent).await.proposed_run_id.unwrap();
    let other = send(&client, Some(sent.conversation_id), "m2", "Never mind").await;
    settled(&client, &other).await;

    decide(&client, run, ApprovalDecision::Approve).await;
    wait_run(&client, run, RunStatus::Succeeded).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    messages(&client, sent.conversation_id, 4).await;
    assert_eq!(fake.generations().len(), 2, "no continuation was started");
    host.shutdown().await;
}

#[tokio::test]
async fn a_waiting_task_survives_a_restart_and_keeps_no_external_data() {
    const EXTERNAL: &str = "SENTINEL-external-data-7f1";
    let fake = FakeGemini::start(KEY).await;
    let mcp = FakeMcp::start(None, vec![tool("docs_search", "Search the docs", true)]).await;
    mcp.answer("docs_search", EXTERNAL);
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;
    let server = client
        .save_mcp_server(SaveMcpServerRequest {
            server_id: None,
            name: "Docs".to_string(),
            url: mcp.url.clone(),
            key: None,
            clear_key: false,
        })
        .await
        .unwrap();
    client
        .set_mcp_tool_enabled(
            server.server_id,
            "docs_search".to_string(),
            Some(server.tools[0].definition_sha256.clone()),
        )
        .await
        .unwrap();

    fake.push(Reply::calls(&[("mcp1_docs_search", json!({"q": "pipes"}))]));
    fake.push(propose("Pipes"));
    let sent = send(&client, None, "s1", "Look it up and save a note").await;
    let run = settled(&client, &sent).await.proposed_run_id.unwrap();
    assert!(
        fake.generations()[1].body.contains(EXTERNAL),
        "the model did get the result while answering"
    );

    let profile = host.shutdown().await;
    let mut stored = Vec::new();
    for name in ["nexus.db", "nexus.db-wal"] {
        if let Ok(bytes) = std::fs::read(profile.path().join(name)) {
            stored.extend(bytes);
        }
    }
    assert!(
        !String::from_utf8_lossy(&stored).contains(EXTERNAL),
        "the waiting task keeps no external data on disk"
    );

    let host = TestHost::start_in(profile, TOKEN, config(&fake)).await;
    let client = host.client().await;
    fake.push(Reply::text(&["Saved."]));
    decide(&client, run, ApprovalDecision::Approve).await;
    let all = messages(&client, sent.conversation_id, 3).await;
    assert_eq!(all[2].text, "Saved.");
    let body = fake.generations().pop().unwrap().json();
    assert_eq!(roles(&body), ["user", "model", "user", "model", "user"]);
    assert!(!body.to_string().contains(EXTERNAL));
    assert_eq!(answer(&body, "propose_note")["status"], "written");
    assert!(
        answer(&body, "mcp1_docs_search")["note"]
            .as_str()
            .unwrap()
            .contains("not kept"),
        "the earlier result is replaced, not resent"
    );
    host.shutdown().await;
}
