use agentpipe_engine::runner::claude::ClaudeRunner;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

#[test]
fn runs_and_captures_last_line_as_artifact() {
    let r = ClaudeRunner::new(fixture("stub-claude.sh"));
    let out = r
        .run("实现功能", None, None, &mut |_: &str| {}, &PathBuf::from("."))
        .expect("ok");
    assert!(out.full_output.contains("STUB CLAUDE 收到: 实现功能"));
    assert_eq!(out.last_line.trim(), "https://gitlab.example.com/mr/42");
}

#[test]
fn skill_prefixes_prompt() {
    let r = ClaudeRunner::new(fixture("stub-claude.sh"));
    let out = r
        .run("审查", Some("four-dimension-review"), None, &mut |_: &str| {}, &PathBuf::from("."))
        .unwrap();
    assert!(out.full_output.contains("/four-dimension-review"));
}
