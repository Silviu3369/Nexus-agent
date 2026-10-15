mod bindings;
mod host;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(host::HostConnection::default())
        .invoke_handler(tauri::generate_handler![
            host::host_status,
            host::start_host
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
