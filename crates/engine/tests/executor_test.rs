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

/// RAII 守护:测试退出(正常或 panic)时自动 remove_var,防止跨测试污染。
/// review-2 §C finding #10:之前 std::env::remove_var(...) 在 ex.run() 之后,
/// panic 跳过清理 → 同进程后续抢到 ENV_LOCK 的测试看到泄漏的 env var。
struct EnvGuard(&'static str);

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        std::env::set_var(key, value);
        Self(key)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var(self.0);
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
    base: HEAD
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
    base: HEAD
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
        base: HEAD
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
        base: HEAD
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

#[test]
fn loop_base_ref_missing_fails_loud_without_spinning_to_max() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "changes_requested");
    // 活锁的对偶面回归:base ref 不存在时,review-mr 必须 fail-loud 走失败决策门,
    // 而不是把 changes_requested 喂回 loop 空转到 max(已观测:9 轮烧 $16)。
    // 预置 Abort 消费第一次决策门 → 断言只跑了 1 轮 review、未到 max。
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: fixloop
    kind: loop
    until: codex-clean
    max: 5
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: agentpipe-nonexistent-base-ref
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel();
    ctx_tx.send(Command::Abort).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    // fail-loud:发了 StepFailed
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StepFailed { step_id, .. } if step_id == "rev")),
        "base 缺失应发 StepFailed(fail-loud)"
    );
    // 不空转:review 只 Started 1 次,绝不跑满 max
    let rev_starts = events
        .iter()
        .filter(|e| matches!(e, Event::StepStarted { step_id, .. } if step_id == "rev"))
        .count();
    assert_eq!(rev_starts, 1, "base 缺失应首轮即 fail-loud,不空转");
    // 注:review-2 §E finding #11 后,sub-step Err 透传时 run_loop 复用 LoopMaxReached
    // 作"loop 因外因停止"信号(避免新 Event 变体跨端代价)。max 字段反映触发时的实际
    // iteration 数,与 manifest max=5 不同,可借此区分"跑满"vs"中段停"。
    let loop_max_reached: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::LoopMaxReached { max, .. } => Some(*max),
            _ => None,
        })
        .collect();
    assert_eq!(loop_max_reached, vec![1], "应在第 1 次 iteration 中段 emit LoopMaxReached{{max=1}},而非跑满 max=5");
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
    let events = run_verify_step("clean", "      by: codex\n      action: review-mr\n      base: HEAD");
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
        "      by: codex\n      action: review-mr\n      base: HEAD\n      max_retries: 2\n      on_unmet: fail",
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
        "      by: codex\n      action: review-mr\n      base: HEAD\n      max_retries: 1\n      on_unmet: continue",
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
      base: HEAD
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
    base: HEAD
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

#[test]
fn budget_exceeded_aborts_run_at_first_overrun() {
    // stub-claude 每步 cost_usd = 0.01。budget = 0.005 < 0.01 → 第 1 个 claude step 完成后
    // charge_and_check 触发 over budget,立刻 emit 单条 StepFailed("超出 USD budget...")
    // + return Err → run() 主循环走 RunStatus::Aborted,第 2 步不启动。
    //
    // **不再双 emit**:新 spec(2026-06-26 review-findings-fix §C)语义是 charge 触发时只发
    // StepFailed,不发 StepFinished —— 避免同一 step_id 既 Done 又 Failed 的矛盾终态。
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "clean");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
budget_usd: 0.005
steps:
  - id: first
    kind: claude
    prompt: "step1"
  - id: second
    kind: claude
    prompt: "step2"
