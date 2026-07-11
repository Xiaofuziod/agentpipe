//! ACP runner integration tests。
//!
//! 用 `examples/mock_acp_agent.rs` 起 fixture binary,通过 stdio 跑真实 ACP wire 协议。
//! 覆盖场景见 docs/specs/2026-06-25-acp-integration-design.md §8.2。

mod common;

use agentpipe_engine::control::Control;
use agentpipe_engine::runner::acp::{AcpConfig, AcpOutcome, AcpRunner};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 跑一个 scenario,返回 (outcome 结果, 收到的 progress 行)。
fn run_scenario_full(
    scenario: &str,
    prompt: &str,
    timeout_secs: u64,
    control: Option<&Control>,
) -> (Result<AcpOutcome, String>, Vec<String>) {
    let command = common::mock_acp_agent_bin();
    let full_cmd = format!("env MOCK_ACP_SCENARIO={scenario} {command}");
    let runner = AcpRunner::with_timeout(
        AcpConfig {
            agent: format!("mock-{scenario}"),
            command: full_cmd,
        },
        timeout_secs,
    );
    let mut progress = Vec::<String>::new();
    let cwd = std::env::current_dir().unwrap();
    let res = runner
        .run(
            prompt,
            control,
            &mut |line, _round| progress.push(line.to_string()),
            &cwd,
            agentpipe_engine::runner::acp::PermissionMode::Reject,
        )
        .map_err(|e| format!("{e:?}"));
    (res, progress)
}

fn run_scenario(scenario: &str, prompt: &str) -> Result<AcpOutcome, String> {
    run_scenario_full(scenario, prompt, 30, None).0
}

fn run_scenario_perm(
    scenario: &str,
    cb: &mut dyn FnMut(&str) -> agentpipe_engine::runner::acp::PermissionDecision,
) -> Result<AcpOutcome, String> {
    let command = common::mock_acp_agent_bin();
    let full_cmd = format!("env MOCK_ACP_SCENARIO={scenario} {command}");
    let runner = AcpRunner::with_timeout(
        AcpConfig {
            agent: format!("mock-{scenario}"),
            command: full_cmd,
        },
        30,
    );
    let cwd = std::env::current_dir().unwrap();
    runner
        .run(
            "go",
            None,
            &mut |_l, _r| {},
            &cwd,
            agentpipe_engine::runner::acp::PermissionMode::Ask(cb),
        )
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn happy_path_aggregates_chunks_and_streams_progress() {
    let (res, progress) = run_scenario_full("happy", "你好", 30, None);
    let outcome = res.expect("happy 场景应该成功");
    assert_eq!(
        outcome.answer, "你好,我是 mock。",
        "聚合 chunk 应拼成完整文本"
    );
    assert!(outcome.metrics.is_none(), "MVP 不上报 metrics");
    // 流式 progress:3 个 chunk 应触发 ≥ 3 行 [msg] progress(spec §6 「流式 → progress_sink」)。
    let msg_lines = progress.iter().filter(|l| l.starts_with("[msg]")).count();
    assert!(
        msg_lines >= 3,
        "应收到 ≥ 3 行流式 progress,实际 {} 行: {progress:?}",
        msg_lines
    );
}

#[test]
fn empty_response_fails_loud() {
    let err = run_scenario("empty", "ping").expect_err("empty 场景必须 fail-loud");
    assert!(
        err.contains("未返回任何文本"),
        "错误信息应明示空 answer: {err}"
    );
}

#[test]
fn wrong_protocol_version_fails_loud() {
    // mock 返回 V0,client 侧 protocolVersion 检查应主动 fail-loud(spec §7.3)。
    let err = run_scenario("wrong_version", "hi")
        .expect_err("wrong_version 场景必须 fail-loud,不能静默继续");
    assert!(
        err.contains("协议版本不匹配") || err.contains("V1"),
        "错误信息应提示版本不匹配: {err}"
    );
}

#[test]
fn abort_mid_stream_returns_promptly() {
    // long_stream agent 每秒发 1 chunk × 30 次 → 总跑 30s。
    // 主线程在 1.5s 时按 abort,期望 acp.run 在 ≤ 3s 总耗时内带 abort 错误返回。
    // warm-up:cold start 时 mock fixture 还没构建,首测会把 cargo build 算进 timer
    // 造成 flaky;先 mock_acp_agent_bin() 触发构建,确保 mock binary 就绪
    // 后再开 timer(无副作用,Once 后续 call_once 跳过)。
    let _ = common::mock_acp_agent_bin();
    let control = Arc::new(Control::default());
    let control_for_aborter = control.clone();
    let aborter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        control_for_aborter.request_abort();
    });

    let started = Instant::now();
    let (res, _progress) = run_scenario_full("long_stream", "go", 30, Some(&control));
    let elapsed = started.elapsed();
    aborter.join().unwrap();

    let err = res.expect_err("abort 后必须返回错误");
    assert!(err.contains("中止"), "错误信息应说明被中止: {err}");
    assert!(
        elapsed < Duration::from_secs(3),
        "abort 响应应 ≤ 3s,实际 {elapsed:?}"
    );
}

