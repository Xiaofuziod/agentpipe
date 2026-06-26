use super::run_command;
use crate::control::Control;
use crate::context::Verdict;
use crate::error::EngineError;
use crate::manifest::CodexAction;
use crate::protocol::ReviewResult;
use serde::Deserialize;
use std::path::Path;
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

static OUT_SEQ: AtomicU64 = AtomicU64::new(0);

/// 一次性 warn:codex CLI 当前不在 stdout 输出 token usage,Verifier::Codex 不上报
/// metrics,budget_usd 对 codex 验证步骤形同虚设(review §A finding #2)。
static WARN_CODEX_COST_BYPASS: Once = Once::new();

/// review prompt 共用尾巴:要求 reviewer 为每个 finding 给具体可执行 suggestion。
/// 两处 prompt 共用,避免漂移(spec §3.2)。**自带前导空格 + 句号收尾**,与调用方
/// prompt 拼接时不产生双标点;**强约束「必须提供」** 保留模型动机(review-2 §A
/// finding #6:避免「可选」措辞让 LLM 走最省路径默认略过)。
const SUGGESTION_HINT: &str = " 每个 finding **必须**提供 suggestion 字段:具体可执行的修改建议(例:'第 42 行 nil 检查改成 if let Some(x) = y { ... }' / '把 unwrap 改为 ? 传播');无具体建议时填 \"N/A\"。";

/// codex review 单次墙钟上限(秒)。挂死 / provider 失联时到点 kill 整组,
/// review() 返回 Err 走 step 失败决策门,绝不冻住整个 run。可经
/// `AGENTPIPE_CODEX_TIMEOUT_SECS` 覆盖(>0 生效)。
/// 默认 1200s(20min):大 MR review 读大 diff + 推理本就慢,留足空间;
/// 仍远低于无超时时观测到的 6.5h 冻死。
const DEFAULT_CODEX_TIMEOUT_SECS: u64 = 1200;

pub struct CodexRunner {
    bin: String,
    timeout_secs: u64,
}

#[derive(Deserialize)]
struct RawReview {
    verdict: String,
    #[serde(default)]
    findings: Vec<RawFinding>,
}

#[derive(Deserialize)]
struct RawFinding {
    // 全部字段 required:与 REVIEW_SCHEMA `required: [...]` 对齐(OpenAI strict mode
    // 要求所有 properties 都 required,additionalProperties:false)。任一字段缺失即整条
    // 解析失败,走 fallback ChangesRequested,不静默渲染空串 / 默认值喂下游 fixer。
    //
    // review §A finding #11:旧版 suggestion 留 `#[serde(default)]` 与 schema required
    // 矛盾 —— 生产 OpenAI 拒老 codex 二进制(无 suggestion 字段)在 schema 层、serde
    // 反而 default 空串通过,两层语义对不上让 reader 困惑且 legacy guard 实际死代码。
    // 现统一 fail-loud:旧 codex 二进制必须升级,与 spec §3.2 文档明示「min codex
    // version」对齐。
    severity: String,
    file: String,
    line: i64,
    summary: String,
    /// 具体修改建议(spec §3.2),提升下游 fixer 的可操作性。
    suggestion: String,
}

impl CodexRunner {
    pub fn new(bin: String) -> Self {
        let timeout_secs = super::timeout_secs_from_env(
            "AGENTPIPE_CODEX_TIMEOUT_SECS",
            DEFAULT_CODEX_TIMEOUT_SECS,
        );
        Self { bin, timeout_secs }
    }

    /// 显式指定超时(秒),供测试注入小值;生产走 `new` 的默认 / env。
    pub fn with_timeout(bin: String, timeout_secs: u64) -> Self {
        Self { bin, timeout_secs }
    }

