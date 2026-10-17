mod automation;
mod autostart;
mod bindings;
mod chat;
mod host;
mod mcp;
mod memory;
mod runs;
mod usage;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(host::HostConnection::default())
        .invoke_handler(tauri::generate_handler![
            host::host_status,
            host::start_host,
            autostart::get_autostart,
            autostart::set_autostart,
            runs::submit_note,
            runs::list_runs,
            runs::get_run,
            runs::decide_approval,
            runs::cancel_run,
            runs::artifact_content,
            runs::reveal_artifact,
            runs::create_backup,
            runs::get_events,
            runs::get_system_status,
            chat::list_connections,
            chat::save_connection,
            chat::get_backup_model,
            chat::set_backup_model,
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
            chat::delete_conversation,
            chat::delete_exchange,
            memory::list_projects,
            memory::save_project,
            memory::set_active_project,
            memory::list_memory,
            memory::add_memory,
            memory::edit_memory,
            memory::decide_memory,
            memory::delete_memory,
            memory::memory_context,
            automation::list_automations,
            automation::save_automation,
            automation::delete_automation,
            automation::run_automation_now,
            automation::automation_runs,
            mcp::list_mcp_servers,
            mcp::save_mcp_server,
            mcp::refresh_mcp_tools,
            mcp::remove_mcp_server,
            mcp::set_mcp_tool_enabled,
            usage::get_usage,
            usage::set_daily_token_budget,
            usage::set_model_price,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
