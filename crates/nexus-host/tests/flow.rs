//! The M1 circuit end to end over real RPC: real client → named pipe → host
//! runtime → SQLite → NTFS. No mocked layer.

mod support;

use std::path::Path;
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_protocol::{
    ApprovalDecision, DecideApprovalRequest, ErrorCode, RunId, RunStatus, RunView,
    SubmitNoteRequest,
};
use support::TestHost;

const TOKEN: &str = "flow-token";

fn note(key: &str, title: &str, body: &str) -> SubmitNoteRequest {
    SubmitNoteRequest {
        idempotency_key: key.to_string(),
        title: title.to_string(),
        body: body.to_string(),
    }
}

async fn wait_for(client: &NexusClient, run: RunId, wanted: RunStatus) -> RunView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Note files in the artifact store, ignoring the staging directory.
fn note_files(profile: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let root = profile.join("artifacts");
    let Ok(projects) = std::fs::read_dir(&root) else {
        return found;
    };
    for project in projects.flatten() {
        for entry in std::fs::read_dir(project.path()).unwrap().flatten() {
            if entry.path().extension().is_some_and(|e| e == "md") {
                found.push(entry.path());
            }
        }
    }
    found
}

async fn submit_and_wait_for_approval(client: &NexusClient, key: &str) -> RunView {
    let submitted = client
        .submit_note(note(key, "Plan", "pasul 1"))
        .await
        .unwrap();
    assert!(submitted.created);
    wait_for(client, submitted.run_id, RunStatus::AwaitingApproval).await
}

#[tokio::test]
async fn run01_note_is_written_only_after_approving_the_exact_preview() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;

    let waiting = submit_and_wait_for_approval(&client, "k1").await;
    let pending = waiting
        .pending_approval
        .expect("awaiting run must expose its approval");
    let preview = pending
        .preview
        .expect("get_run must include the exact preview");
    assert_eq!(preview.content, "# Plan\n\npasul 1\n");
    assert!(
        preview
            .destination
            .starts_with(&*host.profile.path().join("artifacts").to_string_lossy())
    );
    assert!(
        note_files(host.profile.path()).is_empty(),
        "nothing may be written before approval"
    );

    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
        })
        .await
        .unwrap();
    let done = wait_for(&client, waiting.run_id, RunStatus::Succeeded).await;

    let artifact = done
        .artifact
        .expect("a succeeded run must reference its artifact");
    assert_eq!(artifact.path, preview.destination);
    assert_eq!(artifact.sha256, preview.sha256);
    assert_eq!(
        client.artifact_content(artifact.artifact_id).await.unwrap(),
        preview.content
    );
    assert_eq!(
        note_files(host.profile.path()),
        vec![Path::new(&artifact.path).to_path_buf()]
    );

    let page = client.events(0, 1000).await.unwrap();
    assert!(page.events.iter().any(|e| e.kind == "ArtifactCreated"));
    host.shutdown().await;
}

