mod render;

use agentpipe_engine::audit::{event_json_line, RunRecorder};
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, GateKind};
use clap::{Parser, Subcommand};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

#[derive(Parser)]
#[command(name = "agentpipe", about = "Cross-vendor adversarial review pipeline for AI coding CLIs (Claude, Codex)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a task.yaml
    Run {
        task: String,
        /// Parse + validate + print the plan only; spawn no CLI subprocess
        #[arg(long)]
        dry_run: bool,
        /// Events as NDJSON on stdout, human-readable log on stderr
        #[arg(long)]
        json: bool,
    },
    /// Parse + validate a task.yaml only
    Validate { task: String },
    /// List past runs
    Runs,
    /// Replay the events of a run
    View { run_id: String },
    /// Cost breakdown of a run
    Cost { run_id: String },
    /// Diff two runs
    Diff { run_a: String, run_b: String },
}

/// ~/.agentpipe/runs(AGENTPIPE_HOME 优先)。
pub(crate) fn runs_dir() -> PathBuf {
    let base = std::env::var("AGENTPIPE_HOME")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".agentpipe").join("runs")
}

fn load_manifest(path: &str) -> Manifest {
    let yaml = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("failed to read {path}: {e}");
        std::process::exit(1);
    });
    match Manifest::parse(&yaml).and_then(|m| {
        m.validate()?;
        Ok(m)
    }) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("manifest error: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run { task, dry_run, json } => cmd_run(&task, dry_run, json),
        Cmd::Validate { task } => {
            load_manifest(&task);
            println!("✓ {task} is valid");
        }
        Cmd::Runs => commands::runs(),
        Cmd::View { run_id } => commands::view(&run_id),
        Cmd::Cost { run_id } => commands::cost(&run_id),
        Cmd::Diff { run_a, run_b } => commands::diff(&run_a, &run_b),
    }
}

fn cmd_run(task: &str, dry_run: bool, json: bool) {
    let manifest = load_manifest(task);

    // 人读输出去向:--json 时人读走 stderr(数据走 stdout),否则人读走 stdout。
    macro_rules! human {
        ($($a:tt)*) => {{
            if json { eprintln!($($a)*); } else { println!($($a)*); }
        }};
    }

    if dry_run {
        human!("▶ Plan: {}", manifest.name);
        for step in &manifest.steps {
            human!("{}", render::render_plan_step(step));
        }
        return;
    }

    let bins = RunnerBins {
        claude: std::env::var("AGENTPIPE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        codex: std::env::var("AGENTPIPE_CODEX_BIN").unwrap_or_else(|_| "codex".into()),
    };
    let (etx, erx) = mpsc::channel::<Event>();
    let (ctx, crx) = mpsc::channel::<Command>();
    let name = manifest.name.clone();
    let control = std::sync::Arc::new(agentpipe_engine::control::Control::default());
    let handle = thread::spawn(move || {
        let mut ex = Executor::new(manifest, bins, control, etx, crx);
        ex.run()
    });

    // RunStarted 时开 recorder;失败降级为不落盘(审计是旁路)。
    let mut recorder: Option<RunRecorder> = None;
    let run_dir = runs_dir();

    for event in erx {
        if matches!(event, Event::RunStarted { .. }) {
            recorder = RunRecorder::open(&run_dir, &name)
                .map_err(|e| eprintln!("(audit disabled: {e})"))
                .ok();
            if let Some(r) = &recorder {
                human!("(audit: {})", r.path().display());
            }
        }
        if let Some(r) = &mut recorder {
            r.record(&event);
        }
        if json {
            println!("{}", event_json_line(&event));
        }
        human!("{}", render::render_event(&event));

        match &event {
            Event::StepAwaitingGate { step_id, expects_artifact, gate_kind, .. } => {
                let cmd = prompt_gate(step_id, *expects_artifact, gate_kind);
                let _ = ctx.send(cmd);
            }
            Event::RunFinished { .. } => break,
            _ => {}
        }
    }
    let status = handle.join().ok();
    if !matches!(status, Some(agentpipe_engine::protocol::RunStatus::Success)) {
        std::process::exit(1);
    }
}

mod commands;

fn prompt_gate(step_id: &str, expects_artifact: bool, gate_kind: &GateKind) -> Command {
    let hint = if expects_artifact {
        "[y <artifact> / s skip]"
    } else {
        "[y approve / s skip]"
    };
    eprint!("    > {hint} ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let n = std::io::stdin().lock().read_line(&mut line).unwrap_or(0);
    let input = if n == 0 { None } else { Some(line.as_str()) };
    gate_command(input, step_id, gate_kind)
}

/// 门输入 → 指令的纯函数(prompt_gate 只做 IO)。`input = None` 表示 stdin EOF
/// (管道结束 / Ctrl-D / CI 无人值守)。
///
/// EOF 语义按门的种类分流(codex review P1):
/// - **Decision 门(step 失败 / verify 未达标 / loop 耗尽 max)→ Abort**:这些门存在
///   的意义就是"停下来要人裁决";无人在场时 Skip 会把未解决的失败/未收敛转成
///   RunStatus::Success(exit 0),headless/CI 消费方据 exit code 判断,等于静默放行,
///   违反"gates progress on real exit codes"的核心契约。EOF → Abort → exit 1,
///   人显式敲 `s` 才是 Skip(区分"人主动跳过"与"没有人")。
/// - **Step / Human 门 → Skip**(维持既有语义):step 门是 mode:step 的逐步确认,
///   human 门 headless 的正路是预置 value;两者 EOF 跳过不会把失败伪装成成功。
fn gate_command(input: Option<&str>, step_id: &str, gate_kind: &GateKind) -> Command {
    let Some(raw) = input else {
        return match gate_kind {
            GateKind::Decision => {
                eprintln!("    (stdin closed; aborting at decision gate '{step_id}')");
                Command::Abort
            }
            GateKind::Step | GateKind::Human => {
                eprintln!("    (stdin closed; skipping '{step_id}')");
                Command::SkipStep {
                    step_id: step_id.to_string(),
                }
            }
        };
    };
    let line = raw.trim();
    if line.starts_with('s') {
        Command::SkipStep {
            step_id: step_id.to_string(),
        }
    } else {
        let artifact = line
            .strip_prefix("y ")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Command::ApproveGate {
            step_id: step_id.to_string(),
            artifact,
        }
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn eof_at_decision_gate_aborts() {
        // codex review P1 回归:headless(stdin EOF)下决策门绝不能 Skip 成 Success,
        // 必须 Abort 让 run 以非零 exit code 收尾。
        assert!(matches!(
            gate_command(None, "fixloop", &GateKind::Decision),
            Command::Abort
        ));
    }

    #[test]
    fn eof_at_step_and_human_gates_skips() {
        assert!(matches!(
            gate_command(None, "impl", &GateKind::Step),
            Command::SkipStep { .. }
        ));
        assert!(matches!(
            gate_command(None, "mr", &GateKind::Human),
            Command::SkipStep { .. }
        ));
    }

    #[test]
    fn explicit_skip_and_approve_unchanged() {
        // 人显式敲 `s` 仍是 Skip(与 EOF 的 Abort 区分),y + artifact 正常批准。
        assert!(matches!(
            gate_command(Some("s\n"), "fixloop", &GateKind::Decision),
            Command::SkipStep { .. }
        ));
        match gate_command(Some("y https://mr/1\n"), "mr", &GateKind::Human) {
            Command::ApproveGate { artifact, .. } => assert_eq!(artifact.as_deref(), Some("https://mr/1")),
            other => panic!("expected ApproveGate, got {other:?}"),
        }
    }
}
