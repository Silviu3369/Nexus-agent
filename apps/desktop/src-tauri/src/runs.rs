//! Bridge commands for the M1 note circuit. They pass IDs and user text to
//! the host and return its answers; every decision is made by the host.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use nexus_protocol::{
    ApprovalDecision, ApprovalId, ArtifactId, DecideApprovalRequest, RunId, RunStatus, RunView,
    Snapshot, SubmitNoteRequest, SubmitNoteResponse,
};

use crate::host::{BridgeError, HostConnection};

const DASHBOARD_RUNS: u32 = 50;

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn submit_note(
    connection: State<'_>,
    idempotency_key: String,
    title: String,
    body: String,
) -> Result<SubmitNoteResponse, BridgeError> {
    connection
        .call(async |c| {
            c.submit_note(SubmitNoteRequest {
                idempotency_key,
                title,
                body,
            })
            .await
        })
        .await
}

#[tauri::command]
pub async fn list_runs(connection: State<'_>) -> Result<Snapshot, BridgeError> {
    connection
        .call(async |c| c.snapshot(DASHBOARD_RUNS).await)
        .await
}

#[tauri::command]
pub async fn get_run(connection: State<'_>, run_id: RunId) -> Result<RunView, BridgeError> {
    connection.call(async |c| c.run(run_id).await).await
}

/// `manifest_sha256` must be the one the user was shown; the host refuses a
/// decision for anything else.
#[tauri::command]
pub async fn decide_approval(
    connection: State<'_>,
    approval_id: ApprovalId,
    manifest_sha256: String,
    approve: bool,
) -> Result<RunView, BridgeError> {
    let decision = if approve {
        ApprovalDecision::Approve
    } else {
        ApprovalDecision::Deny
    };
    connection
        .call(async |c| {
            c.decide_approval(DecideApprovalRequest {
                approval_id,
                manifest_sha256,
                decision,
            })
            .await
        })
        .await
}

#[tauri::command]
pub async fn cancel_run(connection: State<'_>, run_id: RunId) -> Result<RunView, BridgeError> {
    connection.call(async |c| c.cancel_run(run_id).await).await
}

#[tauri::command]
pub async fn artifact_content(
    connection: State<'_>,
    artifact_id: ArtifactId,
) -> Result<String, BridgeError> {
    connection
        .call(async |c| c.artifact_content(artifact_id).await)
        .await
}

/// Shows a succeeded run's note selected in File Explorer. Nothing is
/// executed or opened with an associated program.
#[tauri::command]
pub async fn reveal_artifact(connection: State<'_>, run_id: RunId) -> Result<(), BridgeError> {
    let run = connection.call(async |c| c.run(run_id).await).await?;
    let artifact = match (run.status, run.artifact) {
        (RunStatus::Succeeded, Some(artifact)) => artifact,
        _ => return Err(local_error("this run has no verified note to show")),
    };
    let path = checked_artifact_path(Path::new(&artifact.path))?;
    std::process::Command::new("explorer.exe")
        // explorer wants `/select,"<path>"` verbatim; a Windows path cannot
        // contain a double quote, so no escaping is needed.
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| local_error(&format!("could not open File Explorer: {e}")))
}

/// Defence in depth on the host's answer: only an existing `.md` file inside
/// this profile's artifact store is ever passed to Explorer.
fn checked_artifact_path(path: &Path) -> Result<PathBuf, BridgeError> {
    let store = nexus_protocol::pairing::profile_dir().join("artifacts");
    let canonical = dunce_canonical(path)?;
    let store = dunce_canonical(&store)?;
    if !canonical.starts_with(&store) || canonical.extension().is_none_or(|e| e != "md") {
        return Err(local_error(
            "refusing a path outside the Nexus artifact store",
        ));
    }
    Ok(canonical)
}

/// `canonicalize` on Windows returns a `\\?\` path, which Explorer does not
/// accept; strip the prefix for local drive paths.
fn dunce_canonical(path: &Path) -> Result<PathBuf, BridgeError> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| local_error(&format!("{}: {e}", path.display())))?;
    let text = canonical.to_string_lossy();
    Ok(match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => canonical,
    })
}

fn local_error(message: &str) -> BridgeError {
    BridgeError {
        code: None,
        message: message.to_string(),
    }
}
