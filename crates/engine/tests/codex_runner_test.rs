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

#[test]
fn renders_suggestion_when_present_and_skips_na_placeholder() {
    // spec §3.2:reviewer 给出具体可执行 suggestion 时,渲染出 "↳ 建议:" 行;
    // suggestion 为 "N/A" 占位(或大小写变体)则不渲染,避免噪音。
    let r = CodexRunner::new(fixture("stub-codex-with-suggestion.sh"))
        .review(
            &CodexAction::ReviewMr,
            None,
            Some("HEAD"),
            None,
            None,
            &mut |_: &str, _: Option<u32>| {},
            &PathBuf::from("."),
        )
        .expect("review ok");
    assert_eq!(r.verdict, Verdict::ChangesRequested);
    // 第一个 finding 带具体 suggestion → 必须有 ↳ 行
    assert!(
        r.findings.contains("↳ 建议: 第 10 行加"),
        "应渲染具体 suggestion: {}",
        r.findings
    );
    // 第二个 finding suggestion="N/A" → 不应渲染 ↳ 行
    let na_lines = r
        .findings
        .lines()
        .filter(|l| l.contains("N/A"))
        .count();
    assert_eq!(na_lines, 0, "N/A 占位不应渲染: {}", r.findings);
}

#[test]
fn placeholder_suggestions_skip_recommend_line() {
    // review-fix §D finding #14:占位词集合扩展 — "none" / "无" / "TBD" / "-" 都不应
    // 渲染 "↳ 建议:" 行(防止噪音稀释下游 fixer 的真实 suggestion)。
    let r = CodexRunner::new(fixture("stub-codex-placeholder-suggestion.sh"))
        .review(
            &CodexAction::ReviewMr,
            None,
            Some("HEAD"),
            None,
            None,
            &mut |_: &str, _: Option<u32>| {},
            &PathBuf::from("."),
        )
        .expect("review ok");
    // 真 suggestion 渲染 ↳
    assert!(r.findings.contains("↳ 建议: 用 X 替换 Y"), "真 suggestion 应渲染: {}", r.findings);
    // 占位词集合(none / 无 / TBD)不应出现在 ↳ 行。
    // 注:review-2 §C finding #4 修正 — 用 ends_with(": <占位>") 锚定后缀,
    // 之前 l.contains("无 ")(尾随空格)与实际渲染 "  ↳ 建议: 无"(末尾无字符)永不匹配,
    // 测试假绿。同时占位集合在 review-2 §D finding #9 收窄(删 no/-/todo)后,这里
    // 只验剩下的 n/a / none / 无 / tbd 四类。
    let placeholder_arrows: Vec<&str> = r
        .findings
        .lines()
        .filter(|l| l.contains("↳"))
        .filter(|l| {
            let t = l.trim_end();
            t.ends_with(": none")
                || t.ends_with(": 无")
                || t.ends_with(": TBD")
                || t.ends_with(": tbd")
        })
        .collect();
    assert!(
        placeholder_arrows.is_empty(),
        "占位词不应渲染 ↳ 行: {placeholder_arrows:?}"
    );
}

#[test]
fn malformed_finding_missing_core_field_falls_back_to_changes_requested() {
    // review-2 §D finding #8:RawFinding 去 #[serde(default)] 后,缺核心字段(此处
    // severity)整次解析失败 → fallback `(无法解析 Codex 输出,按需修改处理)`,
    // 而非静默渲染 "[] :0 summary" 乱码喂下游 fixer。
    let r = CodexRunner::new(fixture("stub-codex-malformed-finding.sh"))
        .review(
            &CodexAction::ReviewMr,
            None,
            Some("HEAD"),
            None,
            None,
            &mut |_: &str, _: Option<u32>| {},
            &PathBuf::from("."),
        )
        .expect("review ok(走 fallback,不抛 Err)");
    assert_eq!(r.verdict, Verdict::ChangesRequested, "缺字段必须 fail-closed");
    assert!(
        r.findings.contains("无法解析"),
        "fallback findings 必须明示解析失败,可观测: {}",
        r.findings
    );
    // 关键:绝对不能渲染出 "[] :0 缺 severity 字段" 这种乱码
    assert!(
        !r.findings.contains("缺 severity 字段"),
        "缺字段时不应静默渲染部分字段(防 review-fix §D #7 残留): {}",
        r.findings
    );
}

#[test]
fn legacy_finding_without_suggestion_field_still_parses() {
    // 旧 fixture(stub-codex.sh)输出不含 suggestion 字段;serde default 给空串,
    // 渲染时按"无建议"跳过 ↳ 行,保持向后兼容。
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STUB_VERDICT", "changes_requested");
    let r = stub()
        .review(&CodexAction::ReviewMr, None, Some("HEAD"), None, None, &mut |_: &str, _: Option<u32>| {}, &PathBuf::from("."))
        .expect("review ok");
    assert!(r.findings.contains("示例问题"), "legacy 字段应正常解析");
    assert!(
        !r.findings.contains("↳"),
        "无 suggestion 字段时不应有 ↳ 行: {}",
        r.findings
    );
}