    /// 返回 ReviewResult。解析失败一律 fail-closed 为 ChangesRequested。
    #[allow(clippy::too_many_arguments)]
    pub fn review(
        &self,
        action: &CodexAction,
        doc_path: Option<&str>,
        base: Option<&str>,
        ask_prompt: Option<&str>,
        control: Option<&Control>,
        on_progress: &mut dyn FnMut(&str, Option<u32>),
        cwd: &Path,
    ) -> Result<ReviewResult, EngineError> {
        // 一次性显式告警:codex review 不上报 cost,budget_usd 对 codex 验证 step 无效。
        // review §A finding #2:让用户可观测 budget guard 在此通道处于 inactive,
        // 避免"配了 budget 仍被烧光"的反认知体验。等 codex CLI 升级出 usage 后填实。
        // 用 eprintln 同 acp.rs:CLI/Tauri 未装 tracing_subscriber,tracing 会被吞。
        WARN_CODEX_COST_BYPASS.call_once(|| {
            eprintln!(
                "[agentpipe] WARN: codex review 当前不上报 token cost(codex CLI \
                 暂无 usage 输出),codex step / Verifier::Codex 不计入 budget_usd \
                 — 如需 budget 兜底请用 Verifier::Claude 或等 codex CLI 升级。"
            );
        });

        let seq = OUT_SEQ.fetch_add(1, Ordering::Relaxed);
        let out_file = std::env::temp_dir()
            .join(format!("agentpipe-codex-{}-{}.json", std::process::id(), seq));
        let out_str = out_file.to_string_lossy().to_string();
        let schema = write_schema()?;

        // 全部走通用 `codex exec` + 严格 --output-schema 拿结构化 verdict。
        // 注:实测 codex v0.139.0 把最终结构化结果打到 stdout、并不写 -o(--output-last-message)
        // 文件,故下方以 stdout 为主、-o 为 fallback(`codex exec review` 子命令的 -o 写散文,弃用)。
        // review-doc 把文档内容经 stdin 喂给 codex(spec 7.2);其余 action 无 stdin。
        let (args, stdin): (Vec<String>, Option<String>) = match action {
            CodexAction::ReviewMr => {
                // base 必须由 caller 提供(写死或模板用 {{...}} 动态解析,见 templates/);
                // 不再有 fallback "dev" — 写死 "dev" 是「review-mr 默认审 dev」的隐式假设,
                // 在 main / master 仓库直接出错且报「ref `dev` 无法解析」让用户困惑。
                // 现在 None → fail-loud 明示「未提供 base 字段」,引导用户用 gh pr view
                // 动态填(see templates/mr-review-loop.yaml 的 base-detect step)。
                let b = match base {
                    Some(b) => b,
                    None => {
                        return Err(EngineError::Cli(
                            "review-mr 必须提供 base 字段(目标分支名)。模板里通常用 \
                             `base: \"{{base-detect.artifact}}\"` 让 claude step 跑 \
                             `gh pr view <MR-URL> --json baseRefName -q .baseRefName` 动态\
                             解析,或直接写死为该 MR 的真实目标分支(如 main / master)。"
                                .into(),
                        ));
                    }
                };
                // base ref 预检:review-mr 审的是 `git diff {b}...HEAD`。若 {b} 在目标仓库
                // 不可解析(分支名配错 / 仓库未 fetch 到该分支),codex 跑 diff 必然失败,
                // 只能把"没法审"返回成 changes_requested。引擎若信任该 verdict 喂回 loop,
                // until:codex-clean 永不满足 → loop 空转烧钱到 max(活锁,已观测)。
                // 这里 fail-loud 返回 Err → executor 走 step 失败决策门(暂停/中止),不静默放过。
                if !base_ref_resolvable(cwd, b) {
                    return Err(EngineError::Cli(format!(
                        "审查基线 ref `{b}` 在目标仓库无法解析(`git diff {b}...HEAD` 会报 unknown revision)。\
                         请确认 task 的 review.base 是该 MR 的真实目标分支(如 main/master),\
                         且目标仓库已 fetch 到该分支。"
                    )));
                }
                (
                    vec![
                        "exec".into(),
                        "-s".into(),
                        "read-only".into(),
                        "--output-schema".into(),
                        schema.clone(),
                        "-o".into(),
                        out_str.clone(),
                        format!(
                            "审查当前工作区相对 `{b}` 分支的代码改动(查看 git diff {b}...HEAD 以及未提交改动),按 schema 输出 verdict(clean 或 changes_requested)和 findings{SUGGESTION_HINT}"
                        ),
                    ],
                    None,
                )
            }
            CodexAction::ReviewDoc => {
                let rel = doc_path.unwrap_or("");
                let content = std::fs::read_to_string(cwd.join(rel)).unwrap_or_default();
                (
                    vec![
                        "exec".into(),
                        "-s".into(),
                        "read-only".into(),
                        "--output-schema".into(),
                        schema.clone(),
                        "-o".into(),
                        out_str.clone(),
                        format!(
                            "审查随附设计文档 {rel} 并按 schema 输出 verdict/findings{SUGGESTION_HINT}"
                        ),
                    ],
                    Some(content),
                )
            }
            CodexAction::Ask => (
                vec![
                    "exec".into(),
                    "-s".into(),
                    "read-only".into(),
                    "-o".into(),
                    out_str.clone(),
                    ask_prompt.unwrap_or("").into(),
                ],
                None,
            ),
        };

        // codex exec 输出非 NDJSON 协议,原始行直接作无轮次进度上报(round=None)。
        let mut raw_sink = |line: &str| on_progress(line, None);
        let started = std::time::Instant::now();
        let (stdout, success) = run_command(
            &self.bin,
            &args,
            cwd,
            stdin.as_deref(),
            Some(self.timeout_secs),
            control,
            &mut raw_sink,
        )?;
        // 超时:run_command 到点 killpg 返回 success=false,用墙钟区分超时与普通非零退出。
        // fail-closed 为 Err → executor 走 step 失败决策门(重试/跳过/中止),不把超时喂回 loop 重挂。
        if !success && started.elapsed() >= std::time::Duration::from_secs(self.timeout_secs) {
            return Err(EngineError::Cli(format!(
                "Codex 审查超时(>{}s),已中止",
                self.timeout_secs
            )));
        }
        // 真实 codex(v0.139.0)把最终结构化结果打到 stdout、不写 -o(--output-last-message)文件。
        // 故 stdout 优先:取最后一条能解析成 schema 的 JSON 行;读 -o 文件作 fallback
        // (stub / 旧 codex 路径,parse_review 自带"无法解析"兜底)。
        Ok(parse_review_stdout(&stdout).unwrap_or_else(|| parse_review(&out_file)))
    }
}

