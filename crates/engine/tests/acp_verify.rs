//! Acp step + verify 门的 executor 级集成测试(spec D2)。

mod common;

use agentpipe_engine::control::Control;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, GateKind, RunStatus};
use std::sync::{mpsc, Arc};

fn run_manifest(yaml: &str) -> (RunStatus, Vec<Event>) {
    let manifest = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_ctx, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::try_new(
        manifest,
        RunnerBins { claude: "claude-not-used".into(), codex: "codex-not-used".into() },
        Arc::new(Control::default()),
        etx,
        crx,
    )
    .unwrap();
    let status = ex.run();
    (status, erx.try_iter().collect())
}

#[test]
fn acp_step_with_command_verify_passes() {
    let command = format!(
        "env MOCK_ACP_SCENARIO=happy {}",
        common::mock_acp_agent_bin()
    );
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{}\"\n    prompt: hi\n    verify:\n      by: command\n      command: \"true\"\n",
        command
    );
    let (status, events) = run_manifest(&yaml);
    assert!(matches!(status, RunStatus::Success), "events: {events:?}");
    let done = events.iter().any(|e| matches!(e,
        Event::StepFinished { summary, .. } if summary.contains("已校验")));
    assert!(done, "StepFinished 应带 已校验 标记: {events:?}");
}

#[test]
fn acp_step_with_failing_verify_on_unmet_fail() {
    let command = format!(
        "env MOCK_ACP_SCENARIO=happy {}",
        common::mock_acp_agent_bin()
    );
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{}\"\n    prompt: hi\n    verify:\n      by: command\n      command: \"false\"\n      max_retries: 0\n      on_unmet: fail\n",
        command
    );
    let (status, events) = run_manifest(&yaml);
    assert!(matches!(status, RunStatus::Failed), "events: {events:?}");
    assert!(events.iter().any(|e| matches!(e, Event::StepFailed { .. })));
}

#[test]
fn acp_verify_unmet_retry_met_succeeds() {
    let command = format!(
        "env MOCK_ACP_SCENARIO=happy {}",
        common::mock_acp_agent_bin()
    );
    let marker = std::env::temp_dir().join(format!(
        "agentpipe-acp-retry-{}-marker",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let verifier = format!(
        "test -f {} || {{ touch {}; exit 1; }}",
        marker.display(),
        marker.display()
    );
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{}\"\n    prompt: hi\n    verify:\n      by: command\n      command: \"{}\"\n      max_retries: 1\n",
        command,
        verifier
    );

    let (status, events) = run_manifest(&yaml);
    let _ = std::fs::remove_file(&marker);

    assert!(matches!(status, RunStatus::Success), "events: {events:?}");
    assert!(
        events.iter().any(|e| matches!(e,
            Event::StepFinished { summary, .. } if summary.contains("已校验"))),
        "StepFinished 应带 已校验 标记: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e,
            Event::StepProgress { line, .. } if line.contains("第 1 次重试"))),
        "应记录第 1 次重试 progress: {events:?}"
    );
}

#[test]
fn acp_ask_policy_opens_permission_gate_and_approve_grants() {
    let command = format!(
        "env MOCK_ACP_SCENARIO=permission_probe {}",
        common::mock_acp_agent_bin()
    );
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{command}\"\n    prompt: go\n    on_permission: ask\n"
    );
    let manifest = Manifest::parse(&yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel::<Command>();
    // 预置批准指令:权限门弹出时 permission gate 的 recv 立即拿到 Approve。
    ctx_tx
        .send(Command::ApproveGate {
            step_id: "a".into(),
            artifact: None,
        })
        .unwrap();
    let mut ex = Executor::try_new(
        manifest,
        RunnerBins {
            claude: "unused".into(),
            codex: "unused".into(),
        },
        Arc::new(Control::default()),
        etx,
        crx,
    )
    .unwrap();
    let status = ex.run();
    let events: Vec<Event> = erx.try_iter().collect();
    assert!(matches!(status, RunStatus::Success), "{events:?}");
    assert!(
        events.iter().any(|e| matches!(e,
            Event::StepAwaitingGate { gate_kind: GateKind::Permission, suggestion, .. }
                if suggestion.contains("权限请求"))),
        "ask 策略必须弹 Permission 门: {events:?}"
    );
}

#[test]
fn acp_default_reject_policy_never_opens_gate() {
    let command = format!(
        "env MOCK_ACP_SCENARIO=permission_probe {}",
        common::mock_acp_agent_bin()
    );
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{command}\"\n    prompt: go\n"
    );
    let (status, events) = run_manifest(&yaml);
    assert!(matches!(status, RunStatus::Success), "{events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::StepAwaitingGate { .. })),
        "缺省 reject 不得弹任何门: {events:?}"
    );
}
