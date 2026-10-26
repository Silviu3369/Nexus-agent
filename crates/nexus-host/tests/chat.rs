//! M2 chat over real RPC against a controlled Gemini stand-in on loopback
//! (`SECURITATE_SI_VALIDARE_RO.md` AI-01..03). No real provider is called;
//! the fake records exactly what would have left the PC.

mod support;

use std::path::Path;
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_host::vault::SecretVault;
use nexus_protocol::{
    AgentStepDetail, AgentStepStatus, ApprovalDecision, ChatRole, ChatStatus,
    DecideApprovalRequest, ErrorCode, RunId, RunStatus, RunView, SavePersonaRequest,
    SendMessageRequest, ThinkingLevel,
};
use support::chat::{files_under, gemini, message, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply, text_event};
use support::{TempProfile, TestHost};

const TOKEN: &str = "chat-token";
const KEY: &str = "SENTINEL-gemini-key-host-5c1e";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(10),
        // Each queued fake reply answers one try; tests of retries opt in.
        retry_waits: Vec::new(),
        ..RuntimeConfig::default()
    }
}

/// A connection with the fake's model selected, ready to chat.
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

fn note_files(profile: &Path) -> Vec<std::path::PathBuf> {
    files_under(&profile.join("artifacts"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect()
}

fn roles(body: &serde_json::Value) -> Vec<String> {
    body["contents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["role"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn ai01_reply_streams_from_the_selected_model_with_persona_and_history() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;

    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    // The tests' key is a paid one (what a free one may receive depends on
    // where the user is: tests/data_policy.rs).
    assert!(saved.has_key && saved.active && !saved.free_tier);
    assert_eq!(saved.model, None);
    let models = client.list_models(saved.connection_id).await.unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        [MODEL],
        "only models that can generate text are offered"
    );
    let selected = client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
    assert_eq!(selected.model.as_deref(), Some(MODEL));

    fake.push(Reply::text(&["Bună! ", "Sunt bine."]));
    let first = send(&client, None, "m1", "Salut, ce faci?").await;
    let reply = settled(&client, &first).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    assert_eq!(reply.role, ChatRole::Model);
    assert_eq!(reply.text, "Bună! Sunt bine.");
    assert_eq!(reply.model.as_deref(), Some(MODEL));
    assert_eq!(reply.persona_version, Some(1));
    assert_eq!(
        (reply.prompt_tokens, reply.output_tokens),
        (Some(11), Some(5))
    );

    let sent = fake.generations().remove(0);
    assert_eq!(
        sent.target,
        format!("/v1beta/models/{MODEL}:streamGenerateContent?alt=sse")
    );
    assert_eq!(sent.header("x-goog-api-key"), Some(KEY));
    assert!(!sent.target.contains(KEY) && !sent.body.contains(KEY));
    let body = sent.json();
    let system = body["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(system.contains("You are Nexus"));
    assert!(system.contains("language of the user's latest message"));
    assert_eq!(roles(&body), ["user"]);
    assert_eq!(body["contents"][0]["parts"][0]["text"], "Salut, ce faci?");
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["name"],
        "propose_note"
    );

    // The next turn carries the conversation so far.
    fake.push(Reply::text(&["Desigur."]));
    let second = send(
        &client,
        Some(first.conversation_id),
        "m2",
        "Poți fi mai scurt?",
    )
    .await;
    assert_eq!(settled(&client, &second).await.text, "Desigur.");
    let body = fake.generations().remove(1).json();
    assert_eq!(roles(&body), ["user", "model", "user"]);
    assert_eq!(body["contents"][1]["parts"][0]["text"], "Bună! Sunt bine.");

    // A new persona version changes the voice of later replies only.
    let persona = client
        .save_persona(SavePersonaRequest {
            name: "Ada".to_string(),
            tone: "caldă".to_string(),
            style: "fraze scurte".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(persona.version, 2);
    fake.push(Reply::text(&["Gata."]));
    let third = send(&client, Some(first.conversation_id), "m3", "Mulțumesc").await;
    assert_eq!(settled(&client, &third).await.persona_version, Some(2));
    let body = fake.generations().remove(2).json();
    assert!(
        body["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("You are Ada")
    );
    assert_eq!(
        message(&client, first.conversation_id, first.reply_message_id)
            .await
            .persona_version,
        Some(1),
        "earlier replies keep the persona they were written with"
    );

    // A repeated send (same key, same text) is the same turn: no second call.
    let again = send(&client, Some(first.conversation_id), "m3", "Mulțumesc").await;
    assert_eq!(again.reply_message_id, third.reply_message_id);
    let conflict = client
        .send_message(SendMessageRequest {
            conversation_id: Some(first.conversation_id),
            project_id: None,
            idempotency_key: "m3".to_string(),
            text: "alt text".to_string(),
        })
        .await;
    assert!(conflict.is_err(), "a reused key with other text is refused");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fake.generations().len(), 3);

    let conversations = client.conversations(10).await.unwrap();
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].conversation_id, first.conversation_id);
    host.shutdown().await;
}

#[tokio::test]
async fn ai02_a_proposed_note_waits_for_approval_and_other_tools_are_ignored() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::calls(&[
        (
            "run_shell",
            serde_json::json!({"command": "del /s C:\\Users"}),
        ),
        (
            "propose_note",
            serde_json::json!({"title": "Plan", "body": "pasul 1"}),
        ),
        (
            "propose_note",
            serde_json::json!({"title": "Al doilea", "body": "x"}),
        ),
    ]));
    let sent = send(&client, None, "n1", "Salvează planul ca notă").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    let run = reply
        .proposed_run_id
        .expect("the note proposal becomes a run");

    let waiting = wait_run(&client, run, RunStatus::AwaitingApproval).await;
    let origin = waiting
        .origin
        .clone()
        .expect("the run names the reply that proposed it");
    assert_eq!(origin.message_id, sent.reply_message_id);
    assert_eq!(origin.conversation_id, sent.conversation_id);
    assert_eq!(origin.conversation_title, "Salvează planul ca notă");
    let actor = |kind: &str, name: &str| {
        Some(nexus_protocol::ActorView {
            kind: kind.into(),
            name: name.into(),
        })
    };
    assert_eq!(
        (waiting.requested_by.clone(), waiting.decided_by.clone()),
        (actor("Agent", "Nexus"), None),
        "the agent asked for it; nobody decided yet"
    );
    assert_eq!(
        client
            .conversation(sent.conversation_id)
            .await
            .unwrap()
            .requested_by,
        actor("User", "Local user"),
        "the user asked for the task"
    );
    let pending = waiting.pending_approval.expect("it waits for the user");
    let wait = pending.expires_at_ms - waiting.updated_at_ms;
    assert!(
        wait > 23 * 3_600_000 && wait <= 24 * 3_600_000,
        "a proposal waits a day for the user's decision (ADR-POLICY-001): {wait} ms"
    );
    assert_eq!(pending.preview.unwrap().content, "# Plan\n\npasul 1\n");
    assert!(
        note_files(host.profile.path()).is_empty(),
        "nothing is written on the model's word"
    );
    let snapshot = client.snapshot(50).await.unwrap();
    assert_eq!(
        snapshot.runs.len(),
        1,
        "one proposal per reply; tools the host does not offer are ignored"
    );
    assert_eq!(
        snapshot.runs[0].origin.as_ref().map(|o| o.message_id),
        Some(sent.reply_message_id),
        "the snapshot carries the origin too"
    );

    // A note the user submits directly has no agent origin.
    let direct = client
        .submit_note(nexus_protocol::SubmitNoteRequest {
            idempotency_key: "direct-note".into(),
            title: "Mine".into(),
            body: "typed by me".into(),
        })
        .await
        .unwrap();
    let direct = client.run(direct.run_id).await.unwrap();
    assert!(direct.origin.is_none());
    assert_eq!(direct.requested_by, actor("User", "Local user"));

    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
            via: nexus_protocol::ApprovalPath::Window,
        })
        .await
        .unwrap();
    let done = wait_run(&client, run, RunStatus::Succeeded).await;
    assert_eq!(
        (done.requested_by, done.decided_by),
        (actor("Agent", "Nexus"), actor("User", "Local user")),
        "the dashboard's session decided: the local user"
    );
    assert_eq!(note_files(host.profile.path()).len(), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn ai03_without_a_chosen_connection_and_model_nothing_is_sent_and_there_is_no_fallback() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;

    let refused = client
        .send_message(SendMessageRequest {
            conversation_id: None,
            project_id: None,
            idempotency_key: "a".to_string(),
            text: "Salut".to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::InvalidRequest));
    assert!(
        fake.received().is_empty(),
        "no connection: no request at all"
    );

    // A connection without a chosen model does not pick one by itself.
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    let refused = client
        .send_message(SendMessageRequest {
            conversation_id: None,
            project_id: None,
            idempotency_key: "b".to_string(),
            text: "Salut".to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::InvalidRequest));

    // Removing the only connection removes its key and leaves nothing to fall back on.
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
    client.remove_connection(saved.connection_id).await.unwrap();
    assert!(client.list_connections().await.unwrap().is_empty());
    let refused = client
        .send_message(SendMessageRequest {
            conversation_id: None,
            project_id: None,
            idempotency_key: "c".to_string(),
            text: "Salut".to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::InvalidRequest));
    assert!(fake.generations().is_empty(), "nothing was ever generated");
    assert_eq!(
        SecretVault::for_profile(host.profile.path())
            .purge()
            .unwrap(),
        0,
        "the removed connection's key is gone from the vault"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_key_the_provider_rejects_is_never_stored_or_echoed() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;

    let wrong = format!("WRONG-{}", uuid::Uuid::new_v4());
    let refused = client.save_connection(gemini(&wrong)).await.unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::Unauthenticated));
    let text = format!("{refused} {refused:?}");
    assert!(
        !text.contains(&wrong),
        "the refused key leaked into: {text}"
    );
    assert!(client.list_connections().await.unwrap().is_empty());
    assert_eq!(
        SecretVault::for_profile(host.profile.path())
            .purge()
            .unwrap(),
        0
    );

    let malformed = client
        .save_connection(gemini("has spaces inside"))
        .await
        .unwrap_err();
    assert_eq!(malformed.code(), Some(ErrorCode::InvalidRequest));
    host.shutdown().await;
}