/// base ref 能否在 cwd 仓库解析为 commit。与 codex 实际跑的 `git diff {base}...HEAD`
/// 同一套 gitrevisions 规则(裸 ref,不做 `origin/` DWIM);`^{commit}` 确保解析到 commit-ish。
/// git 不可用 / 非 git 仓库 / ref 不存在一律返回 false → 调用方 fail-loud(绝不静默放过)。
///
/// 安全 + 容损(双层防御:① 字面 reject ② `--end-of-options` 强制后续 args 一律是 refs):
/// - `base` 以 `-` 开头:被当 git 选项(`--help` 退 0 印帮助;`--exec=` CVE 输入)。
/// - `base` 含空白 / 控制字符:多半是 LLM artifact 解析漂移(带换行 / 全角空格 /
///   markdown 代码块标记),让 rev-parse 喂到任何含空白的 ref 都必然失败,且错误
///   信息含原始字符让用户更难定位。这里 fail-loud 拒掉,引导上层 trim(executor 已 trim
///   首行,这条是兜底)。
///
/// `--end-of-options` 是 git 2.24+(2019),即便未来引入新选项也不混淆。
fn base_ref_resolvable(cwd: &Path, base: &str) -> bool {
    if base.starts_with('-') {
        return false;
    }
    if base.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{base}^{{commit}}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 占位词集合(reviewer 在没具体建议时常用的同义表达);整串(已 trim)小写匹配。
/// **收窄于 review-2 §D finding #9**:删 "no" / "-" / "todo" — code review 上下文
/// 这些常是合法短建议("No, use X instead" 被截 / 'TODO: 抽 helper' 简写 / markdown
/// 列表 '- xxx' 残留),整串等值匹配会误吞真实建议。保留高置信占位:n/a / none / 无 / tbd。
const SUGGESTION_PLACEHOLDERS: &[&str] = &["n/a", "none", "无", "tbd"];

/// suggestion 归一化:同时剥离 ASCII 空白 / 全角空白 / 包含中英标点的尾部装饰
/// (例 `"N/A."`、`"无。"`、`"None!"`、`"  无 "`)。
/// 修治 review §A finding #8:Rust `str::trim()` 只剥 Unicode whitespace,
/// 不动 `.` / `。` / `,` / `，` 等;LLM 给 schema-required 字段时为了"看起来像句子"
/// 常附标点,导致 'N/A.' 绕过 placeholder 检测、`"↳ 建议: N/A."`噪声喂下游 fixer。
fn normalize_suggestion(s: &str) -> String {
    s.trim_matches(|c: char| {
        c.is_whitespace() || ".,;:!?。，；：！？、…·•※()[]【】「」\"'`".contains(c)
    })
    .to_string()
}

/// 占位检测:输入应为 `normalize_suggestion` 归一后的小写串。
fn is_placeholder_suggestion(s: &str) -> bool {
    let lower = s.to_lowercase();
    SUGGESTION_PLACEHOLDERS.contains(&lower.as_str())
}

/// 单条 finding 渲染为人读行;suggestion 非空且非占位时附加 "↳ 建议: ..." 行,
/// 让下游 fixer prompt 直接看到可操作建议(spec §3.2)。
fn render_finding(f: &RawFinding) -> String {
    let head = format!("[{}] {}:{} {}", f.severity, f.file, f.line, f.summary);
    let s = normalize_suggestion(&f.suggestion);
    if s.is_empty() || is_placeholder_suggestion(&s) {
        head
    } else {
        format!("{head}\n  ↳ 建议: {s}")
    }
}

/// RawReview → ReviewResult(verdict 归一 + findings 扁平化)。解析两路共用,避免漂移。
/// metrics 始终 None:codex CLI 不在 stdout 输出 token usage,等升级后填。
fn raw_to_result(raw: RawReview) -> ReviewResult {
    let verdict = if raw.verdict == "clean" {
        Verdict::Clean
    } else {
        Verdict::ChangesRequested
    };
    let findings = raw
        .findings
        .iter()
        .map(render_finding)
        .collect::<Vec<_>>()
        .join("\n");
    ReviewResult { verdict, findings, metrics: None }
}

/// 从 codex stdout 抓最后一条能解析成 schema 的 JSON 行。无则 None(交给 -o fallback)。
fn parse_review_stdout(stdout: &str) -> Option<ReviewResult> {
    for line in stdout.lines().rev() {
        let t = line.trim();
        if t.starts_with('{') {
            if let Ok(raw) = serde_json::from_str::<RawReview>(t) {
                return Some(raw_to_result(raw));
            }
        }
    }
    None
}

fn write_schema() -> Result<String, EngineError> {
    // 按进程 + 序号命名,避免并发进程在同一固定路径上半写竞态。
    let seq = OUT_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "agentpipe-review-schema-{}-{}.json",
        std::process::id(),
        seq
    ));
    std::fs::write(&path, REVIEW_SCHEMA)?;
    Ok(path.to_string_lossy().to_string())
}

