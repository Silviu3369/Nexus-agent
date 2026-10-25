//! Bridge commands for what data may leave for which AI provider
//! (ADR-PROVIDER-002). Keeping more data in is sent from the window. Letting
//! more leave (the EEA said, a DPA signed, consent given, a source back to
//! the user's own) is confirmed in the native Windows dialog this code shows
//! itself, with the change read from the Host; the Host refuses it from any
//! other path.

use nexus_protocol::{ApprovalPath, DataChange, DataPolicyView, SetDataPolicyRequest};

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
pub async fn get_data_policy(connection: State<'_>) -> Result<DataPolicyView, BridgeError> {
    connection.call(async |c| c.data_policy().await).await
}

/// A change from the window: the Host accepts only keeping more data in.
#[tauri::command]
pub async fn set_data_policy(
    connection: State<'_>,
    change: DataChange,
) -> Result<DataPolicyView, BridgeError> {
    connection
        .call(async |c| {
            c.set_data_policy(SetDataPolicyRequest {
                change,
                via: ApprovalPath::Window,
            })
            .await
        })
        .await
}

/// What the dialog asks, from the Host's own view; empty when the change
/// lets nothing more leave.
fn relax_text(now: &DataPolicyView, change: &DataChange) -> String {
    match change {
        DataChange::InEea { in_eea: true } if !now.in_eea => "You use Nexus from the European Economic Area, Switzerland or the UK?\n\nGoogle then applies its paid data terms to the Gemini free quota, and Nexus lets it receive your own data (never other people's).".to_string(),
        DataChange::Connection {
            connection_id,
            dpa_signed,
            allows_training,
        } => {
            let Some(c) = now.connections.iter().find(|c| c.connection_id == *connection_id) else {
                return String::new();
            };
            let mut text = String::new();
            if *dpa_signed && !c.dpa_signed {
                text.push_str(&format!(
                    "You signed {}'s data processing addendum?\n\nNexus then lets “{}” receive other people's data too.",
                    c.provider, c.display_name
                ));
            }
            if *allows_training && !c.allows_training {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&format!(
                    "Let {} use your own data to improve its models?\n\nNexus then sends your own data to “{}” (never other people's).",
                    c.provider, c.display_name
                ));
            }
            text
        }
        DataChange::Source {
            kind,
            id,
            data_class,
        } if data_class == "Personal" => now
            .sources
            .iter()
            .find(|s| &s.kind == kind && &s.id == id && s.data_class == "Business")
            .map(|s| {
                format!(
                    "Treat “{}” as your own data, not other people's?\n\nNexus then sends it to AI connections that may receive your own data.",
                    s.name
                )
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// A change that lets more data leave, confirmed in the Windows dialog;
/// `None` when the user said No there.
#[tauri::command]
pub async fn relax_data_policy(
    window: tauri::WebviewWindow,
    connection: State<'_>,
    change: DataChange,
) -> Result<Option<DataPolicyView>, BridgeError> {
    let Some(_open) = Open::take() else {
        return Err(refused("a confirmation is already open"));
    };
    let now = connection.call(async |c| c.data_policy().await).await?;
    let text = relax_text(&now, &change);
    if text.is_empty() {
        return Err(refused(
            "this change lets no more data leave; save it from the window",
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
            c.set_data_policy(SetDataPolicyRequest {
                change,
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
    use nexus_protocol::{ConnectionDataView, ConnectionId, SourceDataView};

    fn policy() -> DataPolicyView {
        DataPolicyView {
            in_eea: false,
            terms_checked: "2026-10-25".into(),
            connections: vec![ConnectionDataView {
                connection_id: ConnectionId::new(),
                display_name: "Work".into(),
                provider: "OpenAI".into(),
                free_tier: false,
                can_receive: "Personal".into(),
                reason: String::new(),
                terms_url: String::new(),
                dpa: "Offered".into(),
                dpa_signed: false,
                allows_training: false,
            }],
            sources: vec![SourceDataView {
                kind: "Folder".into(),
                id: "f-1".into(),
                name: "Clients".into(),
                data_class: "Business".into(),
            }],
        }
    }

    #[test]
    fn the_dialog_says_what_more_would_leave_and_nothing_else() {
        let now = policy();
        assert!(
            relax_text(&now, &DataChange::InEea { in_eea: true }).contains("Gemini free quota")
        );
        assert!(relax_text(&now, &DataChange::InEea { in_eea: false }).is_empty());
        let id = now.connections[0].connection_id;
        let signed = relax_text(
            &now,
            &DataChange::Connection {
                connection_id: id,
                dpa_signed: true,
                allows_training: false,
            },
        );
        assert!(signed.contains("OpenAI's data processing addendum") && signed.contains("“Work”"));
        let back = relax_text(
            &now,
            &DataChange::Source {
                kind: "Folder".into(),
                id: "f-1".into(),
                data_class: "Personal".into(),
            },
        );
        assert!(back.contains("“Clients”"));
        assert!(
            relax_text(
                &now,
                &DataChange::Source {
                    kind: "Folder".into(),
                    id: "f-1".into(),
                    data_class: "Business".into(),
                },
            )
            .is_empty(),
            "marking it Business keeps more in"
        );
    }
}
