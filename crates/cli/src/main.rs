mod render;

use agentpipe_engine::audit::{event_json_line, RunRecorder};
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event};
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
            Event::StepAwaitingGate { step_id, expects_artifact, .. } => {
                let cmd = prompt_gate(step_id, *expects_artifact);
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

fn prompt_gate(step_id: &str, expects_artifact: bool) -> Command {
    let hint = if expects_artifact {
        "[y <artifact> / s skip]"
    } else {
        "[y approve / s skip]"
    };
    eprint!("    > {hint} ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let n = std::io::stdin().lock().read_line(&mut line).unwrap_or(0);
    // read_line 返回 0 = EOF(stdin 关闭 / 管道结束 / Ctrl-D):无人在回路,
    // fail-closed 跳过该步,绝不静默自动批准(claude 步骤一律 bypassPermissions,更不能放过)。
    if n == 0 {
        eprintln!("    (stdin closed; skipping '{step_id}')");
        return Command::SkipStep {
            step_id: step_id.to_string(),
        };
    }
    let line = line.trim();
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
