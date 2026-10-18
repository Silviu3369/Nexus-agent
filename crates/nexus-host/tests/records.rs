//! Registers through the real host (`PLAN_NEXUS_RO.md` §3a, step 1): the
//! job applications register exists with its fields; the user adds, edits
//! and deletes records, with values checked and a stale edit refused; the
//! agent is told the register and today's date, looks records up and saves
//! what the user tells it directly — each change a visible step with its
//! task in the history, undone with one click; a bad value, a duplicate and
//! a flood of writes are refused.

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_host::schedule;
use nexus_protocol::{
    AgentStepDetail, AgentStepStatus, ChatMessageView, ErrorCode, FieldKind, RecordActor,
    RecordChangeKind, RecordQuery, RegisterView, SaveRecordRequest,
};
use serde_json::{Value, json};
use support::TestHost;
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "records-token";
const KEY: &str = "SENTINEL-gemini-key-records-7a10";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(20),
        retry_waits: Vec::new(),
        ..RuntimeConfig::default()
    }
}

async fn connect(client: &NexusClient) {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

async fn jobs(client: &NexusClient) -> RegisterView {
    client
        .registers()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.key == "job_applications")
        .expect("the job applications register exists from the start")
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn all(register: &RegisterView) -> RecordQuery {
    RecordQuery {
        register_id: register.register_id,
        statuses: Vec::new(),
        text: None,
        follow_up_due: false,
        limit: 0,
    }
}

fn today() -> String {
    schedule::local_date(nexus_storage::now_ms(), None)
}

fn yesterday() -> String {
    schedule::local_date(nexus_storage::now_ms() - 24 * 60 * 60 * 1000, None)
}

/// The function response a request carries for `name` (the n-th one).
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

fn declared(body: &Value) -> Vec<String> {
    body["tools"][0]["functionDeclarations"]
        .as_array()
        .map(|t| {
            t.iter()
                .map(|d| d["name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn record_steps(reply: &ChatMessageView) -> Vec<&AgentStepDetail> {
    reply
        .steps
        .iter()
        .map(|s| &s.detail)
        .filter(|d| {
            matches!(
                d,
                AgentStepDetail::RecordsFound { .. } | AgentStepDetail::RecordChanged { .. }
            )
        })
        .collect()
}

#[tokio::test]
async fn the_user_adds_edits_and_deletes_applications_with_values_checked() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let register = jobs(&client).await;
    assert_eq!(register.name, "Job applications");
    assert_eq!(register.status_field, "status");
    assert_eq!(register.follow_up_field.as_deref(), Some("follow_up_on"));
    assert_eq!(register.title_fields, ["company", "role"]);
    let status = register.fields.iter().find(|f| f.key == "status").unwrap();
    assert_eq!(status.kind, FieldKind::Choice);
    assert_eq!(status.choices[..2], ["To apply", "Applied"]);
    assert_eq!(register.record_count, 0);

    let added = client
        .save_record(SaveRecordRequest {
            register_id: register.register_id,
            record_id: None,
            revision: 0,
            values: values(&[
                ("company", "Acme"),
                ("role", "Service Desk Analyst"),
                ("status", "applied"),
                ("applied_on", "2026-10-10"),
                ("follow_up_on", &yesterday()),
                ("link", ""),
            ]),
        })
        .await
        .unwrap();
    assert_eq!(added.values["status"], "Applied");
    assert!(
        !added.values.contains_key("link"),
        "an empty value is not kept"
    );
    assert_eq!(added.created_by, RecordActor::User);
    assert!(added.follow_up_due);
    let register = jobs(&client).await;
    assert_eq!((register.record_count, register.follow_ups_due), (1, 1));

    for bad in [
        values(&[
            ("company", "Acme"),
            ("role", "x"),
            ("applied_on", "10/10/2026"),
        ]),
        values(&[("company", "Acme"), ("role", "x"), ("status", "Hired")]),
        values(&[("company", "Acme")]),
        values(&[("company", "Acme"), ("role", "x"), ("salary_band", "3")]),
    ] {
        let err = client
            .save_record(SaveRecordRequest {
                register_id: register.register_id,
                record_id: None,
                revision: 0,
                values: bad,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::InvalidRequest), "{err}");
    }

    let edit = SaveRecordRequest {
        register_id: register.register_id,
        record_id: Some(added.record_id),
        revision: added.revision,
        values: values(&[
            ("company", "Acme"),
            ("role", "Service Desk Analyst"),
            ("status", "Rejected"),
            ("follow_up_on", &yesterday()),
        ]),
    };
    let edited = client.save_record(edit.clone()).await.unwrap();
    assert_eq!(edited.revision, 2);
    assert!(
        !edited.follow_up_due,
        "a closed application needs no follow-up"
    );
    let stale = client.save_record(edit).await.unwrap_err();
    assert_eq!(stale.code(), Some(ErrorCode::RevisionConflict));

    let found = client
        .records(RecordQuery {
            statuses: vec!["rejected".into()],
            ..all(&register)
        })
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    let detail = client.record_detail(added.record_id).await.unwrap();
    assert_eq!(detail.history.len(), 2);
    assert_eq!(detail.history[0].kind, RecordChangeKind::Updated);
    assert_eq!(detail.history[0].actor, RecordActor::User);
    assert!(detail.history[0].can_undo && !detail.history[1].can_undo);

    client.delete_record(added.record_id).await.unwrap();
    assert!(client.records(all(&register)).await.unwrap().is_empty());
    let gone = client.record_detail(added.record_id).await.unwrap_err();
    assert_eq!(gone.code(), Some(ErrorCode::NotFound));
    host.shutdown().await;
}

#[tokio::test]
async fn the_agent_saves_what_the_user_tells_it_and_the_user_can_undo_it() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let register = jobs(&client).await;

    fake.push(Reply::calls(&[(
        "find_records",
        json!({"register": "job_applications", "text": "Acme"}),
    )]));
    fake.push(Reply::calls(&[(
        "add_record",
        json!({"register": "job_applications", "values": {
            "company": "Acme", "role": "Service Desk Analyst", "status": "Applied",
            "applied_on": today(), "source": "company site", "follow_up_on": null
        }}),
    )]));
    fake.push(Reply::text(&[
        "Saved: Acme, Service Desk Analyst, applied today.",
    ]));
    let sent = send(
        &client,
        None,
        "r1",
        "I applied today at Acme for Service Desk Analyst, on their site.",
    )
    .await;
    let reply = settled(&client, &sent).await;
    assert_eq!(
        reply.text,
        "Saved: Acme, Service Desk Analyst, applied today."
    );

    let generations = fake.generations();
    assert_eq!(generations.len(), 3);
    let first = generations[0].json();
    let system = first["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(system.contains(&format!("Today is {}", today())));
    assert!(system.contains("job_applications (\"Job applications\")"));
    let tools = declared(&first);
    for t in ["find_records", "add_record", "update_record"] {
        assert!(tools.contains(&t.to_string()), "{tools:?}");
    }
    let lookup = answers(&generations[1].json(), "find_records")[0].clone();
    assert_eq!(lookup["matching"], 0);
    assert_eq!(lookup["today"], today());
    let saved = answers(&generations[2].json(), "add_record")[0].clone();
    assert_eq!(saved["saved"], true);

    let records = client.records(all(&register)).await.unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.created_by, RecordActor::Agent);
    assert_eq!(record.values["source"], "company site");
    assert_eq!(saved["record_id"], record.record_id.to_string());

    let steps = record_steps(&reply);
    assert!(matches!(
        steps[0],
        AgentStepDetail::RecordsFound { count: Some(0), register, .. } if register == "Job applications"
    ));
    let AgentStepDetail::RecordChanged {
        action,
        title,
        change_id,
        record_id,
        undone,
        ..
    } = steps[1]
    else {
        panic!("{steps:?}")
    };
    assert_eq!(
        (action.as_str(), title.as_str()),
        ("added", "Acme · Service Desk Analyst")
    );
    assert_eq!(*record_id, Some(record.record_id));
    assert!(!undone);
    let detail = client.record_detail(record.record_id).await.unwrap();
    assert_eq!(
        detail.history[0].conversation_id,
        Some(sent.conversation_id)
    );
    assert_eq!(detail.history[0].actor, RecordActor::Agent);

    // One click: the addition is undone and its step says so.
    client.undo_record_change(change_id.unwrap()).await.unwrap();
    assert!(client.records(all(&register)).await.unwrap().is_empty());
    let view = client.conversation(sent.conversation_id).await.unwrap();
    let after = view
        .messages
        .iter()
        .find(|m| m.message_id == sent.reply_message_id)
        .unwrap();
    assert!(
        record_steps(after)
            .iter()
            .any(|d| matches!(d, AgentStepDetail::RecordChanged { undone: true, .. }))
    );
    let again = client
        .undo_record_change(change_id.unwrap())
        .await
        .unwrap_err();
    assert_eq!(again.code(), Some(ErrorCode::NotFound));
    host.shutdown().await;
}

#[tokio::test]
async fn an_update_by_the_agent_is_undone_and_the_earlier_values_return() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let register = jobs(&client).await;
    let mine = client
        .save_record(SaveRecordRequest {
            register_id: register.register_id,
            record_id: None,
            revision: 0,
            values: values(&[
                ("company", "Acme"),
                ("role", "Service Desk Analyst"),
                ("status", "Applied"),
            ]),
        })
        .await
        .unwrap();

    fake.push(Reply::calls(&[(
        "update_record",
        json!({"register": "job_applications", "record_id": mine.record_id.to_string(),
               "values": {"status": "interviewing", "next_step": "Interview Tuesday 10:00", "follow_up_on": "2026-10-27"}}),
    )]));
    fake.push(Reply::text(&["Updated."]));
    let sent = send(
        &client,
        None,
        "u1",
        "Acme invited me to an interview on Tuesday at 10.",
    )
    .await;
    let reply = settled(&client, &sent).await;
    let AgentStepDetail::RecordChanged {
        action,
        fields,
        change_id,
        ..
    } = record_steps(&reply)[0]
    else {
        panic!()
    };
    assert_eq!(action, "updated");
    assert_eq!(fields, &["follow_up_on", "next_step", "status"]);

    let detail = client.record_detail(mine.record_id).await.unwrap();
    assert_eq!(detail.record.values["status"], "Interviewing");
    assert_eq!(
        detail.record.values["company"], "Acme",
        "fields not given stay"
    );
    assert_eq!(detail.history[0].actor, RecordActor::Agent);
    assert!(detail.history[0].can_undo);

    client.undo_record_change(change_id.unwrap()).await.unwrap();
    let detail = client.record_detail(mine.record_id).await.unwrap();
    assert_eq!(detail.record.values["status"], "Applied");
    assert!(!detail.record.values.contains_key("next_step"));
    assert_eq!(detail.history[0].kind, RecordChangeKind::Undo);
    assert!(detail.history[1].undone);
    host.shutdown().await;
}

#[tokio::test]
async fn a_bad_value_a_duplicate_and_a_flood_of_writes_are_refused() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let register = jobs(&client).await;
    client
        .save_record(SaveRecordRequest {
            register_id: register.register_id,
            record_id: None,
            revision: 0,
            values: values(&[("company", "Acme"), ("role", "Service Desk Analyst")]),
        })
        .await
        .unwrap();

    let add = |company: &str, applied_on: &str| {
        (
            "add_record",
            json!({"register": "job_applications", "values": {
                "company": company, "role": "Service Desk Analyst", "applied_on": applied_on}}),
        )
    };
    let mut calls = vec![add("Acme", "2026-10-18"), add("Beta", "18.10.2026")];
    for i in 0..10 {
        calls.push(add(&format!("Company {i}"), "2026-10-18"));
    }
    fake.push(Reply::calls(&calls));
    fake.push(Reply::text(&["Done."]));
    let sent = send(&client, None, "f1", "Save these.").await;
    let reply = settled(&client, &sent).await;

    let body = fake.generations()[1].json();
    let results = answers(&body, "add_record");
    assert_eq!(results.len(), 12);
    assert!(
        results[0]["error"]
            .as_str()
            .unwrap()
            .contains("use update_record")
    );
    assert!(results[1]["error"].as_str().unwrap().contains("YYYY-MM-DD"));
    assert!(results[2..10].iter().all(|r| r["saved"] == true));
    assert!(
        results[10..]
            .iter()
            .all(|r| r["error"].as_str().unwrap().contains("budget")),
        "past ten writes per reply nothing more is saved"
    );
    let failed = reply
        .steps
        .iter()
        .filter(|s| {
            matches!(s.detail, AgentStepDetail::RecordChanged { .. })
                && s.status == AgentStepStatus::Failed
        })
        .count();
    assert_eq!(failed, 2);
    assert_eq!(client.records(all(&register)).await.unwrap().len(), 9);
    host.shutdown().await;
}
