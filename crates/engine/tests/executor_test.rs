use agentpipe_engine::control::Control;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, RunStatus, StepStatus};
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
    // loop 自身不发 StepStarted(否则宿主把它渲染成永久 "运行中" 的幽灵步骤);
    // 只有 body 子步骤(rev)发。
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::StepStarted { step_id, .. } if step_id == "fixloop")),
        "loop 步不应发 StepStarted"
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::StepStarted { step_id, kind } if step_id == "rev" && kind == "codex")));
}

/// 跑一个带 verify 的单 claude step,返回收到的事件流。
fn run_verify_step(verdict: &str, verify_yaml: &str) -> Vec<Event> {
    std::env::set_var("STUB_VERDICT", verdict);
    let yaml = format!(
        r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: impl
    kind: claude
    prompt: 实现功能
    verify:
{verify_yaml}
"#
    );
    let m = Manifest::parse(&yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    erx.try_iter().collect()
}

fn count_progress(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::StepProgress { line, .. } if line.contains(needle)))
        .count()
}

#[test]
fn verify_clean_passes_without_retry() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let events = run_verify_step("clean", "      by: codex\n      action: review-mr\n      base: dev");
    assert_eq!(count_progress(&events, "校验未通过"), 0);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepFinished { summary, .. } if summary.contains("已校验")
    )));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::RunFinished { status: RunStatus::Success })));
}

#[test]
fn verify_unmet_retries_to_max_then_fails() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let events = run_verify_step(
        "changes_requested",
        "      by: codex\n      action: review-mr\n      base: dev\n      max_retries: 2\n      on_unmet: fail",
    );
    // 重试 2 次 → 2 条"校验未通过"
    assert_eq!(count_progress(&events, "校验未通过"), 2);
    assert!(events.iter().any(|e| matches!(e, Event::StepFailed { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::RunFinished { status: RunStatus::Failed })));
}

#[test]
fn verify_unmet_continue_proceeds() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let events = run_verify_step(
        "changes_requested",
        "      by: codex\n      action: review-mr\n      base: dev\n      max_retries: 1\n      on_unmet: continue",
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepFinished { summary, .. } if summary.contains("未达标")
    )));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::RunFinished { status: RunStatus::Success })));
}

#[test]
fn verify_unmet_gate_then_skip() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "changes_requested");
    // max_retries:0 → 首次未达成直接走 gate;预置 SkipStep 指令
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: impl
    kind: claude
    prompt: 实现功能
    verify:
      by: codex
      action: review-mr
      base: dev
      max_retries: 0
      on_unmet: gate
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel();
    ctx_tx.send(Command::SkipStep { step_id: "impl".into() }).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepAwaitingGate { gate_kind, .. } if matches!(gate_kind, agentpipe_engine::protocol::GateKind::Decision)
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepFinished { status: StepStatus::Skipped, .. }
    )));
}

#[test]
fn verify_by_claude_pass_proceeds() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // claude verifier 回复 VERDICT: pass → 达成(stub 把换行压成空格,故 result 直接是判定行;
    // 真 claude 的多行末行场景由 executor::tests::parse_verdict_* 单测覆盖)
    std::env::set_var("STUB_CLAUDE_RESULT", "VERDICT: pass");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: impl
    kind: claude
    prompt: 实现功能
    verify:
      by: claude
      prompt: 判定目标是否达成
      on_unmet: fail
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    std::env::remove_var("STUB_CLAUDE_RESULT");
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepFinished { summary, .. } if summary.contains("已校验")
    )));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::RunFinished { status: RunStatus::Success })));
}

#[test]
fn verify_by_claude_fail_then_fails() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_CLAUDE_RESULT", "VERDICT: fail");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: impl
    kind: claude
    prompt: 实现功能
    verify:
      by: claude
      prompt: 判定目标是否达成
      max_retries: 1
      on_unmet: fail
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    std::env::remove_var("STUB_CLAUDE_RESULT");
    assert_eq!(count_progress(&events, "校验未通过"), 1); // max_retries=1
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::RunFinished { status: RunStatus::Failed })));
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

#[test]
fn human_with_preset_value_skips_gate_and_seeds_artifact() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // human 步骤预置 value → 不发 gate、不阻塞;下游步骤可用 {{mr.artifact}} 拿到该值。
    // 无指令送入 commands(crx 空),若仍 gate 会卡死 recv;能跑完即证明跳过了 gate。
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: mr
    kind: human
    instruction: "粘贴链接"
    expects: "链接"
    value: "https://example.com/mr/1"
  - id: echo
    kind: human
    instruction: "记录 {{mr.artifact}}"
    value: "seen {{mr.artifact}}"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_ctx_tx, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    let status = ex.run();

    assert_eq!(status, RunStatus::Success);
    let events: Vec<_> = erx.try_iter().collect();
    // 预置值的 human 步骤不应发出任何 gate
    assert!(
        !events.iter().any(|e| matches!(e, Event::StepAwaitingGate { .. })),
        "预置值的 human 步骤不应发 gate"
    );
    // 两个 human 步骤都 Done
    let done = events
        .iter()
        .filter(|e| matches!(e, Event::StepFinished { status: StepStatus::Done, .. }))
        .count();
    assert_eq!(done, 2);
}

#[test]
fn human_with_blank_preset_falls_back_to_gate() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // value 插值后为空(引用了不存在的 step)→ 回退到正常 gate,而非静默跳过。
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: mr
    kind: human
    instruction: "粘贴链接"
    expects: "链接"
    value: "{{missing.artifact}}"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel();
    // 预放批准,gate 到达即被消费(否则 recv 卡死)
    ctx_tx
        .send(Command::ApproveGate {
            step_id: "mr".into(),
            artifact: Some("manual".into()),
        })
        .unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    assert!(
        events.iter().any(|e| matches!(e, Event::StepAwaitingGate { step_id, .. } if step_id == "mr")),
        "空预置值应回退到人工 gate"
    );
}