#[tokio::test]
async fn provider_failures_settle_the_reply_with_a_stable_code() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(
        TOKEN,
        RuntimeConfig {
            reply_deadline: Duration::from_millis(600),
            ..config(&fake)
        },
    )
    .await;
    let client = host.client().await;
    connect_gemini(&client).await;

    let cases = [
        (
            Reply::Status {
                code: 429,
                headers: vec![("retry-after", "17".to_string())],
                body: "{}".to_string(),
            },
            "QuotaExceeded",
        ),
        (
            Reply::status(503, r#"{"error":{"message":"overloaded"}}"#),
            "ProviderUnavailable",
        ),
        (
            Reply::status(
                401,
                &format!(r#"{{"error":{{"message":"key {KEY} expired"}}}}"#),
            ),
            "Unauthenticated",
        ),
        (
            Reply::Sse(vec!["{not json".to_string()]),
            "ProviderUnavailable",
        ),
        (Reply::Hang, "DeadlineExceeded"),
    ];
    let mut conversation = None;
    for (i, (reply, code)) in cases.into_iter().enumerate() {
        fake.push(reply);
        let sent = send(&client, conversation, &format!("f{i}"), "Salut").await;
        conversation = Some(sent.conversation_id);
        let settled = settled(&client, &sent).await;
        assert_eq!(settled.status, ChatStatus::Failed, "case {code}");
        let failure = settled.failure.expect("a failed reply says why");
        assert_eq!(failure.code, code);
        assert!(
            !failure.message.contains(KEY),
            "the key leaked into: {}",
            failure.message
        );
        if code == "QuotaExceeded" {
            assert!(
                failure.message.contains("retry after 17 s"),
                "{}",
                failure.message
            );
        }
    }

    // Failed replies are not sent back as context.
    fake.push(Reply::text(&["În sfârșit."]));
    let sent = send(&client, conversation, "ok", "Mai încerc").await;
    assert_eq!(settled(&client, &sent).await.status, ChatStatus::Complete);
    let body = fake.generations().pop().unwrap().json();
    assert!(roles(&body).iter().all(|r| r == "user"));
    host.shutdown().await;
}

#[tokio::test]
async fn each_model_call_has_its_own_limit_inside_the_reply_limit() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(
        TOKEN,
        RuntimeConfig {
            reply_deadline: Duration::from_secs(30),
            step_deadline: Duration::from_secs(2),
            ..config(&fake)
        },
    )
    .await;
    let client = host.client().await;
    connect_gemini(&client).await;

    // Two steps that think 1.2 s each: together past one call's limit,
    // which therefore cannot be the limit of the whole reply.
    let slow = Duration::from_millis(1200);
    fake.push(Reply::Slow(
        slow,
        Box::new(Reply::calls(&[(
            "fetch_url",
            serde_json::json!({"url": "http://127.0.0.1:1/private"}),
        )])),
    ));
    fake.push(Reply::Slow(
        slow,
        Box::new(Reply::text(&["Nu am putut accesa pagina."])),
    ));
    let started = std::time::Instant::now();
    let sent = send(&client, None, "slow", "Citește pagina").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete, "{:?}", reply.failure);
    assert_eq!(fake.generations().len(), 2);
    assert!(started.elapsed() >= 2 * slow);

    // A call that never answers is cut at its own limit, long before the
    // reply's.
    fake.push(Reply::Hang);
    let started = std::time::Instant::now();
    let sent = send(&client, Some(sent.conversation_id), "hang", "Încă o dată").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Failed);
    assert_eq!(reply.failure.unwrap().code, "DeadlineExceeded");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "cut after {:?}",
        started.elapsed()
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_call_cut_at_its_limit_is_tried_again_only_while_the_reply_has_time() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(
        TOKEN,
        RuntimeConfig {
            reply_deadline: Duration::from_secs(30),
            step_deadline: Duration::from_secs(1),
            retry_waits: vec![Duration::from_millis(100)],
            ..config(&fake)
        },
    )
    .await;
    let client = host.client().await;
    connect_gemini(&client).await;

    // Nothing came back in the call's second: it is tried again.
    fake.push(Reply::Hang);
    fake.push(Reply::text(&["Al doilea apel a răspuns."]));
    let sent = send(&client, None, "again", "Salut").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete, "{:?}", reply.failure);
    assert_eq!(reply.text, "Al doilea apel a răspuns.");
    assert_eq!(fake.generations().len(), 2);
    let calls: Vec<AgentStepStatus> = reply
        .steps
        .iter()
        .filter(|s| matches!(s.detail, AgentStepDetail::ModelCall { .. }))
        .map(|s| s.status)
        .collect();
    assert_eq!(calls, [AgentStepStatus::Failed, AgentStepStatus::Done]);
    host.shutdown().await;

    // With the reply's time spent, the same cut is final.
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(
        TOKEN,
        RuntimeConfig {
            reply_deadline: Duration::from_millis(1500),
            step_deadline: Duration::from_secs(1),
            retry_waits: vec![Duration::from_millis(100)],
            ..config(&fake)
        },
    )
    .await;
    let client = host.client().await;
    connect_gemini(&client).await;
    fake.push(Reply::Hang);
    let sent = send(&client, None, "spent", "Salut").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Failed);
    assert_eq!(reply.failure.unwrap().code, "DeadlineExceeded");
    assert_eq!(fake.generations().len(), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn cancelling_a_reply_keeps_what_arrived_and_stops_the_call() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::SseThenHang(vec![text_event("Început ")]));
    let sent = send(&client, None, "c1", "Scrie un eseu lung").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let reply = message(&client, sent.conversation_id, sent.reply_message_id).await;
        assert_eq!(reply.status, ChatStatus::Streaming);
        if reply.text == "Început " {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the partial text never showed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    client.cancel_reply(sent.reply_message_id).await.unwrap();
    let reply = message(&client, sent.conversation_id, sent.reply_message_id).await;
    assert_eq!(reply.status, ChatStatus::Cancelled);
    assert_eq!(reply.text, "Început ");
    assert_eq!(reply.failure.unwrap().code, "Cancelled");
    assert!(
        client.cancel_reply(sent.reply_message_id).await.is_err(),
        "a settled reply cannot be cancelled again"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        message(&client, sent.conversation_id, sent.reply_message_id)
            .await
            .status,
        ChatStatus::Cancelled,
        "the aborted call cannot overwrite the cancellation"
    );
    host.shutdown().await;
}