#[tokio::test]
async fn run02_denied_note_writes_nothing() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let waiting = submit_and_wait_for_approval(&client, "k1").await;
    let pending = client
        .run(waiting.run_id)
        .await
        .unwrap()
        .pending_approval
        .unwrap();

    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Deny,
        })
        .await
        .unwrap();
    let denied = wait_for(&client, waiting.run_id, RunStatus::Denied).await;
    assert_eq!(denied.failure.unwrap().code, "Denied");
    assert!(note_files(host.profile.path()).is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn run02_expired_approval_fails_the_run_and_cannot_be_used() {
    let host = TestHost::start_with_ttl(TOKEN, Duration::from_millis(300)).await;
    let client = host.client().await;
    let waiting = submit_and_wait_for_approval(&client, "k1").await;
    let pending = client
        .run(waiting.run_id)
        .await
        .unwrap()
        .pending_approval
        .unwrap();

    let failed = wait_for(&client, waiting.run_id, RunStatus::Failed).await;
    assert_eq!(failed.failure.unwrap().code, "ApprovalExpired");

    let late = client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
        })
        .await;
    assert_eq!(late.unwrap_err().code(), Some(ErrorCode::ApprovalExpired));
    assert!(note_files(host.profile.path()).is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn sec01_decision_for_another_manifest_and_replays_are_refused() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let waiting = submit_and_wait_for_approval(&client, "k1").await;
    let pending = client
        .run(waiting.run_id)
        .await
        .unwrap()
        .pending_approval
        .unwrap();

    let tampered = client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: "0".repeat(64),
            decision: ApprovalDecision::Approve,
        })
        .await;
    assert_eq!(
        tampered.unwrap_err().code(),
        Some(ErrorCode::ApprovalInvalidated)
    );
    assert_eq!(
        client.run(waiting.run_id).await.unwrap().status,
        RunStatus::AwaitingApproval
    );

    let approve = DecideApprovalRequest {
        approval_id: pending.approval_id,
        manifest_sha256: pending.manifest_sha256.clone(),
        decision: ApprovalDecision::Approve,
    };
    client.decide_approval(approve.clone()).await.unwrap();
    wait_for(&client, waiting.run_id, RunStatus::Succeeded).await;

    let replay = client.decide_approval(approve).await;
    assert_eq!(
        replay.unwrap_err().code(),
        Some(ErrorCode::ApprovalInvalidated)
    );
    assert_eq!(
        note_files(host.profile.path()).len(),
        1,
        "a replay must not write a second note"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn run03_repeated_submit_is_deduplicated_and_reused_key_conflicts() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;

    let first = client.submit_note(note("same", "Plan", "a")).await.unwrap();
    let again = client.submit_note(note("same", "Plan", "a")).await.unwrap();
    assert!(first.created);
    assert!(!again.created);
    assert_eq!(first.run_id, again.run_id);

    let conflict = client.submit_note(note("same", "Plan", "b")).await;
    assert_eq!(
        conflict.unwrap_err().code(),
        Some(ErrorCode::RevisionConflict)
    );
    assert_eq!(client.snapshot(50).await.unwrap().runs.len(), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn run07_cancelled_waiting_run_cannot_be_approved_later() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let waiting = submit_and_wait_for_approval(&client, "k1").await;
    let pending = client
        .run(waiting.run_id)
        .await
        .unwrap()
        .pending_approval
        .unwrap();

    let cancelled = client.cancel_run(waiting.run_id).await.unwrap();
    assert_eq!(cancelled.status, RunStatus::Cancelled);

    let late = client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
        })
        .await;
    assert_eq!(
        late.unwrap_err().code(),
        Some(ErrorCode::ApprovalInvalidated)
    );
    assert!(note_files(host.profile.path()).is_empty());
    assert!(
        client.cancel_run(waiting.run_id).await.is_err(),
        "a finished run cannot be cancelled again"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn invalid_note_is_refused_before_anything_is_stored() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;

    let empty_title = client.submit_note(note("k", "   ", "body")).await;
    assert_eq!(
        empty_title.unwrap_err().code(),
        Some(ErrorCode::InvalidRequest)
    );
    let too_big = client
        .submit_note(note("k2", "Mare", &"x".repeat(200 * 1024)))
        .await;
    assert!(
        too_big.is_err(),
        "a note over the size limit must be refused"
    );
    let no_key = client.submit_note(note("", "Plan", "body")).await;
    assert_eq!(no_key.unwrap_err().code(), Some(ErrorCode::InvalidRequest));

    assert!(client.snapshot(50).await.unwrap().runs.is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn evt01_event_pages_continue_from_the_cursor_without_duplicates() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let waiting = submit_and_wait_for_approval(&client, "k1").await;

    let snapshot = client.snapshot(50).await.unwrap();
    assert_eq!(snapshot.runs[0].run_id, waiting.run_id);
    assert!(
        snapshot.runs[0]
            .pending_approval
            .as_ref()
            .unwrap()
            .preview
            .is_none(),
        "snapshots stay bounded"
    );

    let first = client.events(0, 1000).await.unwrap();
    assert_eq!(first.next_after, first.events.last().unwrap().seq);
    assert!(first.next_after >= snapshot.watermark);
    let rest = client.events(first.next_after, 1000).await.unwrap();
    assert!(rest.events.is_empty(), "no event may be delivered twice");
    assert_eq!(rest.next_after, first.next_after);

    let small = client.events(0, 2).await.unwrap();
    assert_eq!(small.events.len(), 2);
    let next = client.events(small.next_after, 1000).await.unwrap();
    assert_eq!(next.events[0].seq, small.events[1].seq + 1);
    host.shutdown().await;
}

#[tokio::test]
async fn artifact_changed_outside_nexus_is_reported_not_served() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let waiting = submit_and_wait_for_approval(&client, "k1").await;
    let pending = client
        .run(waiting.run_id)
        .await
        .unwrap()
        .pending_approval
        .unwrap();
    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
        })
        .await
        .unwrap();
    let artifact = wait_for(&client, waiting.run_id, RunStatus::Succeeded)
        .await
        .artifact
        .unwrap();

    std::fs::write(&artifact.path, "edited by someone else").unwrap();
    let read = client.artifact_content(artifact.artifact_id).await;
    assert_eq!(read.unwrap_err().code(), Some(ErrorCode::ResourceChanged));
    host.shutdown().await;
}

#[tokio::test]
async fn stop_host_ends_the_accept_loop_and_the_executor() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    client.stop_host().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), host.shutdown())
        .await
        .expect("the host must stop promptly after StopHost");
}
