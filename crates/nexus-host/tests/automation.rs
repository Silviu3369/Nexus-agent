//! Automations through the real host (`PLAN_NEXUS_RO.md` M4): a schedule
//! that cannot run is refused, a due time sends the instruction once as a
//! new task named after the automation, "Run now" leaves the schedule as it
//! was, a run whose reply is still being written is never overlapped, a
//! time missed while the Host was off is caught up at most once (only if
//! asked, within a day), and a run sent just before a stop is recorded, not
//! sent again. Time zone and clock-change rules are tested in `schedule`.

mod support;

use std::path::Path;
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_host::backup::DATABASE_FILE;
use nexus_host::schedule;
use nexus_protocol::{
    AutomationId, AutomationRunStatus, AutomationRunView, ChatRole, ChatStatus, ErrorCode,
    SaveAutomationRequest, Schedule,
};
use nexus_storage::{NewExchange, Store, now_ms};
use support::TestHost;
use support::chat::gemini;
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "automation-token";
const KEY: &str = "SENTINEL-gemini-key-automation-41c2";
const PROMPT: &str = "Summarise what changed in my notes today.";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(30),
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

fn request(name: &str, schedule: Schedule, catch_up: bool) -> SaveAutomationRequest {
    SaveAutomationRequest {
        automation_id: None,
        revision: 0,
        name: name.to_string(),
        prompt: PROMPT.to_string(),
        project_id: None,
        schedule,
        enabled: true,
        catch_up,
    }
}

/// Every day, twelve hours from now in local time: its latest time is
/// about half a day ago and its next one about half a day ahead.
fn daily_far_from_now() -> Schedule {
    let label = schedule::local_label(now_ms(), None);
    let hour: u8 = label[11..13].parse().unwrap();
    Schedule::Weekly {
        days: (0..7).collect(),
        hour: (hour + 12) % 24,
        minute: 0,
    }
}

