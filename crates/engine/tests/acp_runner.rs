//! ACP runner integration tests。
//!
//! 用 `examples/mock_acp_agent.rs` 起 fixture binary,通过 stdio 跑真实 ACP wire 协议。
//! 覆盖场景见 docs/specs/2026-06-25-acp-integration-design.md §8.2。

use agentpipe_engine::control::Control;
use agentpipe_engine::runner::acp::{AcpConfig, AcpOutcome, AcpRunner};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

static BUILD_MOCK: Once = Once::new();

fn ensure_mock_built() {
    BUILD_MOCK.call_once(|| {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let status = Command::new("cargo")
            .args(["build", "--quiet", "--example", "mock_acp_agent"])
            .current_dir(manifest_dir)
            .status()
            .expect("spawn cargo build");
        assert!(
            status.success(),
            "cargo build --example mock_acp_agent 失败"
        );
    });
}

/// 计算 mock binary 路径。target 默认 workspace 根的 `target/debug/examples/`。
fn mock_command() -> String {
    ensure_mock_built();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/engine -> agentpipe (workspace root)
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let bin = workspace_root.join("target/debug/examples/mock_acp_agent");
    assert!(
        bin.exists(),
        "mock_acp_agent binary 不存在: {}",
        bin.display()
    );
    bin.to_string_lossy().into_owned()
}

/// 跑一个 scenario,返回 (outcome 结果, 收到的 progress 行)。
fn run_scenario_full(
    scenario: &str,
    prompt: &str,
    timeout_secs: u64,
    control: Option<&Control>,
) -> (Result<AcpOutcome, String>, Vec<String>) {
    let command = mock_command();
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
        )
        .map_err(|e| format!("{e:?}"));
    (res, progress)
}

fn run_scenario(scenario: &str, prompt: &str) -> Result<AcpOutcome, String> {
    run_scenario_full(scenario, prompt, 30, None).0
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
fn timeout_mid_stream_returns_promptly() {
    // long_stream 每秒发 1 chunk × 30 次,timeout 设 1s → 应在 ~1-2s 内 timeout 错误返回。
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
