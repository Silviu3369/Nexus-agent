//! Bridge commands for registers (job applications first). Values are
//! checked against the register's fields in the host; the webview only
//! shows what it returns.

use nexus_protocol::{
    InboxView, RecordDetailView, RecordId, RecordQuery, RecordView, RegisterId, RegisterView,
    SaveInboxRequest, SaveRecordRequest,
};

use crate::host::{BridgeError, HostConnection};

type State<'a> = tauri::State<'a, HostConnection>;

#[tauri::command]
pub async fn list_registers(connection: State<'_>) -> Result<Vec<RegisterView>, BridgeError> {
    connection.call(async |c| c.registers().await).await
}

#[tauri::command]
pub async fn list_records(
    connection: State<'_>,
    query: RecordQuery,
) -> Result<Vec<RecordView>, BridgeError> {
    connection.call(async |c| c.records(query).await).await
}

#[tauri::command]
pub async fn record_detail(
    connection: State<'_>,
    record_id: RecordId,
) -> Result<RecordDetailView, BridgeError> {
    connection
        .call(async |c| c.record_detail(record_id).await)
        .await
}

#[tauri::command]
pub async fn save_record(
    connection: State<'_>,
    request: SaveRecordRequest,
) -> Result<RecordView, BridgeError> {
    connection
        .call(async |c| c.save_record(request).await)
        .await
}

#[tauri::command]
pub async fn delete_record(connection: State<'_>, record_id: RecordId) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.delete_record(record_id).await)
        .await
}

#[tauri::command]
pub async fn undo_record_change(connection: State<'_>, change_id: i64) -> Result<(), BridgeError> {
    connection
        .call(async |c| c.undo_record_change(change_id).await)
        .await
}

#[tauri::command]
pub async fn get_inbox(
    connection: State<'_>,
    register_id: RegisterId,
) -> Result<InboxView, BridgeError> {
    connection.call(async |c| c.inbox(register_id).await).await
}

#[tauri::command]
pub async fn save_inbox(
    connection: State<'_>,
    request: SaveInboxRequest,
) -> Result<InboxView, BridgeError> {
    connection.call(async |c| c.save_inbox(request).await).await
}

#[tauri::command]
pub async fn retry_inbox_file(
    connection: State<'_>,
    register_id: RegisterId,
    file_id: i64,
) -> Result<InboxView, BridgeError> {
    connection
        .call(async |c| c.retry_inbox_file(register_id, file_id).await)
        .await
}
