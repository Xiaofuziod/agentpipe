use agentpipe_engine::control::Control;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, RunStatus};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

fn test_control() -> Arc<Control> {
    Arc::new(Control::default())
}

fn stub_bins() -> RunnerBins {
    RunnerBins {
        claude: fixture("stub-claude.sh"),
        codex: fixture("stub-codex.sh"),
    }
}

#[test]
fn runs_simple_codex_then_claude_in_auto_mode() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "clean");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: rev
    kind: codex
    action: review-mr
    base: dev
  - id: fix
    kind: claude
    prompt: "用 {{rev.findings}}"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_ctx, crx) = mpsc::channel::<Command>();

    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    let status = ex.run();

    assert_eq!(status, RunStatus::Success);
    let events: Vec<Event> = erx.try_iter().collect();
    let started = events
        .iter()
        .filter(|e| matches!(e, Event::StepStarted { .. }))
        .count();
    assert_eq!(started, 2);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::RunFinished { status: RunStatus::Success })));
}

#[test]
fn emits_step_progress_from_cli_stdout() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "clean");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: rev
    kind: codex
    action: review-mr
    base: dev
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<Event> = erx.try_iter().collect();
    // stub-codex.sh 末尾 echo "stub codex done" → 应转成 StepProgress
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepProgress { line, .. } if line.contains("stub codex done")
    )));
}

#[test]
fn loop_converges_when_codex_clean() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "clean");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: fixloop
    kind: loop
    until: codex-clean
    max: 3
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: dev
      - id: fix
        kind: claude
        prompt: "修 {{rev.findings}}"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::LoopConverged { iterations: 1, .. })));
}

#[test]
fn loop_hits_max_when_never_clean() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "changes_requested");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: fixloop
    kind: loop
    until: codex-clean
    max: 2
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: dev
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::LoopMaxReached { max: 2, .. })));
}

#[test]
fn step_mode_waits_for_approval_each_step() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "clean");
    let yaml = r#"
version: 1
name: t
target: .
mode: step
steps:
  - id: rev
    kind: codex
    action: review-mr
    base: dev
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel();
    // 预先放一个批准指令
    ctx_tx
        .send(Command::ApproveGate {
            step_id: "rev".into(),
            artifact: None,
        })
        .unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::StepAwaitingGate { step_id, .. } if step_id == "rev")));
}