fn parse_review(out_file: &Path) -> ReviewResult {
    let fallback = ReviewResult {
        verdict: Verdict::ChangesRequested,
        findings: "(无法解析 Codex 输出,按需修改处理)".into(),
        metrics: None,
    };
    let content = match std::fs::read_to_string(out_file) {
        Ok(c) => c,
        Err(_) => return fallback,
    };
    match serde_json::from_str::<RawReview>(content.trim()) {
        Ok(raw) => raw_to_result(raw),
        Err(_) => fallback,
    }
}

// 必须是严格 JSON Schema:OpenAI 结构化输出要求每个 object 带 additionalProperties:false
// 且所有属性进 required,否则 API 报 invalid_json_schema(实测,见 docs/specs/cli-behavior-findings.md:18)。
//
// suggestion 字段(spec §3.2):reviewer 提供具体修改建议,提升下游 fixer 反馈深度。
// **必填**:OpenAI strict mode 要求所有 properties 都 required;RawFinding 端也去掉了
// `#[serde(default)]`,两层语义对齐(review §A finding #11 — 旧版 schema required +
// serde default 矛盾,生产 OpenAI 拒老 codex 在 schema 层、serde 反而 default 通过,
// 让 reader 困惑且 legacy guard 实际是死代码)。旧 codex 二进制需升级到支持新 schema,
// 与 agentpipe 锁版本策略一致(spec §3.2 follow-up:文档明示 min codex)。
const REVIEW_SCHEMA: &str = r#"{
  "type":"object","additionalProperties":false,
  "required":["verdict","findings"],
  "properties":{
    "verdict":{"type":"string","enum":["clean","changes_requested"]},
    "findings":{"type":"array","items":{
      "type":"object","additionalProperties":false,
      "required":["severity","file","line","summary","suggestion"],
      "properties":{
        "severity":{"type":"string"},"file":{"type":"string"},
        "line":{"type":"integer"},"summary":{"type":"string"},
        "suggestion":{"type":"string"}}}}
  }
}"#;