#[test]
fn abort_during_chatty_stream_returns_promptly() {
    // codex review P2 回归:fast_stream 每 10ms 一 chunk,比主线程 100ms 轮询间隔
    // 密得多,recv_timeout 永远命中 Ok 分支 —— 修复前 abort 只在 Timeout 分支检查,
    // 高频输出会把中止压到 agent 跑完(~30s)才响应;修复后循环头每轮检查,≤3s 返回。
    let _ = common::mock_acp_agent_bin();
    let control = Arc::new(Control::default());
    let control_for_aborter = control.clone();
    let aborter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        control_for_aborter.request_abort();
    });

    let started = Instant::now();
    let (res, _progress) = run_scenario_full("fast_stream", "go", 60, Some(&control));
    let elapsed = started.elapsed();
    aborter.join().unwrap();

    let err = res.expect_err("abort 后必须返回错误");
    assert!(err.contains("中止"), "错误信息应说明被中止: {err}");
    assert!(
        elapsed < Duration::from_secs(3),
        "高频输出下 abort 响应仍应 ≤ 3s,实际 {elapsed:?}"
    );
}

#[test]
fn fs_reverse_request_is_rejected_without_hang() {
    // mock 在 prompt 内主动发反向 fs/read_text_file;client MVP 不声明 fs capability,
    // SDK 应当返回 method_not_found 类错误。mock 忽略错误后继续发 "ok" chunk 并 EndTurn,
    // client 必须能正常完成并拼出 "ok" answer,不卡死(spec §3 / §7.4)。
    let (res, _progress) = run_scenario_full("fs_probe", "probe", 30, None);
    let outcome = res.expect("fs_probe 场景应当成功(反向请求被拒不影响主流程)");
    assert_eq!(outcome.answer, "ok", "反向请求被拒后 agent 仍能完成 prompt");
}

#[test]
fn permission_reject_mode_yields_denied() {
    // 缺省 Reject 模式:与旧行为一致,agent 收 Cancelled。
    let out = run_scenario("permission_probe", "go").expect("应正常完成");
    assert_eq!(out.answer, "denied");
}

#[test]
fn permission_ask_approve_yields_granted() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    let mut asked = Vec::new();
    let out = run_scenario_perm("permission_probe", &mut |desc| {
        asked.push(desc.to_string());
        PermissionDecision::Approve
    })
    .expect("应正常完成");
    assert_eq!(out.answer, "granted");
    assert_eq!(asked.len(), 1, "回调应被调用一次");
}

#[test]
fn abort_during_permission_grant_never_reports_ok() {
    use agentpipe_engine::runner::acp::{PermissionDecision, PermissionMode};

    let control = Arc::new(Control::default());
    let command = common::mock_acp_agent_bin();
    let full_cmd = format!("env MOCK_ACP_SCENARIO=permission_probe {command}");
    let runner = AcpRunner::with_timeout(
        AcpConfig {
            agent: "mock-perm-abort".into(),
            command: full_cmd,
        },
        30,
    );
    let cwd = std::env::current_dir().unwrap();
    let control_in_cb = control.clone();
    let mut cb = |_desc: &str| {
        control_in_cb.request_abort();
        PermissionDecision::Approve
    };

    let err = runner
        .run(
            "go",
            Some(&control),
            &mut |_l, _r| {},
            &cwd,
            PermissionMode::Ask(&mut cb),
        )
        .expect_err("control 已中止的 run 不得返回 Ok");

    assert!(format!("{err:?}").contains("中止"), "{err:?}");
}

#[test]
fn permission_ask_reject_yields_denied() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    let out = run_scenario_perm("permission_probe", &mut |_| PermissionDecision::RejectOnce)
        .expect("应正常完成");
    assert_eq!(out.answer, "denied");
}

#[test]
fn permission_ask_approve_without_allow_option_falls_back_cancelled() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    // spec §6 第五条路径:批准但 agent 未提供 allow 选项 → 回 Cancelled,agent 视角=denied。
    let out = run_scenario_perm("permission_probe_noallow", &mut |_| {
        PermissionDecision::Approve
    })
    .expect("应正常完成(回退 Cancelled 不是错误)");
    assert_eq!(out.answer, "denied");
    assert!(
        out.full_transcript.contains("未提供 allow 选项"),
        "transcript 必须记录回退原因: {}",
        out.full_transcript
    );
}

#[test]
fn permission_ask_abort_aborts_run() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    // slow 变体:agent 回 chunk 后拖 10s 才 EndTurn → abort 分支确定性先赢。
    let err = run_scenario_perm("permission_probe_slow", &mut |_| PermissionDecision::Abort)
        .expect_err("Abort 决策必须中止 run");
    assert!(err.contains("中止"), "{err}");
}

#[test]
fn permission_ask_abort_wins_even_if_agent_finishes_quickly() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    // 快速变体:agent 收 Cancelled 后马上回 chunk + EndTurn。Abort 决策必须仍然强制中止,
    // 不能竞态成一次普通拒绝后的成功完成。
    let err = run_scenario_perm("permission_probe", &mut |_| PermissionDecision::Abort)
        .expect_err("Abort 决策必须中止 run");
    assert!(err.contains("中止"), "{err}");
}

#[test]
fn timeout_mid_stream_returns_promptly() {
    // long_stream 每秒发 1 chunk × 30 次,timeout 设 1s → 应在 ~1-2s 内 timeout 错误返回。
    // 同 abort_mid_stream warm-up:防 cold start 把 cargo build 算进 timer。
    let _ = common::mock_acp_agent_bin();
    let started = Instant::now();
    let (res, _progress) = run_scenario_full("long_stream", "go", 1, None);
    let elapsed = started.elapsed();

    let err = res.expect_err("超时必须返回错误");
    assert!(err.contains("超时"), "错误信息应说明超时: {err}");
    assert!(
        elapsed < Duration::from_secs(3),
        "timeout 响应应 ≤ 3s,实际 {elapsed:?}"
    );
}
