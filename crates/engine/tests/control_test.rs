use agentpipe_engine::control::Control;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, RunStatus, StepStatus};
use std::sync::{mpsc, Arc};

fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

#[test]
fn aborted_before_run_yields_aborted_status() {
    let yaml = "version: 1\nname: t\ntarget: .\nmode: auto\nsteps:\n  - id: a\n    kind: human\n    instruction: x\n";
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel();
    let control = Arc::new(Control::default());
    control.request_abort(); // 跑之前就中止
    let bins = RunnerBins { claude: "x".into(), codex: "x".into() };
    let mut ex = Executor::new(m, bins, control, etx, crx);
    let status = ex.run();
    assert_eq!(status, RunStatus::Aborted);
    let evs: Vec<_> = erx.try_iter().collect();
    assert!(evs
        .iter()
        .any(|e| matches!(e, Event::RunFinished { status: RunStatus::Aborted })));
}

#[test]
fn failed_step_can_be_skipped_via_decision_gate() {
    // claude 步骤指向失败 stub → step 失败 → 决策 gate → 预投 SkipStep → 跳过继续 → Success
    let yaml = "version: 1\nname: t\ntarget: .\nmode: auto\nsteps:\n  - id: a\n    kind: claude\n    prompt: x\n";
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx, crx) = mpsc::channel();
    ctx.send(Command::SkipStep { step_id: "a".into() }).unwrap();
    let bins = RunnerBins {
        claude: fixture("stub-fail.sh"),
        codex: "x".into(),
    };
    let mut ex = Executor::new(m, bins, Arc::new(Control::default()), etx, crx);
    let status = ex.run();
    assert_eq!(status, RunStatus::Success);
    let evs: Vec<_> = erx.try_iter().collect();
    assert!(evs.iter().any(|e| matches!(e, Event::StepFailed { .. })));
    assert!(evs
        .iter()
        .any(|e| matches!(e, Event::StepFinished { status: StepStatus::Skipped, .. })));
}