"#;
    let m = Manifest::parse(yaml).unwrap();
    assert!(m.validate().is_ok());
    let (etx, erx) = mpsc::channel();
    let (_ctx, crx) = mpsc::channel::<Command>();

    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    let status = ex.run();

    assert_eq!(status, RunStatus::Aborted, "超 budget 应走 Aborted,不是 Failed");
    let events: Vec<Event> = erx.try_iter().collect();

    // 第 2 步必须没启动(charge 在第 1 步 spawn 后立即触发停)
    let started_second = events
        .iter()
        .any(|e| matches!(e, Event::StepStarted { step_id, .. } if step_id == "second"));
    assert!(!started_second, "第 2 步不该启动,实际事件: {events:?}");

    // budget 错误必须有解释性 StepFailed,且 step_id = "first"
    let budget_err = events.iter().any(|e| {
        matches!(e, Event::StepFailed { step_id, error, .. } if step_id == "first" && error.contains("超出 USD budget"))
    });
    assert!(budget_err, "应 emit 含 '超出 USD budget' 的 StepFailed 事件: {events:?}");

    // 关键:第 1 步**不应**有 StepFinished{Done},charge 触发就停 —— 避免双 emit 矛盾终态。
    let finished_first = events.iter().any(
        |e| matches!(e, Event::StepFinished { step_id, status, .. } if step_id == "first" && *status == StepStatus::Done),
    );
    assert!(!finished_first, "charge 触发时不应 emit StepFinished(避免同 step_id 既 Done 又 Failed)");
}

#[test]
fn step_finished_metrics_include_verifier_cost_not_just_last_attempt() {
    // review-2 §B 主修目标(finding #2 + #5 + #7):StepFinished.metrics 必须反映该 step
    // 内**所有** spawn 的累加成本(attempt + verifier),而非只末次 attempt 一份;否则
    // audit::aggregate_cost 和 GUI total cost 系统性低估。
    //
    // stub-claude 每次返 cost=0.01;Claude step + Claude verify(verdict=pass 一次过)→
    // 应当 cumulate 1 attempt + 1 verifier = 0.02。改造前只看到 0.01。
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _v = EnvGuard::set("STUB_VERDICT", "clean");
    let _r = EnvGuard::set("STUB_CLAUDE_RESULT", "VERDICT: pass");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: do
    kind: claude
    prompt: "干活"
    verify:
      by: claude
      prompt: "判定"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_ctx, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    let status = ex.run();

    assert_eq!(status, RunStatus::Success);
    let events: Vec<Event> = erx.try_iter().collect();
    let metrics = events.iter().find_map(|e| match e {
        Event::StepFinished { step_id, metrics: Some(m), .. } if step_id == "do" => Some(m.clone()),
        _ => None,
    });
    let m = metrics.expect("StepFinished 应带 metrics");
    // 1 attempt + 1 verifier × 0.01 = 0.02。改造前只看到 0.01。
    assert!(
        (m.cost_usd - 0.02).abs() < 1e-9,
        "StepFinished metrics 应累积 attempt+verifier cost,期望 0.02,实际 {}",
        m.cost_usd
    );
    // num_turns 同样累加(stub 每次返 1)
    assert_eq!(m.num_turns, 2, "num_turns 应累加 attempt+verifier");
}

