use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, StepStatus};
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::thread;

fn main() {
    let mut args = std::env::args().skip(1);
    let sub = args.next();
    let path = match (sub.as_deref(), args.next()) {
        (Some("run"), Some(p)) => p,
        _ => {
            eprintln!("用法: agentpipe run <task.yaml>");
            std::process::exit(2);
        }
    };

    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("读取 {path} 失败: {e}");
        std::process::exit(1);
    });
    let manifest = match Manifest::parse(&yaml).and_then(|m| {
        m.validate()?;
        Ok(m)
    }) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("manifest 错误: {e}");
            std::process::exit(1);
        }
    };

    let bins = RunnerBins {
        claude: std::env::var("AGENTPIPE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        codex: std::env::var("AGENTPIPE_CODEX_BIN").unwrap_or_else(|_| "codex".into()),
    };

    let (etx, erx) = mpsc::channel::<Event>();
    let (ctx, crx) = mpsc::channel::<Command>();

    let control = std::sync::Arc::new(agentpipe_engine::control::Control::default());
    let handle = thread::spawn(move || {
        let mut ex = Executor::new(manifest, bins, control, etx, crx);
        ex.run()
    });

    for event in erx {
        match event {
            Event::RunStarted { name } => println!("▶ Run: {name}"),
            Event::StepStarted { step_id, kind } => println!("  ▷ [{kind}] {step_id}"),
            Event::StepProgress { line, .. } => println!("    {line}"),
            Event::StepFinished {
                step_id,
                status,
                summary,
            } => {
                let mark = if matches!(status, StepStatus::Skipped) {
                    "⏭"
                } else {
                    "✓"
                };
                println!("  {mark} {step_id}: {summary}");
            }
            Event::StepFailed { step_id, error } => println!("  ✗ {step_id}: {error}"),
            Event::LoopIteration {
                loop_id,
                iteration,
            } => println!("  ↻ {loop_id} 第 {iteration} 轮"),
            Event::LoopConverged {
                loop_id,
                iterations,
            } => println!("  ✓ {loop_id} {iterations} 轮收敛"),
            Event::LoopMaxReached { loop_id, max } => {
                println!("  ⚠ {loop_id} 到上限 {max} 仍未干净")
            }
            Event::StepAwaitingGate {
                step_id,
                suggestion,
                expects_artifact,
                ..
            } => {
                println!("  ⏸ {step_id}: {suggestion}");
                let cmd = prompt_gate(&step_id, expects_artifact);
                let _ = ctx.send(cmd);
            }
            Event::RunFinished { status } => {
                println!("■ 结束: {status:?}");
                break;
            }
        }
    }

    let _ = handle.join();
}

fn prompt_gate(step_id: &str, expects_artifact: bool) -> Command {
    let hint = if expects_artifact {
        "[y <产物> / s 跳过]"
    } else {
        "[y 批准 / s 跳过]"
    };
    print!("    > {hint} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let n = std::io::stdin().lock().read_line(&mut line).unwrap_or(0);
    // read_line 返回 0 = EOF(stdin 关闭 / 管道结束 / Ctrl-D):无人在回路,
    // fail-closed 跳过该步,绝不静默自动批准(claude 步骤一律 bypassPermissions,更不能放过)。
    if n == 0 {
        eprintln!("    (stdin 已结束,跳过 '{step_id}')");
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
