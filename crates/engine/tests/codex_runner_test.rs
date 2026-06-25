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
        .review(&CodexAction::ReviewMr, None, Some("HEAD"), None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."))
        .expect("review ok");
    assert_eq!(r.verdict, Verdict::ChangesRequested);
    assert!(r.findings.contains("示例问题"));
}

#[test]
fn parses_clean() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "clean");
    let r = stub()
        .review(&CodexAction::ReviewMr, None, Some("HEAD"), None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."))
        .unwrap();
    assert_eq!(r.verdict, Verdict::Clean);
}

#[test]
fn parses_verdict_from_stdout_when_no_output_file() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 真实 codex(v0.139.0)把最终结构化 JSON 打到 stdout、不写 -o 文件:
    // 引擎必须能从 stdout 解析出 verdict,而不是因 -o 缺失 fail-closed 成 changes_requested。
    std::env::set_var("STUB_VERDICT", "clean");
    let r = CodexRunner::new(fixture("stub-codex-stdout.sh"))
        .review(&CodexAction::ReviewMr, None, Some("HEAD"), None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."))
        .expect("review ok");
    assert_eq!(r.verdict, Verdict::Clean);
}

#[test]
fn review_times_out_and_errors() {
    // 挂死的 codex(睡 30s)+ 1s 超时 → 应在 ~1s 内超时返回 Err,不冻住整个 run。
    let r = CodexRunner::with_timeout(fixture("stub-codex-hang.sh"), 1).review(
        &CodexAction::ReviewMr,
        None,
        Some("HEAD"),
        None,
        None,
        &mut |_: &str, _: Option<u32>| {},
        &PathBuf::from("."),
    );
    assert!(r.is_err(), "超时应返回 Err,实际: {:?}", r.map(|x| x.verdict));
}

#[test]
fn review_mr_errors_when_base_ref_missing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // base ref 在目标仓库不可解析时,codex review-mr 跑 `git diff base...HEAD` 必然失败,
    // 只能把"没法审"表达成 changes_requested。引擎若信任这个 verdict 喂回 loop,
    // until:codex-clean 永不满足 → loop 空转烧钱到 max(活锁)。
    // 正确行为:fail-loud 返回 Err,交 executor 决策门(暂停/中止),绝不静默放过。
    std::env::set_var("STUB_VERDICT", "changes_requested");
    let r = stub().review(
        &CodexAction::ReviewMr,
        None,
        Some("agentpipe-nonexistent-base-ref"),
        None,
        None,
        &mut |_: &str, _: Option<u32>| {},
        &PathBuf::from("."),
    );
    assert!(
        r.is_err(),
        "base ref 不存在应 fail-loud 返回 Err,实际: {:?}",
        r.map(|x| x.verdict)
    );
}

#[test]
fn unparseable_output_is_changes_requested() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 注入会产出非法 JSON 的 verdict,校验 fail-closed
    std::env::set_var("STUB_VERDICT", "\"broken");
    let r = stub()
        .review(&CodexAction::ReviewMr, None, Some("HEAD"), None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."))
        .unwrap();
    assert_eq!(r.verdict, Verdict::ChangesRequested);
}
