use agentpipe_engine::context::{RunContext, StepOutput, Verdict};
use std::path::PathBuf;

#[test]
fn interpolates_recorded_artifacts() {
    let mut ctx = RunContext::new(PathBuf::from("/tmp/repo"));
    ctx.record(
        "brainstorm",
        StepOutput {
            artifact: Some("docs/spec.md".into()),
            ..Default::default()
        },
    );
    ctx.record(
        "review-mr",
        StepOutput {
            findings: Some("两处空指针".into()),
            verdict: Some(Verdict::ChangesRequested),
            ..Default::default()
        },
    );

    let out = ctx.interpolate("审查 {{brainstorm.artifact}};修复 {{review-mr.findings}}");
    assert_eq!(out, "审查 docs/spec.md;修复 两处空指针");
}

#[test]
fn unknown_reference_left_empty() {
    let ctx = RunContext::new(PathBuf::from("/tmp"));
    assert_eq!(ctx.interpolate("x={{nope.artifact}}"), "x=");
}
