//! Bridge commands for the user's skills (ADR-PACK-001). The host reads the
//! folder or `.zip` the user names and checks it; turning a skill on names
//! the revision the user read.

use nexus_protocol::{SetSkillEnabledRequest, SkillContentView, SkillId, SkillView};

use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn list_skills(connection: State<'_>) -> Result<Vec<SkillView>, BridgeError> {
    connection.call(async |c| c.skills().await).await
}

#[tauri::command]
pub async fn get_skill(
    connection: State<'_>,
    skill_id: SkillId,
) -> Result<SkillContentView, BridgeError> {
    connection.call(async |c| c.skill(skill_id).await).await
}

#[tauri::command]
pub async fn import_skill(connection: State<'_>, path: String) -> Result<SkillView, BridgeError> {
    connection.call(async |c| c.import_skill(path).await).await
}

#[tauri::command]
pub async fn set_skill_enabled(
    connection: State<'_>,
    request: SetSkillEnabledRequest,
) -> Result<SkillView, BridgeError> {
    connection
        .call(async |c| c.set_skill_enabled(request).await)
        .await
}

#[tauri::command]
pub async fn remove_skill(connection: State<'_>, skill_id: SkillId) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.remove_skill(skill_id).await)
        .await
}
