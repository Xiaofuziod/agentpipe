//! Acp step + verify 门的 executor 级集成测试(spec D2)。

use agentpipe_engine::control::Control;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, GateKind, RunStatus};
use std::path::PathBuf;
use std::process::Command as Proc;
use std::sync::{mpsc, Arc, Once};

static BUILD_MOCK: Once = Once::new();

fn mock_command() -> String {
    BUILD_MOCK.call_once(|| {
        let status = Proc::new("cargo")
            .args(["build", "--quiet", "--example", "mock_acp_agent"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("spawn cargo build");
        assert!(status.success());
    });
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bin = root.parent().unwrap().parent().unwrap().join("target/debug/examples/mock_acp_agent");
    format!("env MOCK_ACP_SCENARIO=happy {}", bin.to_string_lossy())
}

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
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{}\"\n    prompt: hi\n    verify:\n      by: command\n      command: \"true\"\n",
        mock_command()
    );
    let (status, events) = run_manifest(&yaml);
    assert!(matches!(status, RunStatus::Success), "events: {events:?}");
    let done = events.iter().any(|e| matches!(e,
        Event::StepFinished { summary, .. } if summary.contains("已校验")));
    assert!(done, "StepFinished 应带 已校验 标记: {events:?}");
}

#[test]
fn acp_step_with_failing_verify_on_unmet_fail() {
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{}\"\n    prompt: hi\n    verify:\n      by: command\n      command: \"false\"\n      max_retries: 0\n      on_unmet: fail\n",
        mock_command()
    );
    let (status, events) = run_manifest(&yaml);
    assert!(matches!(status, RunStatus::Failed), "events: {events:?}");
    assert!(events.iter().any(|e| matches!(e, Event::StepFailed { .. })));
}

#[test]
fn acp_ask_policy_opens_decision_gate_and_approve_grants() {
    let command =
        mock_command().replace("MOCK_ACP_SCENARIO=happy", "MOCK_ACP_SCENARIO=permission_probe");
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{command}\"\n    prompt: go\n    on_permission: ask\n"
    );
    let manifest = Manifest::parse(&yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel::<Command>();
    // 预置批准指令:权限门弹出时 decision_gate 的 recv 立即拿到 Approve。
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
            Event::StepAwaitingGate { gate_kind: GateKind::Decision, suggestion, .. }
                if suggestion.contains("权限请求"))),
        "ask 策略必须弹 Decision 门: {events:?}"
    );
}

#[test]
fn acp_default_reject_policy_never_opens_gate() {
    let command =
        mock_command().replace("MOCK_ACP_SCENARIO=happy", "MOCK_ACP_SCENARIO=permission_probe");
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
