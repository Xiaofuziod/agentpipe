use crate::bridge;
use crate::state::{ActiveRun, AppState};
use agentpipe_engine::executor::RunnerBins;
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::Command;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager, State};
use agentpipe_engine::audit::{self, RunEntry};
use agentpipe_engine::protocol::Event;

fn runner_bins() -> RunnerBins {
    RunnerBins {
        claude: std::env::var("AGENTPIPE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        codex: std::env::var("AGENTPIPE_CODEX_BIN").unwrap_or_else(|_| "codex".into()),
    }
}

/// 模板目录:安装后读打进 bundle 的资源(自包含,与 tauri.conf.json > bundle > resources
/// 同一相对路径语法);dev 模式或资源缺失时兜底回源码仓 templates/。
fn templates_dir(app: &AppHandle) -> std::path::PathBuf {
    if let Ok(p) = app.path().resolve("../templates", BaseDirectory::Resource) {
        if p.is_dir() {
            return p;
        }
    }
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
    // 与 save_manifest 用同一解析器,保证"存到哪就跑哪"(~ 展开 / 裸名落 tasks_dir)
    let resolved = crate::paths::resolve_task_path(&path);
    // 读/解析在锁外做,不持 AppState 锁跨阻塞 I/O(避免阻塞 send_command / 清理)
    let yaml = std::fs::read_to_string(&resolved)
        .map_err(|e| format!("读取 {} 失败: {e}", resolved.display()))?;
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
    // 安装后 GUI 进程 cwd=/(只读),裸名 / 相对路径会写到只读根 → EROFS。
    // 统一经 resolve_task_path 落到可写的 ~/.agentpipe/tasks/(或用户给的绝对路径)。
    let resolved = crate::paths::resolve_task_path(&path);
    if let Some(parent) = resolved.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
    }
    std::fs::write(&resolved, yaml).map_err(|e| format!("写入 {} 失败: {e}", resolved.display()))?;
    Ok(())
}

#[tauri::command]
pub fn list_templates(app: AppHandle) -> Result<Vec<String>, String> {
    let dir = templates_dir(&app);
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
pub fn load_template(app: AppHandle, name: String) -> Result<Manifest, String> {
    if name.contains('/') || name.contains("..") {
        return Err("非法模板名".into());
    }
    let p = templates_dir(&app).join(format!("{name}.yaml"));
    let yaml = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    Manifest::parse(&yaml).map_err(|e| e.to_string())
}

// ==== 审计读命令 ====

#[derive(serde::Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub name: String,
    pub target: String,
    pub status: Option<String>,
    pub total_cost_usd: f64,
    pub total_turns: u32,
    pub step_count: usize,
    pub complete: bool,
}

#[derive(serde::Serialize)]
pub struct DiffRow {
    pub step_id: String,
    pub kind: String, // only_a | only_b | changed
    pub a_status: Option<String>,
    pub a_cost: Option<f64>,
    pub b_status: Option<String>,
    pub b_cost: Option<f64>,
}

fn collect_runs(dir: &std::path::Path) -> Result<Vec<RunSummary>, String> {
    let mut ids: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".ndjson").map(str::to_string))
            .filter(|id| audit::is_valid_run_id(id))
            .collect(),
        Err(_) => return Ok(vec![]), // 目录不存在 = 无历史
    };
    ids.sort();
    ids.reverse();
    let mut out = Vec::new();
    for id in ids {
        let path = dir.join(format!("{id}.ndjson"));
        let entries = audit::read_run(&path).map_err(|e| e.to_string())?;
        let s = audit::run_summary(&entries);
        out.push(RunSummary {
            run_id: id,
            name: s.name,
            target: s.target,
            status: s.status,
            total_cost_usd: s.total_cost_usd,
            total_turns: s.total_turns,
            step_count: s.step_count,
            complete: s.complete,
        });
    }
    Ok(out)
}

