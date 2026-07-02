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

#[test]
fn history_accumulates_previous_findings_only() {
    let mut ctx = RunContext::new(PathBuf::from("."));
    // 第 1 轮:record 前 archive(无既有 findings → 无操作)
    ctx.archive_findings("rev");
    ctx.record("rev", StepOutput { findings: Some("第一轮问题".into()), ..Default::default() });
    assert_eq!(ctx.interpolate("{{rev.history}}"), "", "第 1 轮 history 必须为空");
    // 第 2 轮
    ctx.archive_findings("rev");
    ctx.record("rev", StepOutput { findings: Some("第二轮问题".into()), ..Default::default() });
    let h = ctx.interpolate("{{rev.history}}");
    assert!(h.contains("── 第 1 轮 ──") && h.contains("第一轮问题"), "h = {h}");
    assert!(!h.contains("第二轮问题"), "history 不含当前轮");
    // 第 3 轮:两段历史,带分隔
    ctx.archive_findings("rev");
    ctx.record("rev", StepOutput { findings: Some("第三轮问题".into()), ..Default::default() });
    let h = ctx.interpolate("{{rev.history}}");
    assert!(h.contains("── 第 2 轮 ──") && h.contains("第二轮问题"), "h = {h}");
}

#[test]
fn history_skips_empty_findings_and_unknown_step() {
    let mut ctx = RunContext::new(PathBuf::from("."));
    ctx.record("rev", StepOutput { findings: Some("  ".into()), ..Default::default() });
    ctx.archive_findings("rev"); // 空白 findings 不归档
    assert_eq!(ctx.interpolate("{{rev.history}}"), "");
    assert_eq!(ctx.interpolate("{{nobody.history}}"), "", "未知 step 与其他字段同语义:空串");
}
