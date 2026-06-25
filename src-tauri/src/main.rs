#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod bridge;
mod commands;
mod paths;
mod shellpath;
mod state;

fn main() {
    // Finder/Dock 启动的 .app 拿不到终端 PATH → 修回真实 PATH,否则 spawn claude/codex ENOENT。
    // 必须在启动引擎(spawn 子进程)前做;放 main 最前,所有后续 spawn 都继承修好的 env。
    shellpath::repair_path_from_login_shell();

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
            commands::list_runs,
            commands::view_run,
            commands::delete_run,
            commands::diff_runs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