fn build_diff(a: &[RunEntry], b: &[RunEntry]) -> Vec<DiffRow> {
    let (fa, fb) = (audit::step_finals(a), audit::step_finals(b));
    let mut keys: Vec<&String> = fa.keys().chain(fb.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut rows = Vec::new();
    for k in keys {
        match (fa.get(k), fb.get(k)) {
            (Some(x), None) => rows.push(DiffRow {
                step_id: k.clone(),
                kind: "only_a".into(),
                a_status: Some(x.status.clone()),
                a_cost: Some(x.cost_usd),
                b_status: None,
                b_cost: None,
            }),
            (None, Some(y)) => rows.push(DiffRow {
                step_id: k.clone(),
                kind: "only_b".into(),
                a_status: None,
                a_cost: None,
                b_status: Some(y.status.clone()),
                b_cost: Some(y.cost_usd),
            }),
            (Some(x), Some(y)) if x != y => rows.push(DiffRow {
                step_id: k.clone(),
                kind: "changed".into(),
                a_status: Some(x.status.clone()),
                a_cost: Some(x.cost_usd),
                b_status: Some(y.status.clone()),
                b_cost: Some(y.cost_usd),
            }),
            _ => {}
        }
    }
    rows
}

fn run_path_checked(run_id: &str) -> Result<std::path::PathBuf, String> {
    if !audit::is_valid_run_id(run_id) {
        return Err(format!("非法 run-id: {run_id}"));
    }
    Ok(crate::paths::runs_dir().join(format!("{run_id}.ndjson")))
}

fn view_run_impl(run_id: &str) -> Result<Vec<Event>, String> {
    let path = run_path_checked(run_id)?;
    let entries = audit::read_run(&path).map_err(|e| e.to_string())?;
    Ok(entries.into_iter().map(|e| e.event).collect())
}

#[tauri::command]
pub fn list_runs() -> Result<Vec<RunSummary>, String> {
    collect_runs(&crate::paths::runs_dir())
}

#[tauri::command]
pub fn view_run(run_id: String) -> Result<Vec<Event>, String> {
    view_run_impl(&run_id)
}

#[tauri::command]
pub fn diff_runs(a: String, b: String) -> Result<Vec<DiffRow>, String> {
    let pa = run_path_checked(&a)?;
    let pb = run_path_checked(&b)?;
    let ea = audit::read_run(&pa).map_err(|e| e.to_string())?;
    let eb = audit::read_run(&pb).map_err(|e| e.to_string())?;
    Ok(build_diff(&ea, &eb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentpipe_engine::audit::RunRecorder;
    use agentpipe_engine::protocol::{RunStatus, StepStatus, StepMetrics};
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aptauri-{}-{tag}", std::process::id()))
    }

    fn write_run(dir: &std::path::Path, name: &str, cost: f64, finish: bool) -> String {
        let mut r = RunRecorder::open(dir, name).unwrap();
        r.record(&Event::RunStarted { name: name.into(), target: "/repo/demo".into() });
        r.record(&Event::StepFinished {
            step_id: "a".into(),
            status: StepStatus::Done,
            summary: "".into(),
            metrics: Some(StepMetrics {
                num_turns: 1,
                duration_ms: 1000,
                cost_usd: cost,
            }),
        });
        if finish {
            r.record(&Event::RunFinished {
                status: RunStatus::Success,
            });
        }
        r.run_id().to_string()
    }

    #[test]
    fn collect_runs_summarizes_and_sorts_desc() {
        let dir = tmp("list");
        let _ = write_run(&dir, "one", 0.10, true);
        let _ = write_run(&dir, "two", 0.20, true);
        let runs = collect_runs(&dir).unwrap();
        assert_eq!(runs.len(), 2);
        // 倒序:run-id 时间戳前缀大的在前(两次同名不同 → 退避序号;断言总成本可读)
        assert!(runs.iter().all(|r| r.total_cost_usd > 0.0));
        assert!(runs[0].run_id >= runs[1].run_id, "应按 run_id 倒序(最新在前): {:?}", runs.iter().map(|r| &r.run_id).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_diff_buckets_only_and_changed() {
        let dir = tmp("diff");
        let a = write_run(&dir, "a", 0.10, true);
        let b = write_run(&dir, "b", 0.99, true);
        let ea = agentpipe_engine::audit::read_run(&dir.join(format!("{a}.ndjson"))).unwrap();
        let eb = agentpipe_engine::audit::read_run(&dir.join(format!("{b}.ndjson"))).unwrap();
        let rows = build_diff(&ea, &eb);
        // 同一 step "a" 成本不同 → changed
        assert!(rows.iter().any(|r| r.step_id == "a" && r.kind == "changed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn view_run_rejects_traversal() {
        assert!(view_run_impl("../etc/passwd").is_err());
    }
}
