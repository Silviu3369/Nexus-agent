mod bindings;
mod chat;
mod host;
mod runs;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(host::HostConnection::default())
        .invoke_handler(tauri::generate_handler![
            host::host_status,
            host::start_host,
            runs::submit_note,
            runs::list_runs,
            runs::get_run,
            runs::decide_approval,
            runs::cancel_run,
            runs::artifact_content,
            runs::reveal_artifact,
            runs::create_backup,
            chat::list_connections,
            chat::save_gemini_connection,
            chat::remove_connection,
            chat::list_models,
            chat::select_model,
            chat::set_thinking_level,
            chat::get_activity,
            chat::get_persona,
            chat::save_persona,
            chat::list_conversations,
            chat::get_conversation,
            chat::send_message,
            chat::cancel_reply,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
