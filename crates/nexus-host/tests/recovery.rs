//! Crash recovery with real processes (`SECURITATE_SI_VALIDARE_RO.md` §5): the
//! actual `nexus-host` binary is aborted at defined points via
//! `NEXUS_FAULT_POINT` (debug builds only), then restarted on the same
//! isolated profile. A mocked database could not prove any of this.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_protocol::{
    ApprovalDecision, DecideApprovalRequest, RunId, RunStatus, RunView, SubmitNoteRequest,
};
use support::{TempProfile, connect, spawn_host, wait_exit};

const CONTENT: &str = "# Recuperare\n\npasul 1\n";

async fn wait_status(client: &NexusClient, run: RunId, wanted: RunStatus) -> RunView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = client.run(run).await.expect("get_run must succeed");
        if view.status == wanted {
            return view;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run stayed {:?}, expected {wanted:?}",
            view.status
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn note_files(profile: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(projects) = std::fs::read_dir(profile.join("artifacts")) {
        for project in projects.flatten() {
            for entry in std::fs::read_dir(project.path()).unwrap().flatten() {
                if entry.path().extension().is_some_and(|e| e == "md") {
                    found.push(entry.path());
                }
            }
        }
    }
    found
}

fn staging_files(profile: &Path) -> usize {
    let mut count = 0;
    if let Ok(projects) = std::fs::read_dir(profile.join("artifacts")) {
        for project in projects.flatten() {
            if let Ok(entries) = std::fs::read_dir(project.path().join(".staging")) {
                count += entries.flatten().count();
            }
        }
    }
    count
}

/// Starts a host that will abort at `fault`, approves one note and waits for
/// the crash. Returns the run and the destination the approval covered.
async fn approve_and_crash(profile: &Path, fault: &str) -> (RunId, String) {
    let mut host = spawn_host(profile, Some(fault));
    let client = connect(profile).await;
    let submitted = client
        .submit_note(SubmitNoteRequest {
            idempotency_key: "recovery".to_string(),
            title: "Recuperare".to_string(),
            body: "pasul 1".to_string(),
        })
        .await
        .unwrap();
    let waiting = wait_status(&client, submitted.run_id, RunStatus::AwaitingApproval).await;
    let pending = client
        .run(waiting.run_id)
        .await
        .unwrap()
        .pending_approval
        .unwrap();
    let destination = pending.preview.as_ref().unwrap().destination.clone();
    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
            via: nexus_protocol::ApprovalPath::Window,
        })
        .await
        .unwrap();
    drop(client);
    let status = wait_exit(&mut host);
    assert!(
        !status.success(),
        "the host must have been aborted at {fault}"
    );
    (submitted.run_id, destination)
}

async fn restart_and_stop(profile: &Path, check: impl AsyncFnOnce(&NexusClient)) {
    let mut host = spawn_host(profile, None);
    let client = connect(profile).await;
    check(&client).await;
    client.stop_host().await.unwrap();
    drop(client);
    assert!(
        wait_exit(&mut host).success(),
        "an explicit stop must exit cleanly"
    );
}

#[tokio::test]
async fn run05_crash_after_publishing_reconciles_without_a_second_file() {
    let profile = TempProfile::new("recovery");
    let (run, destination) = approve_and_crash(profile.path(), "after_publish").await;

    // The note was published before the crash; the result was not recorded.
    assert_eq!(
        note_files(profile.path()),
        vec![PathBuf::from(&destination)]
    );

    restart_and_stop(profile.path(), async |client| {
        let done = wait_status(client, run, RunStatus::Succeeded).await;
        assert_eq!(done.artifact.unwrap().path, destination);
    })
    .await;
    assert_eq!(
        note_files(profile.path()).len(),
        1,
        "reconciliation must not create a second file"
    );
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), CONTENT);
    assert_eq!(staging_files(profile.path()), 0);
}

#[tokio::test]
async fn run04_crash_after_intent_before_the_effect_retries_the_same_operation() {
    let profile = TempProfile::new("recovery");
    let (run, destination) = approve_and_crash(profile.path(), "after_intent").await;
    assert!(
        note_files(profile.path()).is_empty(),
        "the crash happened before any write"
    );

    restart_and_stop(profile.path(), async |client| {
        let done = wait_status(client, run, RunStatus::Succeeded).await;
        assert_eq!(
            done.artifact.unwrap().path,
            destination,
            "same OperationId, same destination"
        );
        let events = client.events(0, 1000).await.unwrap().events;
        assert!(events.iter().any(|e| e.kind == "OperationRetried"));
    })
    .await;
    assert_eq!(
        note_files(profile.path()),
        vec![PathBuf::from(&destination)]
    );
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), CONTENT);
}

#[tokio::test]
async fn run05_partial_write_left_by_a_crash_is_never_treated_as_success() {
    let profile = TempProfile::new("recovery");
    let (run, destination) = approve_and_crash(profile.path(), "after_intent").await;

    // Simulate a crash in the middle of writing: a truncated staging file.
    let target = PathBuf::from(&destination);
    let staging = target.parent().unwrap().join(".staging").join(
        target
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(".md", ".tmp"),
    );
    std::fs::create_dir_all(staging.parent().unwrap()).unwrap();
    std::fs::write(&staging, &CONTENT[..6]).unwrap();

    restart_and_stop(profile.path(), async |client| {
        wait_status(client, run, RunStatus::Succeeded).await;
    })
    .await;
    assert_eq!(note_files(profile.path()), vec![target.clone()]);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        CONTENT,
        "the full approved content, not the fragment"
    );
    assert_eq!(
        staging_files(profile.path()),
        0,
        "the fragment must be cleaned up"
    );
}

#[tokio::test]
async fn run06_file_modified_before_recovery_needs_review_and_is_not_overwritten() {
    let profile = TempProfile::new("recovery");
    let (run, destination) = approve_and_crash(profile.path(), "after_publish").await;
    std::fs::write(&destination, "changed by someone else").unwrap();

    restart_and_stop(profile.path(), async |client| {
        let review = wait_status(client, run, RunStatus::NeedsReview).await;
        assert_eq!(review.failure.unwrap().code, "OutcomeUnknown");
        assert!(
            review.artifact.is_none(),
            "an unproven file must not be recorded as the result"
        );
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "changed by someone else"
    );
    assert_eq!(note_files(profile.path()).len(), 1);
}
