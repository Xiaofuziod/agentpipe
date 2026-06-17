#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod bridge;
mod commands;
mod state;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::start_run,
            commands::start_run_inline,
            commands::send_command,
            commands::save_manifest,
            commands::list_templates,
            commands::load_template,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
