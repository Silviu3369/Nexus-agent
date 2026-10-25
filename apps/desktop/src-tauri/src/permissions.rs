//! Bridge commands for the Permissions page (ADR-POLICY-001). Turning a
//! tool off or letting fewer register writes run without asking tightens
//! the policy and is sent from the window. Loosening it (a tool back on,
//! more writes without asking) is confirmed in the native Windows dialog
//! this code shows itself, with the change read from the Host; the Host
//! refuses it from any other path.

use nexus_protocol::{ApprovalPath, PermissionView, SetPermissionRequest};

use crate::confirm::{Open, ask};
use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

fn refused(message: &str) -> BridgeError {
    BridgeError {
        code: None,
        message: message.to_string(),
    }
}

#[tauri::command]
pub async fn list_permissions(connection: State<'_>) -> Result<Vec<PermissionView>, BridgeError> {
    connection.call(async |c| c.permissions().await).await
}

/// A change from the window: the Host accepts only a tightening this way.
#[tauri::command]
pub async fn set_permission(
    connection: State<'_>,
    tool_id: String,
    enabled: bool,
    max_auto: Option<u32>,
) -> Result<Vec<PermissionView>, BridgeError> {
    connection
        .call(async |c| {
            c.set_permission(SetPermissionRequest {
                tool_id,
                enabled,
                max_auto,
                via: ApprovalPath::Window,
            })
            .await
        })
        .await
}

/// What the dialog asks, from the Host's own view of the tool.
fn relax_text(now: &PermissionView, enabled: bool, max_auto: Option<u32>) -> String {
    let mut text = if enabled && !now.enabled {
        format!(
            "Turn this back on?\n\n{}\nWhat it sends: {}",
            now.label, now.sends
        )
    } else {
        String::new()
    };
    if let (Some(after), Some(before)) = (max_auto, now.max_auto)
        && after > before
    {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "Let Nexus make up to {after} register changes per reply without asking (now {before})?\nBeyond them, its changes wait for you in one batch."
        ));
    }
    text
}

/// A change that loosens the policy, confirmed in the Windows dialog;
/// `None` when the user said No there.
#[tauri::command]
pub async fn relax_permission(
    window: tauri::WebviewWindow,
    connection: State<'_>,
    tool_id: String,
    enabled: bool,
    max_auto: Option<u32>,
) -> Result<Option<Vec<PermissionView>>, BridgeError> {
    let Some(_open) = Open::take() else {
        return Err(refused("a confirmation is already open"));
    };
    let all = connection.call(async |c| c.permissions().await).await?;
    let now = all
        .iter()
        .find(|p| p.tool_id == tool_id)
        .ok_or_else(|| refused("no such tool"))?;
    let text = relax_text(now, enabled, max_auto);
    if text.is_empty() {
        return Err(refused(
            "this change does not loosen anything; save it from the window",
        ));
    }
    let owner = window.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    let yes = tauri::async_runtime::spawn_blocking(move || ask(owner, &text))
        .await
        .map_err(|e| refused(&e.to_string()))?;
    if !yes {
        return Ok(None);
    }
    connection
        .call(async |c| {
            c.set_permission(SetPermissionRequest {
                tool_id,
                enabled,
                max_auto,
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

    fn view(enabled: bool, max_auto: Option<u32>) -> PermissionView {
        PermissionView {
            tool_id: "RecordWrite".into(),
            label: "Add and change records in your registers".into(),
            effect_class: "InternalReversible".into(),
            sends: "nothing outside Nexus".into(),
            enabled,
            decision: "Auto".into(),
            max_auto,
            default_max_auto: Some(10),
            changed_at_ms: None,
        }
    }

    #[test]
    fn the_dialog_says_what_loosens_and_nothing_else() {
        let on = relax_text(&view(false, Some(10)), true, Some(10));
        assert!(on.starts_with("Turn this back on?"), "{on}");
        assert!(on.contains("What it sends: nothing outside Nexus"));
        let more = relax_text(&view(true, Some(10)), true, Some(20));
        assert!(more.contains("up to 20 register changes per reply without asking (now 10)"));
        assert!(
            relax_text(&view(true, Some(10)), true, Some(5)).is_empty(),
            "fewer is not a loosening"
        );
        assert!(relax_text(&view(true, Some(10)), false, Some(10)).is_empty());
    }
}
