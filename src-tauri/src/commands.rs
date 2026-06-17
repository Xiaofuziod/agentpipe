use crate::bridge;
use crate::state::{ActiveRun, AppState};
use agentpipe_engine::executor::RunnerBins;
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::Command;
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

/// 校验通过的 manifest → 启动引擎(单 Run 不变式:已有活跃 Run 则拒绝)。
/// start_run(从文件) 与 start_run_inline(从对象) 共用此尾段。
fn launch(app: AppHandle, state: &State<AppState>, manifest: Manifest) -> Result<(), String> {
    manifest.validate().map_err(|e| e.to_string())?;
    let mut active = state.active.lock().unwrap();
    if active.is_some() {
        return Err("已有运行中的 Run,请先结束".into());
    }
    let started = bridge::start(app, manifest, runner_bins());
    *active = Some(ActiveRun {
        commands: started.commands,
        control: started.control,
    });
    Ok(())
}

#[tauri::command]
pub fn start_run(app: AppHandle, state: State<AppState>, path: String) -> Result<(), String> {
    // 读/解析在锁外做,不持 AppState 锁跨阻塞 I/O(避免阻塞 send_command / 清理)
    let yaml = std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败: {e}"))?;
    let manifest = Manifest::parse(&yaml).map_err(|e| e.to_string())?;
    launch(app, &state, manifest)
}

/// 快捷运行:直接跑一个 manifest 对象,不落临时文件(控制台底部 prompt 栏用)。
#[tauri::command]
pub fn start_run_inline(app: AppHandle, state: State<AppState>, manifest: Manifest) -> Result<(), String> {
    launch(app, &state, manifest)
}

#[tauri::command]
pub fn send_command(state: State<AppState>, cmd: Command) -> Result<(), String> {
    let active = state.active.lock().unwrap();
    match active.as_ref() {
        Some(run) => {
            // Abort 必须同时杀进程(request_abort)+ 送 channel(解开正等 gate 的 recv)。
            if matches!(cmd, Command::Abort) {
                run.control.request_abort();
            }
            run.commands.send(cmd).map_err(|e| e.to_string())
        }
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