/// Waits until the automation has at least `count` history entries.
async fn runs_when(client: &NexusClient, id: AutomationId, count: usize) -> Vec<AutomationRunView> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let runs = client.automation_runs(id, 50).await.unwrap();
        if runs.len() >= count {
            return runs;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {count} runs, have {runs:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The task's reply once it is no longer being written.
async fn reply_of(client: &NexusClient, run: &AutomationRunView) -> (String, ChatStatus, String) {
    let conversation = run.conversation_id.expect("a started run names its task");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = client.conversation(conversation).await.unwrap();
        let reply = view
            .messages
            .iter()
            .find(|m| m.role == ChatRole::Model)
            .unwrap();
        if reply.status != ChatStatus::Streaming {
            return (view.title, reply.status, reply.text.clone());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the reply never settled"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Changes the profile's database while the host is stopped, as time
/// passing while it was off would.
async fn with_db<T: Send + 'static>(profile: &Path, f: impl AsyncFnOnce(&Store) -> T) -> T {
    let store = Store::open(&profile.join(DATABASE_FILE)).unwrap();
    f(&store).await
}

async fn set_next(store: &Store, id: AutomationId, schedule: Option<&Schedule>, next: i64) {
    let schedule = schedule.map(|s| serde_json::to_string(s).unwrap());
    store
        .call(move |c| {
            c.execute(
                "UPDATE automations SET next_run_at_ms = ?1 WHERE id = ?2",
                (next, id.to_string()),
            )?;
            if let Some(schedule) = schedule {
                c.execute(
                    "UPDATE automations SET schedule_json = ?1 WHERE id = ?2",
                    (schedule, id.to_string()),
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_schedule_that_cannot_run_is_refused_and_a_stale_edit_is_not_saved() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let refused = [
        request(
            "No days",
            Schedule::Weekly {
                days: vec![],
                hour: 9,
                minute: 0,
            },
            false,
        ),
        request(
            "Past",
            Schedule::Once {
                at_ms: now_ms() - 1_000,
            },
            false,
        ),
        request("   ", daily_far_from_now(), false),
        SaveAutomationRequest {
            prompt: " ".to_string(),
            ..request("Empty", daily_far_from_now(), false)
        },
    ];
    for bad in refused {
        let err = client.save_automation(bad).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::InvalidRequest), "{err}");
    }

    let now = now_ms();
    let saved = client
        .save_automation(request("Morning", daily_far_from_now(), false))
        .await
        .unwrap();
    let next = saved
        .next_run_at_ms
        .expect("an enabled schedule has a next time");
    assert!(next > now && next <= now + DAY_MS);
    assert!(saved.last_run.is_none());

    let edit = SaveAutomationRequest {
        automation_id: Some(saved.automation_id),
        revision: saved.revision,
        ..request("Evening", daily_far_from_now(), true)
    };
    let edited = client.save_automation(edit.clone()).await.unwrap();
    assert_eq!((edited.revision, edited.name.as_str()), (2, "Evening"));
    let stale = client.save_automation(edit).await.unwrap_err();
    assert_eq!(stale.code(), Some(ErrorCode::RevisionConflict));

    let off = client
        .save_automation(SaveAutomationRequest {
            automation_id: Some(saved.automation_id),
            revision: edited.revision,
            enabled: false,
            ..request("Evening", daily_far_from_now(), true)
        })
        .await
        .unwrap();
    assert_eq!(off.next_run_at_ms, None, "a disabled automation never runs");
    assert_eq!(client.automations().await.unwrap().len(), 1);

    client.delete_automation(saved.automation_id).await.unwrap();
    assert!(client.automations().await.unwrap().is_empty());
    let gone = client
        .automation_runs(saved.automation_id, 10)
        .await
        .unwrap_err();
    assert_eq!(gone.code(), Some(ErrorCode::NotFound));
    host.shutdown().await;
}

#[tokio::test]
async fn a_due_time_sends_one_task_named_after_the_automation() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    fake.push(Reply::text(&["Done."]));

    let at = now_ms() + 1_500;
    let saved = client
        .save_automation(request("Daily notes", Schedule::Once { at_ms: at }, false))
        .await
        .unwrap();
    assert_eq!(saved.next_run_at_ms, Some(at));

    let runs = runs_when(&client, saved.automation_id, 1).await;
    let run = &runs[0];
    assert_eq!(run.status, AutomationRunStatus::Started);
    assert_eq!(run.scheduled_for_ms, at);
    assert!(run.at_ms >= at && !run.catch_up && !run.manual);
    let (title, status, text) = reply_of(&client, run).await;
    assert_eq!(
        title,
        format!("Daily notes · {}", schedule::local_label(at, None))
    );
    assert_eq!((status, text.as_str()), (ChatStatus::Complete, "Done."));
    assert_eq!(
        client
            .conversation(run.conversation_id.unwrap())
            .await
            .unwrap()
            .requested_by,
        Some(nexus_protocol::ActorView {
            kind: "Automation".into(),
            name: "Daily notes".into(),
        }),
        "the task names the automation that asked for it (ADR-IDENTITY-001)"
    );
    let generations = fake.generations();
    assert_eq!(generations.len(), 1);
    assert!(generations[0].json().to_string().contains(PROMPT));

    let listed = client.automations().await.unwrap();
    assert_eq!(
        listed[0].next_run_at_ms, None,
        "a one-time schedule is done"
    );
    assert_eq!(
        listed[0].last_run.as_ref().map(|r| r.status),
        Some(AutomationRunStatus::Started)
    );
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(
        client
            .automation_runs(saved.automation_id, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(fake.generations().len(), 1, "sent once, never again");
    host.shutdown().await;
}

#[tokio::test]
async fn run_now_keeps_the_schedule_and_a_run_still_answering_is_not_overlapped() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    fake.push(Reply::Hang);

    let saved = client
        .save_automation(request("Inbox", daily_far_from_now(), false))
        .await
        .unwrap();
    let manual = client
        .run_automation_now(saved.automation_id)
        .await
        .unwrap();
    assert_eq!(manual.status, AutomationRunStatus::Started);
    assert!(manual.manual);
    assert_eq!(
        client.automations().await.unwrap()[0].next_run_at_ms,
        saved.next_run_at_ms,
        "running now does not move the schedule"
    );

    let again = client
        .run_automation_now(saved.automation_id)
        .await
        .unwrap_err();
    assert_eq!(again.code(), Some(ErrorCode::InvalidRequest));
    assert!(again.to_string().contains("still answering"));

    // Due while the first reply is still being written: skipped, noted.
    let at = now_ms() + 1_500;
    client
        .save_automation(SaveAutomationRequest {
            automation_id: Some(saved.automation_id),
            revision: saved.revision,
            ..request("Inbox", Schedule::Once { at_ms: at }, false)
        })
        .await
        .unwrap();
    let runs = runs_when(&client, saved.automation_id, 2).await;
    assert_eq!(runs[0].status, AutomationRunStatus::Skipped);
    assert_eq!(runs[0].scheduled_for_ms, at);
    assert!(
        runs[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("still answering")
    );
    assert!(runs[0].conversation_id.is_none());
    assert_eq!(fake.generations().len(), 1);

    let conversation = manual.conversation_id.unwrap();
    let streaming = client
        .conversation(conversation)
        .await
        .unwrap()
        .messages
        .into_iter()
        .find(|m| m.status == ChatStatus::Streaming)
        .unwrap();
    client.cancel_reply(streaming.message_id).await.unwrap();
    host.shutdown().await;
}

#[tokio::test]
async fn without_a_model_a_due_time_is_recorded_as_failed() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let saved = client
        .save_automation(request(
            "Report",
            Schedule::Once {
                at_ms: now_ms() + 1_200,
            },
            false,
        ))
        .await
        .unwrap();
    let runs = runs_when(&client, saved.automation_id, 1).await;
    assert_eq!(runs[0].status, AutomationRunStatus::Failed);
    assert!(
        runs[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("choose an AI connection"),
        "{runs:?}"
    );
    assert!(client.conversations(10).await.unwrap().is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn a_time_missed_while_the_host_was_off_is_caught_up_once_or_recorded() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let daily = daily_far_from_now();
    let catch_up = client
        .save_automation(request("Catch up", daily.clone(), true))
        .await
        .unwrap()
        .automation_id;
    let no_catch_up = client
        .save_automation(request("No catch up", daily.clone(), false))
        .await
        .unwrap()
        .automation_id;
    let too_old = client
        .save_automation(request(
            "Too old",
            Schedule::Once {
                at_ms: now_ms() + DAY_MS,
            },
            true,
        ))
        .await
        .unwrap()
        .automation_id;
    let profile = host.shutdown().await;

    // The Host was off for three days.
    let now = now_ms();
    let three_days_ago = schedule::next_run(&daily, now - 3 * DAY_MS, None).unwrap();
    let latest = schedule::next_run(&daily, now - DAY_MS, None).unwrap();
    let once = Schedule::Once {
        at_ms: now - 2 * DAY_MS,
    };
    with_db(profile.path(), async |store| {
        set_next(store, catch_up, None, three_days_ago).await;
        set_next(store, no_catch_up, None, latest).await;
        set_next(store, too_old, Some(&once), now - 2 * DAY_MS).await;
    })
    .await;

    fake.push(Reply::text(&["Caught up."]));
    let host = TestHost::start_in(profile, TOKEN, config(&fake)).await;
    let client = host.client().await;

    let runs = runs_when(&client, catch_up, 2).await;
    assert_eq!(runs.len(), 2, "every missed time collapses into one run");
    assert_eq!(runs[0].status, AutomationRunStatus::Started);
    assert!(runs[0].catch_up && !runs[0].manual);
    assert_eq!(runs[0].scheduled_for_ms, latest);
    assert_eq!(runs[1].status, AutomationRunStatus::Missed);
    assert_eq!(runs[1].scheduled_for_ms, three_days_ago);
    assert_eq!(
        runs[1].reason.as_deref(),
        Some("Nexus.Host was not running at this time or the next one")
    );
    let (_, status, text) = reply_of(&client, &runs[0]).await;
    assert_eq!(
        (status, text.as_str()),
        (ChatStatus::Complete, "Caught up.")
    );

    let runs = runs_when(&client, no_catch_up, 1).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, AutomationRunStatus::Missed);
    assert!(
        runs[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("catching up is off")
    );

    let runs = runs_when(&client, too_old, 1).await;
    assert_eq!(runs[0].status, AutomationRunStatus::Missed);
    assert!(
        runs[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("more than a day ago")
    );

    for automation in client.automations().await.unwrap() {
        match automation.schedule {
            Schedule::Once { .. } => assert_eq!(automation.next_run_at_ms, None),
            Schedule::Weekly { .. } => assert!(automation.next_run_at_ms.unwrap() > now),
        }
    }
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(fake.generations().len(), 1, "one catch-up, nothing else");
    host.shutdown().await;
}

#[tokio::test]
async fn a_run_sent_just_before_a_stop_is_recorded_not_sent_again() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;
    let id = client
        .save_automation(request("Interrupted", daily_far_from_now(), false))
        .await
        .unwrap()
        .automation_id;
    let profile = host.shutdown().await;

    // The task was sent at its time, and the Host stopped before noting it.
    let due = now_ms() - 60_000;
    let sent = with_db(profile.path(), async |store| {
        let connection = store.connections().await.unwrap()[0].id;
        let persona = store.persona(now_ms()).await.unwrap().version;
        let exchange = store
            .append_exchange(
                NewExchange {
                    requester: nexus_storage::Requester::User,
                    pack: None,
                    conversation_id: None,
                    project_id: None,
                    idempotency_key: format!("auto:{id}:{due}"),
                    text: PROMPT.to_string(),
                    connection_id: connection,
                    model: MODEL.to_string(),
                    persona_version: persona,
                    title: Some("Interrupted".to_string()),
                },
                due,
            )
            .await
            .unwrap();
        set_next(store, id, None, due).await;
        exchange.conversation_id
    })
    .await;

    let host = TestHost::start_in(profile, TOKEN, config(&fake)).await;
    let client = host.client().await;
    let runs = runs_when(&client, id, 1).await;
    assert_eq!(runs[0].status, AutomationRunStatus::Started);
    assert_eq!(runs[0].conversation_id, Some(sent));
    let (_, status, _) = reply_of(&client, &runs[0]).await;
    assert_eq!(
        status,
        ChatStatus::Failed,
        "the stop interrupted that reply"
    );
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(fake.generations().is_empty(), "never sent again");
    assert!(
        client.automations().await.unwrap()[0]
            .next_run_at_ms
            .unwrap()
            > now_ms()
    );
    host.shutdown().await;
}
