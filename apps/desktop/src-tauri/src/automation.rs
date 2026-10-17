//! Bridge commands for automations. The schedule is checked, and the next
//! time computed, in the host; the webview only shows what it returns.

use nexus_protocol::{
    AutomationId, AutomationRunView, AutomationView, ProjectId, SaveAutomationRequest, Schedule,
};

use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn list_automations(connection: State<'_>) -> Result<Vec<AutomationView>, BridgeError> {
    connection.call(async |c| c.automations().await).await
}

/// `automation_id: None` creates one.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn save_automation(
    connection: State<'_>,
    automation_id: Option<AutomationId>,
    revision: i64,
    name: String,
    prompt: String,
    project_id: Option<ProjectId>,
    schedule: Schedule,
    enabled: bool,
    catch_up: bool,
) -> Result<AutomationView, BridgeError> {
    connection
        .call(async |c| {
            c.save_automation(SaveAutomationRequest {
                automation_id,
                revision,
                name,
                prompt,
                project_id,
                schedule,
                enabled,
                catch_up,
            })
            .await
        })
        .await
}

#[tauri::command]
pub async fn delete_automation(
    connection: State<'_>,
    automation_id: AutomationId,
) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.delete_automation(automation_id).await)
        .await
}

#[tauri::command]
pub async fn run_automation_now(
    connection: State<'_>,
    automation_id: AutomationId,
) -> Result<AutomationRunView, BridgeError> {
    connection
        .call(async |c| c.run_automation_now(automation_id).await)
        .await
}

#[tauri::command]
pub async fn automation_runs(
    connection: State<'_>,
    automation_id: AutomationId,
    limit: u32,
) -> Result<Vec<AutomationRunView>, BridgeError> {
    connection
        .call(async |c| c.automation_runs(automation_id, limit).await)
        .await
}
