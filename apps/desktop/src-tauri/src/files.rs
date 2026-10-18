//! Bridge commands for the folders the user shares, and for Excel export
//! and import of registers. Every path is resolved and checked by the host;
//! the webview only names a shared folder and a path inside it.

use nexus_protocol::{
    ExportView, FolderEntryView, FolderId, FolderView, ImportPreviewView, ImportRequest,
    ImportResultView, RegisterId, SaveFolderRequest,
};

use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn list_folders(connection: State<'_>) -> Result<Vec<FolderView>, BridgeError> {
    connection.call(async |c| c.folders().await).await
}

#[tauri::command]
pub async fn save_folder(
    connection: State<'_>,
    request: SaveFolderRequest,
) -> Result<FolderView, BridgeError> {
    connection
        .call(async |c| c.save_folder(request).await)
        .await
}

#[tauri::command]
pub async fn remove_folder(connection: State<'_>, folder_id: FolderId) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.remove_folder(folder_id).await)
        .await
}

#[tauri::command]
pub async fn folder_entries(
    connection: State<'_>,
    folder_id: FolderId,
) -> Result<Vec<FolderEntryView>, BridgeError> {
    connection
        .call(async |c| c.folder_entries(folder_id).await)
        .await
}

#[tauri::command]
pub async fn export_register(
    connection: State<'_>,
    register_id: RegisterId,
    folder_id: FolderId,
) -> Result<ExportView, BridgeError> {
    connection
        .call(async |c| c.export_register(register_id, folder_id).await)
        .await
}

#[tauri::command]
pub async fn import_preview(
    connection: State<'_>,
    register_id: RegisterId,
    folder_id: FolderId,
    path: String,
) -> Result<ImportPreviewView, BridgeError> {
    connection
        .call(async |c| c.import_preview(register_id, folder_id, path).await)
        .await
}

#[tauri::command]
pub async fn import_records(
    connection: State<'_>,
    request: ImportRequest,
) -> Result<ImportResultView, BridgeError> {
    connection
        .call(async |c| c.import_records(request).await)
        .await
}
