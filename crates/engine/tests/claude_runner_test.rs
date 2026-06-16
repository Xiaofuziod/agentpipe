use agentpipe_engine::runner::claude::ClaudeRunner;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

#[test]
fn runs_and_captures_last_line_as_artifact() {
    let r = ClaudeRunner::new(fixture("stub-claude.sh"));
    let out = r
        .run("实现功能", None, false, None, None, &mut |_: &str| {}, &PathBuf::from("."))
        .expect("ok");
    assert!(out.full_output.contains("STUB CLAUDE 收到: 实现功能"));
    assert_eq!(out.last_line.trim(), "https://gitlab.example.com/mr/42");
}

#[test]
fn skill_prefixes_prompt() {
    let r = ClaudeRunner::new(fixture("stub-claude.sh"));
    let out = r
        .run("审查", Some("four-dimension-review"), false, None, None, &mut |_: &str| {}, &PathBuf::from("."))
        .unwrap();
    assert!(out.full_output.contains("/four-dimension-review"));
}

#[test]
fn times_out_on_hanging_cli() {
    let r = ClaudeRunner::new(fixture("stub-sleep.sh"));
    let start = std::time::Instant::now();
    let res = r.run("x", None, false, Some(1), None, &mut |_: &str| {}, &PathBuf::from("."));
    // 超时按失败 → run 返回 Err;且不应傻等满 5 秒
    assert!(res.is_err());
    assert!(start.elapsed().as_secs() < 4);
}
