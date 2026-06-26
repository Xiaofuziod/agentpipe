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

// review §A finding #11:RawFinding.suggestion 去 #[serde(default)] 后,「缺 suggestion
// 字段 → fallback」语义与 malformed_finding_missing_core_field_falls_back_to_changes_requested
// 同源(任一字段缺失都触发整 RawReview 解析失败 → fallback ChangesRequested)。原
// legacy_finding_without_suggestion_field test 与 malformed test 是等价覆盖,删除避免冗余 —
// stub-codex.sh 现已补 suggestion 字段对齐新 schema,不再作为 legacy 输入。

#[test]
fn review_mr_rejects_dash_prefixed_base_ref_fail_loud() {
    // review §A finding #10:base_ref_resolvable 必须拒以 `-` 开头的 base,否则
    // `git rev-parse --verify --quiet --help` 把 `--help` 当 git option(印 help 退 0)
    // → 误判 ref 存在 → codex 实际跑 `git diff --help...HEAD` 输出乱码。
    // 双层防御:① 字面 reject 以 `-` 开头 ② --end-of-options 兜底。本测试守护 ①。
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let err = stub()
        .review(
            &CodexAction::ReviewMr,
            None,
            Some("--help"),
            None,
            None,
            &mut |_: &str, _: Option<u32>| {},
            &PathBuf::from("."),
        )
        .expect_err("以 - 开头的 base 必须 fail-loud,不能静默放过");
    let msg = err.to_string();
    assert!(
        msg.contains("无法解析") || msg.contains("--help"),
        "错误信息应明示 base ref 不可解析: {msg}"
    );
}

#[test]
fn placeholder_suggestion_trims_trailing_punctuation_and_full_width() {
    // review §A finding #12:占位过滤必须吃掉尾部标点和全角空白,否则 LLM 返回
    // "N/A." / "无。" / "(none)" 类装饰串绕过过滤,渲染出 "↳ 建议: N/A." 噪音
    // 喂下游 fixer。normalize_suggestion 在 render_finding 内调用,通过整 review
    // 跑完 → findings 文本端验证。
    use agentpipe_engine::protocol::ReviewResult;
    use std::env;
    // 直接驱动 parse_review_stdout 的私有路径不行(私有),通过 stub 注入构造场景:
    // 临时拼一个 codex 输出,占位 suggestion 有尾部全角句号 → 期望不渲染 ↳ 行。
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 用 ENV+stub 模拟:写一个动态 stub,产出 suggestion='N/A.' (尾部西文句号) +
    // suggestion='无。' (全角句号)+ suggestion='(none)' (括号包) 三条 finding,
    // 全部应被识别为 placeholder 不出 ↳ 行;再有一条真 suggestion 应出 ↳ 行确认
    // 过滤未误伤。
    let tmpdir = env::temp_dir();
    let stub_path = tmpdir.join("agentpipe-stub-placeholder.sh");
    std::fs::write(
        &stub_path,
        r#"#!/usr/bin/env bash
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
cat > "$out" <<'EOF'
{"verdict":"changes_requested","findings":[
  {"severity":"low","file":"a.rs","line":1,"summary":"P1","suggestion":"N/A."},
  {"severity":"low","file":"a.rs","line":2,"summary":"P2","suggestion":"无。"},
  {"severity":"low","file":"a.rs","line":3,"summary":"P3","suggestion":"(none)"},
  {"severity":"low","file":"a.rs","line":4,"summary":"P4","suggestion":"用 X 替换 Y"}
]}
EOF
echo "done"
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let r: ReviewResult = CodexRunner::new(stub_path.to_string_lossy().into_owned())
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
    // P1 / P2 / P3 都是占位,应被识别 → 不出 ↳ 建议
    assert!(!r.findings.contains("↳ 建议: N/A."), "尾部 . 应被识别为占位: {}", r.findings);
    assert!(!r.findings.contains("↳ 建议: 无。"), "全角句号应被剥离: {}", r.findings);
    assert!(!r.findings.contains("↳ 建议: (none)"), "括号包应被识别: {}", r.findings);
    // P4 真 suggestion 应正常渲染
    assert!(
        r.findings.contains("↳ 建议: 用 X 替换 Y"),
        "真 suggestion 不该被误伤: {}",
        r.findings
    );
    let _ = std::fs::remove_file(&stub_path);
}
