//! BACKUP-01 on a real host, SQLite file and NTFS: consistent backup with a
//! hash manifest, restore into a new profile only, and no automatic
//! execution in the restored profile.

mod support;

use std::path::Path;
use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::backup::{self, BackupError, BackupManifest};
use nexus_protocol::{
    ApprovalDecision, DecideApprovalRequest, HealthStatus, RunId, RunStatus, RunView,
    SubmitNoteRequest,
};
use support::{TempProfile, TestHost};

const TOKEN: &str = "backup-token";

async fn wait_for(client: &NexusClient, run: RunId, wanted: RunStatus) -> RunView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let view = client.run(run).await.unwrap();
        if view.status == wanted {
            return view;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run stayed {:?}",
            view.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn submit(client: &NexusClient, key: &str, title: &str) -> RunView {
    let r = client
        .submit_note(SubmitNoteRequest {
            idempotency_key: key.to_string(),
            title: title.to_string(),
            body: format!("conținut {title}"),
        })
        .await
        .unwrap();
    wait_for(client, r.run_id, RunStatus::AwaitingApproval).await
}

async fn approve(client: &NexusClient, run: RunId) -> Result<RunView, nexus_client::CallError> {
    let pending = client.run(run).await.unwrap().pending_approval.unwrap();
    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
            via: nexus_protocol::ApprovalPath::Window,
        })
        .await
}

fn md_files(profile: &Path) -> usize {
    let mut n = 0;
    if let Ok(projects) = std::fs::read_dir(profile.join("artifacts")) {
        for p in projects.flatten() {
            n += std::fs::read_dir(p.path())
                .unwrap()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
                .count();
        }
    }
    n
}

#[tokio::test]
async fn backup01_backup_restore_and_no_automatic_execution() {
    // --- original profile: one finished note, one waiting for approval ---
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let done = submit(&client, "a", "Prima").await;
    approve(&client, done.run_id).await.unwrap();
    let done = wait_for(&client, done.run_id, RunStatus::Succeeded).await;
    let waiting = submit(&client, "b", "A doua").await;

    let summary = client.create_backup().await.unwrap();
    assert_eq!(summary.artifacts, 1);
    assert_eq!(summary.unmatched_artifacts, 0);
    let backup_dir = std::path::PathBuf::from(&summary.path);
    assert!(backup_dir.starts_with(host.profile.path().join("backups")));

    let manifest: BackupManifest =
        serde_json::from_slice(&std::fs::read(backup_dir.join(backup::MANIFEST_FILE)).unwrap())
            .unwrap();
    assert_eq!(manifest.database_sha256, summary.database_sha256);
    assert_eq!(
        manifest.artifacts[0].recorded_sha256,
        done.artifact.as_ref().unwrap().sha256
    );
    assert_eq!(
        manifest.artifacts[0].copied_sha256,
        manifest.artifacts[0].recorded_sha256
    );
    let original = client.snapshot(50).await.unwrap();
    drop(client);
    let _original_profile = host.shutdown().await;

    // --- restore refuses an existing, non-empty profile ---
    let occupied = TempProfile::new("occupied");
    std::fs::write(occupied.path().join("keep.txt"), "user data").unwrap();
    assert!(matches!(
        backup::restore(&backup_dir, occupied.path()),
        Err(BackupError::TargetNotEmpty(_))
    ));
    assert_eq!(
        std::fs::read_to_string(occupied.path().join("keep.txt")).unwrap(),
        "user data"
    );

    // --- restore into a new profile; files verified against the manifest ---
    let restored = TempProfile::new("restored");
    let target = restored.path().join("profile");
    backup::restore(&backup_dir, &target).unwrap();
    assert!(backup::is_restored_profile(&target));
    assert_eq!(md_files(&target), 1);

    // --- the restored profile has the same runs and executes nothing ---
    let runtime = nexus_host::Runtime::open(&target, nexus_host::DEFAULT_APPROVAL_TTL)
        .await
        .unwrap();
    assert!(runtime.is_paused());
    let pipe = support::unique_pipe_name();
    let listener = nexus_host::bind_first_instance(&pipe).unwrap();
    let executor = tokio::spawn(runtime.clone().run_executor());
    tokio::spawn(nexus_host::serve_forever(
        listener,
        pipe.clone(),
        TOKEN.to_string(),
        runtime.clone(),
    ));
    let (rclient, _) = NexusClient::connect_with(&pipe, TOKEN.to_string(), "restore-test")
        .await
        .unwrap();

    let health = rclient.get_health().await.unwrap().unwrap();
    assert_eq!(health.status, HealthStatus::Paused);
    let copy = rclient.snapshot(50).await.unwrap();
    let states = |s: &nexus_protocol::Snapshot| {
        let mut v: Vec<_> = s.runs.iter().map(|r| (r.run_id, r.status)).collect();
        v.sort_by_key(|(id, _)| id.to_string());
        v
    };
    assert_eq!(
        states(&copy),
        states(&original),
        "the restore must reproduce the same runs"
    );

    // Approving the waiting note is recorded, but nothing executes.
    approve(&rclient, waiting.run_id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        rclient.run(waiting.run_id).await.unwrap().status,
        RunStatus::Queued
    );
    assert_eq!(
        md_files(&target),
        1,
        "a paused profile must not create notes"
    );
    assert_eq!(
        rclient
            .artifact_content(done.artifact.unwrap().artifact_id)
            .await
            .unwrap(),
        "# Prima\n\nconținut Prima\n"
    );

    runtime.request_stop();
    let _ = executor.await;
}

#[tokio::test]
async fn backup01_tampered_backup_is_refused() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let run = submit(&client, "a", "Prima").await;
    approve(&client, run.run_id).await.unwrap();
    wait_for(&client, run.run_id, RunStatus::Succeeded).await;
    let summary = client.create_backup().await.unwrap();
    drop(client);
    let _original_profile = host.shutdown().await;

    let backup_dir = std::path::PathBuf::from(&summary.path);
    let manifest: BackupManifest =
        serde_json::from_slice(&std::fs::read(backup_dir.join(backup::MANIFEST_FILE)).unwrap())
            .unwrap();
    std::fs::write(
        backup_dir
            .join("artifacts")
            .join(&manifest.artifacts[0].relpath),
        "tampered",
    )
    .unwrap();

    let restored = TempProfile::new("restored");
    let target = restored.path().join("profile");
    assert!(matches!(
        backup::restore(&backup_dir, &target),
        Err(BackupError::Verification(_))
    ));
    assert!(
        !backup::is_restored_profile(&target),
        "a failed restore must not produce a usable profile"
    );
}
