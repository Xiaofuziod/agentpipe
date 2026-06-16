use crate::bridge;
use crate::state::{ActiveRun, AppState};
use agentpipe_engine::control::Control;
use agentpipe_engine::executor::RunnerBins;
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::Command;
use std::sync::Arc;
use tauri::{AppHandle, State};

fn runner_bins() -> RunnerBins {
    RunnerBins {
        claude: std::env::var("AGENTPIPE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        codex: std::env::var("AGENTPIPE_CODEX_BIN").unwrap_or_else(|_| "codex".into()),
    }
}

fn templates_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates")
}

#[tauri::command]
pub fn start_run(app: AppHandle, state: State<AppState>, path: String) -> Result<(), String> {
    let mut active = state.active.lock().unwrap();
    if active.is_some() {
        return Err("已有运行中的 Run,请先结束".into());
    }
    let yaml = std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败: {e}"))?;
    let manifest = Manifest::parse(&yaml).map_err(|e| e.to_string())?;
    manifest.validate().map_err(|e| e.to_string())?;

    // Control 在 Task 6 接入引擎;此处先建并存,供后续 Abort 使用
    let control = Arc::new(Control::default());
    let started = bridge::start(app, manifest, runner_bins());
    *active = Some(ActiveRun {
        commands: started.commands,
        control,
    });
    Ok(())
}

#[tauri::command]
pub fn send_command(state: State<AppState>, cmd: Command) -> Result<(), String> {
    let active = state.active.lock().unwrap();
    match active.as_ref() {
        Some(run) => run.commands.send(cmd).map_err(|e| e.to_string()),
        None => Err("没有运行中的 Run".into()),
    }
}

#[tauri::command]
pub fn save_manifest(manifest: Manifest, path: String) -> Result<(), String> {
    manifest.validate().map_err(|e| e.to_string())?;
    let yaml = serde_yml::to_string(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(&path, yaml).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_templates() -> Result<Vec<String>, String> {
    let dir = templates_dir();
    let mut names = vec![];
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().and_then(|s| s.to_str()) == Some("yaml") {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    Ok(names)
}

/// 解析模板 YAML 返回 Manifest(webview 无 YAML 解析器,复用 Rust serde)。
#[tauri::command]
pub fn load_template(name: String) -> Result<Manifest, String> {
    if name.contains('/') || name.contains("..") {
        return Err("非法模板名".into());
    }
    let p = templates_dir().join(format!("{name}.yaml"));
    let yaml = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    Manifest::parse(&yaml).map_err(|e| e.to_string())
}
