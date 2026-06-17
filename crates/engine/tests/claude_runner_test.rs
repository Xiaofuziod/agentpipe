use agentpipe_engine::runner::claude::ClaudeRunner;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

#[test]
fn runs_and_captures_result_as_answer() {
    let r = ClaudeRunner::new(fixture("stub-claude.sh"));
    let out = r
        .run("实现功能", None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."), false)
        .expect("ok");
    // full_output 是原始 NDJSON,含 assistant 文本
    assert!(out.full_output.contains("STUB CLAUDE 收到: 实现功能"));
    // answer 取自 result.result,绝不是 stdout 最后一行(那是 result JSON)
    assert_eq!(out.answer.trim(), "https://gitlab.example.com/mr/42");
    // 度量从 result 行解析
    assert_eq!(out.metrics.map(|m| m.num_turns), Some(1));
}

#[test]
fn reports_round_per_assistant_turn() {
    let r = ClaudeRunner::new(fixture("stub-claude.sh"));
    let mut rounds: Vec<Option<u32>> = Vec::new();
    let out = r
        .run(
            "实现功能",
            None,
            None,
            &mut |_: &str, round: Option<u32>| rounds.push(round),
            &PathBuf::from("."),
            false,
        )
        .expect("ok");
    // stub 一轮 assistant → 一次 round=Some(1) 进度上报
    assert_eq!(rounds, vec![Some(1)]);
    assert!(out.metrics.is_some());
}

#[test]
fn skill_prefixes_prompt() {
    let r = ClaudeRunner::new(fixture("stub-claude.sh"));
    let out = r
        .run("审查", Some("four-dimension-review"), None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."), false)
        .unwrap();
    assert!(out.full_output.contains("/four-dimension-review"));
}
