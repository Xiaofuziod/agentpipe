use agentpipe_engine::context::Verdict;
use agentpipe_engine::manifest::CodexAction;
use agentpipe_engine::runner::codex::CodexRunner;
use std::path::PathBuf;
use std::sync::Mutex;

// STUB_VERDICT 是进程级 env,并行测试需串行化避免竞态。
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

fn stub() -> CodexRunner {
    CodexRunner::new(fixture("stub-codex.sh"))
}

#[test]
fn parses_changes_requested() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "changes_requested");
    let r = stub()
        .review(&CodexAction::ReviewMr, None, Some("dev"), None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."))
        .expect("review ok");
    assert_eq!(r.verdict, Verdict::ChangesRequested);
    assert!(r.findings.contains("示例问题"));
}

#[test]
fn parses_clean() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "clean");
    let r = stub()
        .review(&CodexAction::ReviewMr, None, Some("dev"), None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."))
        .unwrap();
    assert_eq!(r.verdict, Verdict::Clean);
}

#[test]
fn unparseable_output_is_changes_requested() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 注入会产出非法 JSON 的 verdict,校验 fail-closed
    std::env::set_var("STUB_VERDICT", "\"broken");
    let r = stub()
        .review(&CodexAction::ReviewMr, None, Some("dev"), None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."))
        .unwrap();
    assert_eq!(r.verdict, Verdict::ChangesRequested);
}
