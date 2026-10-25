//! The trusted confirmation path (ADR-POLICY-001, APPROVAL-01). An effect
//! outside Nexus (a file in a shared folder, an MCP call that can change
//! something) is approved only here: the page only asks; this code reads the
//! waiting request from the Host itself, shows it in a native Windows dialog
//! the page can neither draw nor click, and sends the approval only if the
//! user chooses Yes there. The page cannot reach the Host directly, so it
//! cannot claim this path.

use std::sync::atomic::{AtomicBool, Ordering};

use nexus_protocol::{
    ApprovalDecision, ApprovalId, ApprovalPath, DecideApprovalRequest, RunId, RunKind, RunView,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
};

use crate::host::{BridgeError, HostConnection};

/// The longest piece of a call's arguments the dialog shows (the window
/// shows them whole).
const MAX_ARGUMENTS_CHARS: usize = 400;

/// One dialog at a time: a page asking again and again gets a refusal, not
/// a pile of dialogs.
static OPEN: AtomicBool = AtomicBool::new(false);

pub(crate) struct Open;

impl Open {
    pub(crate) fn take() -> Option<Self> {
        OPEN.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Open)
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        OPEN.store(false, Ordering::Release);
    }
}

fn refused(message: &str) -> BridgeError {
    BridgeError {
        code: None,
        message: message.to_string(),
    }
}

/// What the dialog says, all of it from the Host's own view of the run.
fn dialog_text(run: &RunView) -> String {
    let pending = run.pending_approval.as_ref();
    let (headline, what) = match run.kind {
        RunKind::Document => (
            "Create a file outside Nexus?",
            match pending.and_then(|p| p.preview.as_ref()) {
                Some(p) => format!("File: {}\nSize: {} bytes", p.destination, p.size_bytes),
                None => format!("File: {}", run.title),
            },
        ),
        RunKind::McpCall => (
            "Call a tool that can change something outside Nexus?",
            match run.call.as_ref() {
                Some(c) => {
                    let mut arguments: String =
                        c.arguments_json.chars().take(MAX_ARGUMENTS_CHARS).collect();
                    if c.arguments_json.chars().count() > MAX_ARGUMENTS_CHARS {
                        arguments.push('…');
                    }
                    format!(
                        "Server: {} ({})\nTool: {}\nArguments: {}",
                        c.server, c.url, c.tool, arguments
                    )
                }
                None => run.title.clone(),
            },
        ),
        RunKind::Note | RunKind::RecordBatch => ("Approve this action?", run.title.clone()),
    };
    let undo = match run.effect_class.as_deref() {
        Some("ExternalReversible") => "It can be undone later, while the file is unchanged.",
        Some("ExternalIrreversible") => "It cannot be undone.",
        _ => "",
    };
    let asked = match run.requested_by.as_ref() {
        Some(a) if a.kind == "User" => "you".to_string(),
        Some(a) => a.name.clone(),
        None => "Nexus".to_string(),
    };
    format!(
        "{headline}\n\n{what}\n\n{undo}\n\nAsked by {asked}. Approve only if this is what the Nexus window shows."
    )
}

/// Shows the native dialog, owned by the app's window; true for Yes. "No"
/// is the default button, so a stray Enter does not approve.
pub(crate) fn ask(owner: isize, text: &str) -> bool {
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let caption: Vec<u16> = "Nexus - Confirm".encode_utf16().chain(Some(0)).collect();
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive
    // the call; `owner` is the app's own window handle (or null).
    let answer = unsafe {
        MessageBoxW(
            owner as windows_sys::Win32::Foundation::HWND,
            text.as_ptr(),
            caption.as_ptr(),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND,
        )
    };
    answer == IDYES
}

/// Approves the waiting effect if the user says Yes in the Windows dialog;
/// `None` when they did not (nothing is decided).
#[tauri::command]
pub async fn confirm_approval(
    window: tauri::WebviewWindow,
    connection: tauri::State<'_, HostConnection>,
    run_id: RunId,
    approval_id: ApprovalId,
    manifest_sha256: String,
) -> Result<Option<RunView>, BridgeError> {
    let Some(_open) = Open::take() else {
        return Err(refused("a confirmation is already open"));
    };
    let run = connection.call(async |c| c.run(run_id).await).await?;
    let waiting = run
        .pending_approval
        .as_ref()
        .is_some_and(|p| p.approval_id == approval_id && p.manifest_sha256 == manifest_sha256);
    if !waiting {
        return Err(refused(
            "this is no longer the request waiting for approval; look at it again",
        ));
    }
    let text = dialog_text(&run);
    let owner = window.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    let yes = tauri::async_runtime::spawn_blocking(move || ask(owner, &text))
        .await
        .map_err(|e| refused(&e.to_string()))?;
    if !yes {
        return Ok(None);
    }
    connection
        .call(async |c| {
            c.decide_approval(DecideApprovalRequest {
                approval_id,
                manifest_sha256,
                decision: ApprovalDecision::Approve,
                via: ApprovalPath::TrustedDialog,
            })
            .await
        })
        .await
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_protocol::{ActorView, McpCallView, NotePreview, PendingApproval, RunStatus};

    fn run(kind: RunKind, class: &str) -> RunView {
        RunView {
            run_id: RunId::new(),
            title: "Cover letter Acme.docx in Jobs".into(),
            status: RunStatus::AwaitingApproval,
            effect_class: Some(class.into()),
            evidence: None,
            undone_at_ms: None,
            can_undo: false,
            revision: 1,
            failure: None,
            pending_approval: Some(PendingApproval {
                approval_id: ApprovalId::new(),
                manifest_sha256: "abc".into(),
                expires_at_ms: 0,
                preview: Some(NotePreview {
                    destination: "C:\\Jobs\\Cover letter Acme.docx".into(),
                    content: "Dear Acme".into(),
                    size_bytes: 9,
                    sha256: "abc".into(),
                }),
                call: None,
                records: None,
            }),
            artifact: None,
            origin: None,
            kind,
            call: None,
            requested_by: Some(ActorView {
                kind: "Agent".into(),
                name: "Nexus".into(),
            }),
            decided_by: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn the_dialog_names_the_exact_file_and_whether_it_can_be_undone() {
        let text = dialog_text(&run(RunKind::Document, "ExternalReversible"));
        assert!(text.starts_with("Create a file outside Nexus?"), "{text}");
        assert!(text.contains("File: C:\\Jobs\\Cover letter Acme.docx\nSize: 9 bytes"));
        assert!(text.contains("can be undone later"));
        assert!(text.contains("Asked by Nexus."));
    }

    #[test]
    fn the_dialog_names_the_call_and_cuts_long_arguments() {
        let mut call = run(RunKind::McpCall, "ExternalIrreversible");
        call.call = Some(McpCallView {
            server: "Docs".into(),
            url: "https://docs.example/mcp".into(),
            tool: "delete_page".into(),
            arguments_json: format!("{{\"q\": \"{}\"}}", "x".repeat(1000)),
            result_chars: None,
            truncated: None,
            duration_ms: None,
        });
        let text = dialog_text(&call);
        assert!(text.contains("Server: Docs (https://docs.example/mcp)\nTool: delete_page"));
        assert!(text.contains("It cannot be undone."));
        assert!(text.contains('…') && text.len() < 1000, "{text}");
    }

    #[test]
    fn only_one_dialog_is_open_at_a_time() {
        let first = Open::take().expect("free");
        assert!(Open::take().is_none(), "a second request is refused");
        drop(first);
        assert!(Open::take().is_some(), "free again once closed");
    }
}