#[test]
fn budget_charges_each_verify_retry_attempt_and_verifier() {
    // spec §3.1 主修复目标:verify-retry 中段每次 attempt + verifier cost 都入账,
    // 不再只看末次 attempt 的 metrics(以前 finish 路径漏统计 N-1 次 attempt + verifier 全部)。
    //
    // stub-claude 每次返 cost=0.01,STUB_CLAUDE_RESULT="VERDICT: fail" 让 claude verifier 始终判
    // 未通过 → 触发 retry。verify max_retries=2 / on_unmet=fail,即一个 step 最多跑 3 次干活 +
    // 3 次 verifier = 6 次 claude.run。budget=0.045 → 第 5 次 charge 累计 0.05 > 0.045 触发 abort。
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _v = EnvGuard::set("STUB_VERDICT", "clean");
    let _r = EnvGuard::set("STUB_CLAUDE_RESULT", "VERDICT: fail");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
budget_usd: 0.045
steps:
  - id: do
    kind: claude
    prompt: "干活"
    verify:
      by: claude
      prompt: "判定"
      max_retries: 2
      on_unmet: fail
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_ctx, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    let status = ex.run();

    assert_eq!(status, RunStatus::Aborted, "verify-retry 中段触发 budget 应 Aborted");
    let events: Vec<Event> = erx.try_iter().collect();
    let budget_err = events.iter().any(|e| {
        matches!(e, Event::StepFailed { step_id, error, .. } if step_id == "do" && error.contains("超出 USD budget"))
    });
    assert!(
        budget_err,
        "verify-retry 中段必须能触发 budget StepFailed(否则 verifier/retry cost 全漏): {events:?}"
    );
    // review §A finding #4 守护:budget 触发的 StepFailed 必须携带 cumulative metrics,
    // 让 audit::aggregate_cost 能正确合计失败 step 的真实花费(旧版 StepFailed 无 metrics
    // 字段,audit 总成本永远 $0,与 ctx.cost_so_far_usd 真实账目脱节)。
    let metrics_in_failed = events.iter().find_map(|e| match e {
        Event::StepFailed { step_id, metrics: Some(m), error, .. }
            if step_id == "do" && error.contains("超出 USD budget") =>
        {
            Some(m.clone())
        }
        _ => None,
    });
    let m = metrics_in_failed
        .expect("budget StepFailed 必须携带 cumulative metrics(review §A finding #4)");
    // 触发时已经至少 charge 了 ≥ budget(0.045)的累积成本,且每次单位 0.01。
    assert!(
        m.cost_usd >= 0.045,
        "StepFailed.metrics 应反映 cumulative 而非单次,期望 ≥ 0.045,实际 {}",
        m.cost_usd
    );
}

#[test]
fn loop_max_reached_carries_distinct_reason_for_each_termination_path() {
    // review §A finding #15 守护:LoopMaxReached.reason 区分自然 max / 外部 abort /
    // sub-step 失败三条路径,UI/CLI 渲染按 reason 出不同文案。本 test 覆盖 sub-step
    // 失败路径(SubStepFailed)— 借用 base_ref_missing 已有的活锁防御场景:base 不
    // 存在 → review fail-loud → decision gate Abort → sub-step Err 透传 → loop
    // emit LoopMaxReached{reason: SubStepFailed, max: 1}。
    use agentpipe_engine::protocol::LoopEndReason;
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _v = EnvGuard::set("STUB_VERDICT", "changes_requested");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: fixloop
    kind: loop
    until: codex-clean
    max: 5
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: agentpipe-nonexistent-base-ref
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel();
    ctx_tx.send(Command::Abort).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    ex.run();
    let events: Vec<Event> = erx.try_iter().collect();
    // 必有一条 LoopMaxReached{reason: SubStepFailed}
    let sub_failed = events.iter().any(|e| {
        matches!(
            e,
            Event::LoopMaxReached { loop_id, reason: LoopEndReason::SubStepFailed, .. }
                if loop_id == "fixloop"
        )
    });
    assert!(
        sub_failed,
        "sub-step Err 透传应 emit LoopMaxReached.reason=SubStepFailed: {events:?}"
    );
}

#[test]
fn step_gate_abort_classifies_run_as_aborted_not_failed() {
    // review §A finding #13 守护:用户在 step 门控 / 决策门 选 Abort 时,executor 必须
    // 翻 control.request_abort,让 run() 顶层分类落 RunStatus::Aborted(用户主动中止),
    // 而非误分类为 Failed(引擎失败)。本 test 通过 step 模式发 Abort 验证。
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _v = EnvGuard::set("STUB_VERDICT", "clean");
    let yaml = r#"
version: 1
name: t
target: .
mode: step
steps:
  - id: only
    kind: claude
    prompt: "hi"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel();
    // step 门控 Abort:while waiting at first gate,发 Abort
    ctx_tx.send(Command::Abort).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    let status = ex.run();
    assert_eq!(
        status,
        RunStatus::Aborted,
        "用户经决策门 Abort 必须分类为 RunStatus::Aborted,实际 {status:?}"
    );
    let events: Vec<Event> = erx.try_iter().collect();
    let saw_finished_aborted = events.iter().any(|e| {
        matches!(e, Event::RunFinished { status } if matches!(status, RunStatus::Aborted))
    });
    assert!(saw_finished_aborted, "RunFinished 必须报 Aborted: {events:?}");
}
