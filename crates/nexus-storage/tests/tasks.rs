//! The agent's work as tasks and runs (`schema::V15`, ADR-TASK-001) on a
//! real SQLite file: a conversation is one task with its origin; each
//! request is a run of type Agent that ends with its reply (succeeded,
//! failed with a cause from a closed list, stopped, or interrupted when the
//! Host stopped); a reply that waits on a proposal leaves its turn waiting,
//! the proposed effect becomes its child run, and the turn goes on after
//! the decision or ends when the user moves on; M1's executor, recovery and
//! run list never see the agent's runs; deleting a task removes them; and
//! the migration turns the existing history into tasks and runs.

mod common;

use std::path::PathBuf;

use nexus_core::MessageStatus;
use nexus_protocol::{AutomationRunStatus, ConversationId, MessageId, RunId, Schedule};
use nexus_storage::{
    AutomationRunRow, Exchange, NewAutomation, NewExchange, Pause, Requester, SUPPORTED_VERSION,
    Settlement, Store, SubmitOutcome,
};

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
    let dir = std::env::temp_dir().join(format!("nexus-tasks-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

async fn exchange(
    store: &Store,
    conversation: Option<ConversationId>,
    key: &str,
    requester: Requester,
    pack: Option<&str>,
) -> Exchange {
    store.persona(0).await.unwrap();
    store.ensure_default_project(0).await.unwrap();
    let connection = match store.connections().await.unwrap().first() {
        Some(c) => c.id,
        None => {
            store
                .save_connection(None, "Gemini", "g".into(), false, "gemini-tasks".into(), 1)
                .await
                .unwrap()
                .id
        }
    };
    store
        .append_exchange(
            NewExchange {
                conversation_id: conversation,
                project_id: None,
                idempotency_key: key.into(),
                text: format!("request {key}"),
                connection_id: connection,
                model: "gemini-test".into(),
                persona_version: 1,
                title: None,
                requester,
                pack: pack.map(str::to_string),
                agent: None,
            },
            2,
        )
        .await
        .unwrap()
}

async fn settle(store: &Store, ex: &Exchange, status: MessageStatus, code: Option<&str>) {
    store
        .settle_message(
            ex.model_message.id,
            status,
            Settlement {
                error_code: code.map(str::to_string),
                ..Settlement::default()
            },
            5,
        )
        .await
        .unwrap();
}

async fn effect_run(store: &Store, key: &str) -> RunId {
    let project = store.ensure_default_project(0).await.unwrap();
    match store
        .submit_task(
            nexus_storage::EffectRequester::Agent,
            project,
            key.into(),
            "{}".into(),
            key.into(),
            3,
        )
        .await
        .unwrap()
    {
        SubmitOutcome::Created { run_id, .. } | SubmitOutcome::Existing { run_id, .. } => run_id,
    }
}

/// The reply stops on `run`, waiting for the user.
async fn pause(store: &Store, ex: &Exchange, run: RunId) {
    store
        .settle_message(
            ex.model_message.id,
            MessageStatus::Complete,
            Settlement {
                proposed_run_id: Some(run),
                pause: Some(Pause {
                    run_id: run,
                    state_json: r#"{"version":1}"#.into(),
                }),
                ..Settlement::default()
            },
            4,
        )
        .await
        .unwrap();
}

/// (kind, state, failure cause, parent) of a run.
async fn run(store: &Store, id: RunId) -> (String, String, Option<String>, Option<String>) {
    store
        .call(move |c| {
            Ok(c.query_row(
                "SELECT kind, state, failure_cause, parent_run_id FROM runs WHERE id = ?1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?)
        })
        .await
        .unwrap()
}

/// (task id, origin, pack) of a conversation's task.
async fn task_of(store: &Store, conversation: ConversationId) -> (String, String, Option<String>) {
    store
        .call(move |c| {
            Ok(c.query_row(
                "SELECT id, origin, pack FROM tasks WHERE conversation_id = ?1",
                [conversation.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap()
}

async fn origin_of_run_task(store: &Store, run: RunId) -> String {
    store
        .call(move |c| {
            Ok(c.query_row(
                "SELECT t.origin FROM tasks t JOIN runs r ON r.task_id = t.id WHERE r.id = ?1",
                [run.to_string()],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap()
}

async fn count(store: &Store, sql: &'static str) -> i64 {
    store
        .call(move |c| Ok(c.query_row(sql, [], |r| r.get(0))?))
        .await
        .unwrap()
}

fn turn(ex: &Exchange) -> RunId {
    let run = ex
        .user_message
        .run_id
        .expect("the request belongs to a run");
    assert_eq!(ex.model_message.run_id, Some(run), "the reply too");
    run
}

#[tokio::test]
async fn a_request_is_a_run_of_its_task_and_ends_with_its_reply() {
    let env = env();
    let s = &env.store;
    let first = exchange(s, None, "a", Requester::User, None).await;
    let conversation = first.conversation_id;
    let (task, origin, pack) = task_of(s, conversation).await;
    assert_eq!((origin.as_str(), pack), ("User", None));
    let a = turn(&first);
    assert_eq!(
        run(s, a).await,
        ("Agent".into(), "Running".into(), None, None)
    );
    settle(s, &first, MessageStatus::Complete, None).await;
    assert_eq!(run(s, a).await.1, "Succeeded");

    let second = exchange(s, Some(conversation), "b", Requester::Inbox, None).await;
    assert_eq!(
        task_of(s, conversation).await.0,
        task,
        "one task per conversation"
    );
    assert_eq!(
        task_of(s, conversation).await.1,
        "User",
        "a follow-up keeps what started the task"
    );
    let b = turn(&second);
    assert_ne!(a, b, "each request is its own run");
    settle(s, &second, MessageStatus::Failed, Some("QuotaExceeded")).await;
    assert_eq!(
        run(s, b).await,
        (
            "Agent".into(),
            "Failed".into(),
            Some("QuotaExceeded".into()),
            None
        )
    );

    let third = exchange(s, Some(conversation), "c", Requester::User, None).await;
    settle(s, &third, MessageStatus::Cancelled, None).await;
    assert_eq!(run(s, turn(&third)).await.1, "Cancelled");
    // A second settlement changes nothing (the reply settles once).
    assert!(
        s.settle_message(
            third.model_message.id,
            MessageStatus::Complete,
            Settlement::default(),
            6
        )
        .await
        .is_err()
    );
    assert_eq!(run(s, turn(&third)).await.1, "Cancelled");

    let fourth = exchange(s, Some(conversation), "d", Requester::User, None).await;
    assert_eq!(s.fail_interrupted_messages(7).await.unwrap(), 1);
    assert_eq!(
        run(s, turn(&fourth)).await,
        (
            "Agent".into(),
            "Failed".into(),
            Some("HostInterrupted".into()),
            None
        )
    );

    // M1's run list and executor never see the agent's runs.
    let (runs, _) = s.snapshot(50).await.unwrap();
    assert!(runs.is_empty(), "{runs:?}");
    assert!(s.claim_next_queued_run(8).await.unwrap().is_none());

    s.delete_conversation(conversation, 9).await.unwrap();
    assert_eq!(count(s, "SELECT count(*) FROM tasks").await, 0);
    assert_eq!(count(s, "SELECT count(*) FROM runs").await, 0);
}

#[tokio::test]
async fn a_turn_waits_on_its_proposal_then_goes_on_or_ends_when_the_user_moves_on() {
    let env = env();
    let s = &env.store;
    let ex = exchange(s, None, "w", Requester::Inbox, Some("job_applications")).await;
    let (_, origin, pack) = task_of(s, ex.conversation_id).await;
    assert_eq!(
        (origin.as_str(), pack.as_deref()),
        ("Intake", Some("job_applications"))
    );
    let agent = turn(&ex);
    let effect = effect_run(s, "w-effect").await;
    assert_eq!(
        origin_of_run_task(s, effect).await,
        "Agent",
        "asked for by the agent from the start (ADR-IDENTITY-001)"
    );
    pause(s, &ex, effect).await;
    assert_eq!(run(s, agent).await.1, "AwaitingApproval");
    assert_eq!(
        run(s, effect).await.3,
        Some(agent.to_string()),
        "the proposed effect is a child run of the turn"
    );
    assert_eq!(origin_of_run_task(s, effect).await, "Agent");

    s.call(move |c| {
        c.execute(
            "UPDATE runs SET state = 'Succeeded' WHERE id = ?1",
            [effect.to_string()],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let continued = s
        .begin_continuation(ex.model_message.id, 6)
        .await
        .unwrap()
        .expect("the turn goes on after the decision");
    assert_eq!(continued.model_message.run_id, Some(agent), "same turn");
    assert_eq!(run(s, agent).await.1, "Running");
    s.settle_message(
        continued.model_message.id,
        MessageStatus::Complete,
        Settlement::default(),
        7,
    )
    .await
    .unwrap();
    assert_eq!(run(s, agent).await.1, "Succeeded");

    // A turn left waiting ends when the user sends a new request instead.
    let waiting = exchange(s, Some(ex.conversation_id), "w2", Requester::User, None).await;
    let effect2 = effect_run(s, "w2-effect").await;
    pause(s, &waiting, effect2).await;
    assert_eq!(run(s, turn(&waiting)).await.1, "AwaitingApproval");
    let next = exchange(s, Some(ex.conversation_id), "w3", Requester::User, None).await;
    assert_eq!(run(s, turn(&waiting)).await.1, "Cancelled");
    assert_eq!(run(s, turn(&next)).await.1, "Running");

    // Deleting the request and its replies removes that turn; its proposed
    // effect stays, without the link.
    s.settle_message(
        next.model_message.id,
        MessageStatus::Complete,
        Settlement::default(),
        8,
    )
    .await
    .unwrap();
    s.delete_exchange(waiting.user_message.id, 9).await.unwrap();
    assert_eq!(
        count(s, "SELECT count(*) FROM runs WHERE kind = 'Agent'").await,
        2
    );
    assert_eq!(run(s, effect2).await.3, None);
}

fn daily() -> NewAutomation {
    NewAutomation {
        name: "Morning".into(),
        prompt: "summarise".into(),
        project_id: None,
        schedule: Schedule::Weekly {
            days: vec![0, 1, 2, 3, 4, 5, 6],
            hour: 9,
            minute: 0,
        },
        enabled: true,
        catch_up: false,
        next_run_at_ms: Some(1_000),
    }
}

#[tokio::test]
async fn migrating_to_v15_turns_the_history_into_tasks_and_runs() {
    let env = env();
    let path = env.dir.join("nexus.db");
    let s = &env.store;
    // A chat with a reply that succeeded, one that failed (busy provider)
    // and one waiting on a proposal.
    let chat = exchange(s, None, "m1", Requester::User, None).await;
    settle(s, &chat, MessageStatus::Complete, None).await;
    let failed = exchange(s, Some(chat.conversation_id), "m2", Requester::User, None).await;
    settle(s, &failed, MessageStatus::Failed, Some("Busy")).await;
    let paused = exchange(s, Some(chat.conversation_id), "m3", Requester::User, None).await;
    let effect = effect_run(s, "m3-effect").await;
    pause(s, &paused, effect).await;
    // An automation's task and an intake's task.
    let auto = exchange(s, None, "m4", Requester::User, None).await;
    settle(s, &auto, MessageStatus::Complete, None).await;
    let automation = s.create_automation(daily(), 1).await.unwrap();
    s.record_automation_runs(
        automation.id,
        Some(1_000),
        vec![AutomationRunRow {
            automation_id: automation.id,
            scheduled_for_ms: 1_000,
            at_ms: 1_001,
            status: AutomationRunStatus::Started,
            reason: None,
            conversation_id: Some(auto.conversation_id),
            catch_up: false,
            manual: false,
        }],
        Some(2_000),
        1_001,
    )
    .await
    .unwrap();
    let ad = exchange(s, None, "m5", Requester::User, None).await;
    settle(s, &ad, MessageStatus::Complete, None).await;
    let register = s
        .ensure_register(
            "job_applications".into(),
            "Jobs".into(),
            "{}".into(),
            "status".into(),
            None,
            1,
        )
        .await
        .unwrap()
        .id;
    s.inbox_seen(register, "ad.pdf".into(), "ab".repeat(32), 1, None, 3)
        .await
        .unwrap();
    let file = s.next_inbox_file(register, 3).await.unwrap().unwrap();
    s.inbox_outcome(file.id, Some(ad.conversation_id), None, 4)
        .await
        .unwrap();
    // A note sent directly (M1).
    let direct = effect_run(s, "direct-note").await;

    s.call(|c| {
        common::undo_v15(c)?;
        c.execute_batch("PRAGMA user_version = 14;")?;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        count(s, "SELECT count(*) FROM tasks").await,
        2,
        "only M1's tasks at v14"
    );

    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let origins = |conversation: ConversationId| task_of(&s, conversation);
    assert_eq!(origins(chat.conversation_id).await.1, "User");
    assert_eq!(origins(auto.conversation_id).await.1, "Automation");
    assert_eq!(
        origins(ad.conversation_id).await,
        (
            origins(ad.conversation_id).await.0,
            "Intake".into(),
            Some("job_applications".into())
        )
    );
    let run_of = |message: MessageId| {
        let s = &s;
        async move {
            s.call(move |c| {
                Ok(c.query_row(
                    "SELECT r.state, r.failure_cause, r.kind FROM messages m JOIN runs r ON r.id = m.run_id
                     WHERE m.id = ?1",
                    [message.to_string()],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?)),
                )?)
            })
            .await
            .unwrap()
        }
    };
    assert_eq!(
        run_of(chat.user_message.id).await,
        ("Succeeded".into(), None, "Agent".into())
    );
    assert_eq!(
        run_of(failed.model_message.id).await,
        (
            "Failed".into(),
            Some("ProviderUnavailable".into()),
            "Agent".into()
        )
    );
    assert_eq!(run_of(paused.model_message.id).await.0, "AwaitingApproval");
    assert_eq!(
        count(&s, "SELECT count(*) FROM runs WHERE kind = 'Agent'").await,
        5,
        "one run per request"
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM messages WHERE run_id IS NULL").await,
        0
    );
    let effect_parent = run(&s, effect).await.3;
    let paused_turn: String = s
        .call(move |c| {
            Ok(c.query_row(
                "SELECT run_id FROM messages WHERE id = ?1",
                [paused.model_message.id.to_string()],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(effect_parent, Some(paused_turn));
    assert_eq!(origin_of_run_task(&s, effect).await, "Agent");
    assert_eq!(origin_of_run_task(&s, direct).await, "Direct");
    let (runs, _) = s.snapshot(50).await.unwrap();
    assert_eq!(runs.len(), 2, "M1's run list keeps only its own runs");
}