// Must match MAX_FETCH_CALLS / MAX_MCP_CALLS / RECORD_STEPS / FILE_STEPS / MAX_AGENT_STEPS in
// nexus-host/src/runtime/chat.rs.
const MAX_FETCH_CALLS: usize = 4;
const MAX_MCP_CALLS: usize = 4;
/// Plus the model calls a reply may spend on the user's registers.
const RECORD_STEPS: usize = 4;
/// Plus the model calls a reply may spend on shared folders.
const FILE_STEPS: usize = 4;
const MAX_AGENT_STEPS: usize = MAX_FETCH_CALLS + MAX_MCP_CALLS + RECORD_STEPS + FILE_STEPS + 2;

#[tokio::test]
async fn a_refused_fetch_is_reported_back_to_the_model_which_then_answers() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    // Any URL fetch_url is offered is loopback relative to the host, so the
    // real SSRF gate refuses it — this proves the loop and the gate both
    // run for real, not that a fetch can succeed in this harness.
    fake.push(Reply::calls(&[(
        "fetch_url",
        serde_json::json!({"url": "http://127.0.0.1:1/private"}),
    )]));
    fake.push(Reply::text(&["Nu am putut accesa acea pagină."]));

    let sent = send(&client, None, "f1", "Caută ceva pe acea pagină").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    assert_eq!(reply.text, "Nu am putut accesa acea pagină.");

    let generations = fake.generations();
    assert_eq!(
        generations.len(),
        2,
        "the loop must call the model a second time"
    );
    let second_body = generations[1].json();
    assert_eq!(roles(&second_body), ["user", "model", "user"]);
    let function_response = &second_body["contents"][2]["parts"][0]["functionResponse"];
    assert_eq!(function_response["name"], "fetch_url");
    assert_eq!(
        second_body["contents"][1]["parts"][0]["thoughtSignature"],
        support::fake_gemini::signature_for("fetch_url").as_str(),
        "the call goes back with the signature it came with (Gemini 3 answers 400 without it)"
    );
    assert!(
        function_response["response"]["error"].is_string(),
        "a refused fetch must come back as an error, not silently dropped: {function_response}"
    );
    assert!(
        !function_response["response"]["error"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    host.shutdown().await;
}

#[tokio::test]
async fn the_fetch_budget_is_enforced_then_the_tool_is_withdrawn() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    // The model never stops asking to fetch; the host must stop it anyway.
    for _ in 0..MAX_AGENT_STEPS {
        fake.push(Reply::calls(&[(
            "fetch_url",
            serde_json::json!({"url": "http://127.0.0.1:1/private"}),
        )]));
    }

    let sent = send(&client, None, "f2", "Continuă să cauți oricât").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(
        reply.status,
        ChatStatus::Complete,
        "an exhausted step budget ends the turn, it is not a failure"
    );

    let generations = fake.generations();
    assert_eq!(
        generations.len(),
        MAX_AGENT_STEPS,
        "exactly the step budget worth of calls, not one more"
    );
    for (i, g) in generations.iter().enumerate() {
        let tools = &g.json()["tools"][0]["functionDeclarations"];
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        if i < MAX_FETCH_CALLS {
            assert!(
                names.contains(&"fetch_url"),
                "call {i} must still offer fetch_url"
            );
        } else {
            assert!(
                !names.contains(&"fetch_url"),
                "call {i} must not offer fetch_url once the budget is spent"
            );
        }
        assert!(
            names.contains(&"propose_note"),
            "propose_note is always offered"
        );
    }
    let fetches = reply
        .steps
        .iter()
        .filter(|s| matches!(s.detail, AgentStepDetail::Fetch { .. }))
        .count();
    assert_eq!(
        fetches, MAX_FETCH_CALLS,
        "a fetch asked for after it was withdrawn is answered with an error, not performed"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_note_proposed_alongside_a_fetch_ends_the_turn_on_the_note_alone() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::calls(&[
        (
            "fetch_url",
            serde_json::json!({"url": "http://127.0.0.1:1/x"}),
        ),
        (
            "propose_note",
            serde_json::json!({"title": "Găsit", "body": "conținut"}),
        ),
    ]));

    let sent = send(&client, None, "f3", "Caută și salvează ce găsești").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    let run = reply
        .proposed_run_id
        .expect("the note proposal still becomes a run");
    wait_run(&client, run, RunStatus::AwaitingApproval).await;

    assert_eq!(
        fake.generations().len(),
        1,
        "a note in the same step ends the turn; fetch_url is not also run"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn every_agent_step_is_recorded_and_shown_in_the_reply_and_the_activity() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    // Model asks to fetch (refused: loopback), sees why, then proposes a note.
    fake.push(Reply::calls(&[(
        "fetch_url",
        serde_json::json!({"url": "http://127.0.0.1:1/private"}),
    )]));
    fake.push(Reply::calls(&[(
        "propose_note",
        serde_json::json!({"title": "Rezumat", "body": "nu am putut citi pagina"}),
    )]));
    let sent = send(&client, None, "s1", "Citește pagina și notează").await;
    let reply = settled(&client, &sent).await;
    let run = reply.proposed_run_id.expect("the note became a run");

    let kinds: Vec<&str> = reply
        .steps
        .iter()
        .map(|s| match s.detail {
            AgentStepDetail::ModelCall { .. } => "model",
            AgentStepDetail::Fetch { .. } => "fetch",
            AgentStepDetail::NoteProposed { .. } => "note",
            AgentStepDetail::MemoryProposed { .. } => "memory",
            AgentStepDetail::McpCall { .. } => "mcp",
            AgentStepDetail::RecordsFound { .. } | AgentStepDetail::RecordChanged { .. } => {
                "record"
            }
            AgentStepDetail::FilesListed { .. }
            | AgentStepDetail::DocumentRead { .. }
            | AgentStepDetail::DocumentProposed { .. } => "document",
        })
        .collect();
    assert_eq!(kinds, ["model", "fetch", "model", "note"]);
    assert_eq!(
        reply.steps.iter().map(|s| s.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert!(reply.steps.iter().all(|s| s.finished_at_ms.is_some()));

    match &reply.steps[0].detail {
        AgentStepDetail::ModelCall {
            model,
            duration_ms,
            error,
            ..
        } => {
            assert_eq!(model, MODEL);
            assert!(duration_ms.is_some(), "the call is timed");
            assert!(error.is_none());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(reply.steps[1].status, AgentStepStatus::Failed);
    match &reply.steps[1].detail {
        AgentStepDetail::Fetch {
            url,
            error,
            http_status,
            ..
        } => {
            assert_eq!(url, "http://127.0.0.1:1/private");
            assert!(
                error.as_deref().is_some_and(|e| !e.is_empty()),
                "the refusal says why"
            );
            assert_eq!(*http_status, None, "nothing was fetched");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(reply.steps[3].status, AgentStepStatus::Done);
    match &reply.steps[3].detail {
        AgentStepDetail::NoteProposed { title, run_id, .. } => {
            assert_eq!(title, "Rezumat");
            assert_eq!(
                *run_id,
                Some(run),
                "the step points at the run awaiting approval"
            );
        }
        other => panic!("{other:?}"),
    }

    let activity = client.activity(20).await.unwrap();
    assert_eq!(
        activity.iter().map(|s| s.step_id).collect::<Vec<_>>(),
        reply
            .steps
            .iter()
            .rev()
            .map(|s| s.step_id)
            .collect::<Vec<_>>(),
        "the dashboard feed shows the same steps, newest first"
    );
    assert!(
        activity
            .iter()
            .all(|s| s.conversation_id == sent.conversation_id)
    );
    assert_eq!(activity[0].conversation_title, "Citește pagina și notează");

    let user_message = message(&client, sent.conversation_id, sent.user_message_id).await;
    assert!(user_message.steps.is_empty(), "steps belong to the reply");
    host.shutdown().await;
}

#[tokio::test]
async fn a_failed_or_stopped_model_call_is_a_failed_step_not_a_running_one() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::status(503, r#"{"error":{"message":"overloaded"}}"#));
    let sent = send(&client, None, "e1", "Salut").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Failed);
    assert_eq!(reply.steps.len(), 1);
    assert_eq!(reply.steps[0].status, AgentStepStatus::Failed);
    match &reply.steps[0].detail {
        AgentStepDetail::ModelCall { error, .. } => {
            assert!(
                error.as_deref().is_some_and(|e| e.contains("overloaded")),
                "{error:?}"
            );
        }
        other => panic!("{other:?}"),
    }

    fake.push(Reply::SseThenHang(vec![text_event("Început ")]));
    let sent = send(&client, Some(sent.conversation_id), "e2", "Scrie mult").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let live = message(&client, sent.conversation_id, sent.reply_message_id).await;
        if live
            .steps
            .first()
            .is_some_and(|s| s.status == AgentStepStatus::Running)
            && live.text == "Început "
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the running step never showed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    client.cancel_reply(sent.reply_message_id).await.unwrap();
    let stopped = message(&client, sent.conversation_id, sent.reply_message_id).await;
    assert_eq!(stopped.steps.len(), 1);
    assert_eq!(stopped.steps[0].status, AgentStepStatus::Failed);
    match &stopped.steps[0].detail {
        AgentStepDetail::ModelCall { error, .. } => {
            assert_eq!(error.as_deref(), Some("stopped by the user"));
        }
        other => panic!("{other:?}"),
    }
    host.shutdown().await;
}

#[tokio::test]
async fn a_step_left_running_by_a_stopped_host_is_failed_on_restart() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::SseThenHang(vec![text_event("Început ")]));
    let sent = send(&client, None, "r1", "Scrie mult").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !message(&client, sent.conversation_id, sent.reply_message_id)
        .await
        .steps
        .first()
        .is_some_and(|s| s.status == AgentStepStatus::Running)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the step never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(client);
    let profile = host.shutdown().await;

    let host = TestHost::start_in(profile, TOKEN, config(&fake)).await;
    let client = host.client().await;
    let reply = message(&client, sent.conversation_id, sent.reply_message_id).await;
    assert_eq!(
        reply.status,
        ChatStatus::Failed,
        "the reply itself is marked interrupted"
    );
    assert_eq!(reply.steps.len(), 1);
    assert_eq!(reply.steps[0].status, AgentStepStatus::Failed);
    match &reply.steps[0].detail {
        AgentStepDetail::ModelCall { error, .. } => assert_eq!(
            error.as_deref(),
            Some("the host stopped before this step finished")
        ),
        other => panic!("{other:?}"),
    }
    host.shutdown().await;
}

#[tokio::test]
async fn the_chosen_thinking_level_is_sent_for_a_gemini_3_model_only() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    assert_eq!(
        saved.thinking_level,
        ThinkingLevel::Low,
        "a new connection starts fast"
    );

    // select_model only checks the id's character set, so a Gemini-3-shaped
    // id can be used here without the fake catalogue needing to list it.
    client
        .select_model(saved.connection_id, "gemini-3-test-flash".to_string())
        .await
        .unwrap();
    let updated = client
        .set_thinking_level(saved.connection_id, ThinkingLevel::High)
        .await
        .unwrap();
    assert_eq!(updated.thinking_level, ThinkingLevel::High);

    fake.push(Reply::text(&["ok"]));
    let sent = send(&client, None, "t1", "Salut").await;
    settled(&client, &sent).await;
    let body = fake.generations().remove(0).json();
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "HIGH"
    );

    // An older model must not get the field, whatever level is stored.
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
    fake.push(Reply::text(&["ok"]));
    let sent = send(&client, Some(sent.conversation_id), "t2", "Din nou").await;
    settled(&client, &sent).await;
    let body = fake.generations().remove(1).json();
    assert!(
        body["generationConfig"].get("thinkingConfig").is_none(),
        "{body}"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_restored_profile_does_not_contact_the_provider() {
    let fake = FakeGemini::start(KEY).await;
    let profile = TempProfile::new("restored-chat");
    std::fs::write(
        profile.path().join(nexus_host::backup::RESTORED_MARKER),
        "{}",
    )
    .unwrap();
    let host = TestHost::start_in(profile, TOKEN, config(&fake)).await;
    let client = host.client().await;
    let refused = client
        .send_message(SendMessageRequest {
            conversation_id: None,
            project_id: None,
            idempotency_key: "r".to_string(),
            text: "Salut".to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::Forbidden));
    assert!(fake.received().is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn startup_removes_vault_entries_no_connection_refers_to() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    drop(client);
    let profile = host.shutdown().await;

    // A key stored just before a crash, with no connection row yet.
    let vault = SecretVault::for_profile(profile.path());
    vault.store("gemini-orphan", "SENTINEL-orphan").unwrap();

    let host = TestHost::start_in(profile, TOKEN, config(&fake)).await;
    assert!(
        vault.load("gemini-orphan").unwrap().is_none(),
        "an unreferenced key is removed at startup"
    );
    let connections = host.client().await.list_connections().await.unwrap();
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0].connection_id, saved.connection_id);
    assert!(connections[0].has_key, "a referenced key is kept");
    host.shutdown().await;
}
