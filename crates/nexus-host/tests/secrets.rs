//! DATA-01 with the real `nexus-host` binary: the pairing secret never
//! appears in the log, in errors, in responses or in a backup, and a refused
//! credential is not echoed back or logged.

mod support;

use std::time::Duration;

use nexus_client::NexusClient;
use nexus_protocol::pairing;
use nexus_protocol::{
    ApprovalDecision, DecideApprovalRequest, RunId, RunStatus, RunView, SubmitNoteRequest,
};
use support::{TempProfile, connect, spawn_host, wait_exit};

async fn wait_for(
    client: &NexusClient,
    run: RunId,
    wanted: RunStatus,
    seen: &mut Vec<String>,
) -> RunView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = client.run(run).await.unwrap();
        seen.push(serde_json::to_string(&view).unwrap());
        if view.status == wanted {
            return view;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run stayed {:?}",
            view.status
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(files_under(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[tokio::test]
async fn data01_pairing_secret_never_leaks() {
    let profile = TempProfile::new("secrets");
    let mut host = spawn_host(profile.path(), None);
    let client = connect(profile.path()).await;
    let secret = pairing::read_token_in(profile.path()).unwrap();
    let mut seen: Vec<String> = Vec::new();

    // A refused credential: the error must not echo it back.
    let wrong = format!("WRONG-{}", uuid::Uuid::new_v4());
    let refused = NexusClient::connect_with(
        &pairing::pipe_name_for(profile.path()),
        wrong.clone(),
        "attacker",
    )
    .await
    .err()
    .expect("a wrong token must be refused");
    let refusal = format!("{refused} {refused:?}");
    assert!(
        !refusal.contains(&wrong),
        "a refusal must not echo the presented credential"
    );
    seen.push(refusal);

    // Normal traffic and refused calls, all collected.
    let submitted = client
        .submit_note(SubmitNoteRequest {
            idempotency_key: "k".to_string(),
            title: "Secret".to_string(),
            body: "conținut".to_string(),
        })
        .await
        .unwrap();
    seen.push(serde_json::to_string(&submitted).unwrap());
    let waiting = wait_for(
        &client,
        submitted.run_id,
        RunStatus::AwaitingApproval,
        &mut seen,
    )
    .await;
    let pending = waiting.pending_approval.unwrap();
    let tampered = client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: "0".repeat(64),
            decision: ApprovalDecision::Approve,
        })
        .await
        .unwrap_err();
    seen.push(format!("{tampered} {tampered:?}"));
    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision: ApprovalDecision::Approve,
        })
        .await
        .unwrap();
    let done = wait_for(&client, submitted.run_id, RunStatus::Succeeded, &mut seen).await;
    seen.push(
        client
            .artifact_content(done.artifact.unwrap().artifact_id)
            .await
            .unwrap(),
    );
    seen.push(serde_json::to_string(&client.snapshot(50).await.unwrap()).unwrap());
    seen.push(serde_json::to_string(&client.events(0, 500).await.unwrap()).unwrap());
    let invalid = client
        .submit_note(SubmitNoteRequest {
            idempotency_key: String::new(),
            title: String::new(),
            body: String::new(),
        })
        .await
        .unwrap_err();
    seen.push(format!("{invalid} {invalid:?}"));
    let backup = client.create_backup().await.unwrap();
    seen.push(serde_json::to_string(&backup).unwrap());

    client.stop_host().await.unwrap();
    drop(client);
    assert!(wait_exit(&mut host).success());

    let log = std::fs::read_to_string(profile.path().join("logs").join("nexus-host.log")).unwrap();
    assert!(
        log.contains("handshake accepted"),
        "the log must actually record activity"
    );
    assert!(
        log.contains("rejected handshake"),
        "the refused attempt must be logged (without the credential)"
    );
    assert!(
        !log.contains(&wrong),
        "a refused credential must not be logged"
    );
    seen.push(log);

    for text in &seen {
        assert!(
            !text.contains(&secret),
            "the pairing secret leaked into: {text}"
        );
    }

    // A backup never carries the secret: no token file, and no file that
    // contains its bytes (the database does not store it).
    for file in files_under(std::path::Path::new(&backup.path)) {
        assert_ne!(file.file_name().unwrap(), "host.pairing-token");
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "the secret is inside backup file {}",
            file.display()
        );
    }
}
