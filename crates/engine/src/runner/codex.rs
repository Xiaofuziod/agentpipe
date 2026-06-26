use super::run_command;
use crate::control::Control;
use crate::context::Verdict;
use crate::error::EngineError;
use crate::manifest::CodexAction;
use crate::protocol::ReviewResult;
use serde::Deserialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static OUT_SEQ: AtomicU64 = AtomicU64::new(0);

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
    // 核心字段去 #[serde(default)]:与 REVIEW_SCHEMA required 对齐;缺失即整条解析
    // 失败,走 fallback ChangesRequested(review-fix §D finding #7 治本,不再静默
    // 渲染 "[] :0 " 乱码喂下游 fixer)。
    severity: String,
    file: String,
    line: i64,
    summary: String,
    /// 具体修改建议(spec §3.2),提升下游 fixer 的可操作性。
    /// optional:旧 codex 二进制不输出时 serde 给空串,渲染时按"无建议"处理。
    /// (review-fix §D:从 required 改 optional,与 spec §3.2「向后兼容」对齐)
    #[serde(default)]
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
                let b = base.unwrap_or("dev");
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
fn base_ref_resolvable(cwd: &Path, base: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--verify", "--quiet"])
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

/// caller(render_finding)已 trim 输入,此处只 to_lowercase 即可(review-2 §D
/// finding #13:删冗余 inner trim)。
fn is_placeholder_suggestion(s: &str) -> bool {
    SUGGESTION_PLACEHOLDERS.contains(&s.to_lowercase().as_str())
}

/// 单条 finding 渲染为人读行;suggestion 非空且非占位时附加 "↳ 建议: ..." 行,
/// 让下游 fixer prompt 直接看到可操作建议(spec §3.2)。
fn render_finding(f: &RawFinding) -> String {
    let head = format!("[{}] {}:{} {}", f.severity, f.file, f.line, f.summary);
    let s = f.suggestion.trim();
    if s.is_empty() || is_placeholder_suggestion(s) {
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
// **必填**(review-2 §A finding #1 修正):上一轮把 suggestion 从 required 移出与「所有
// properties 必须 required」OpenAI 约束冲突,真实 codex 调用会被 422 invalid_json_schema
// reject 触发 loop 活锁。现回归 required;旧 codex 二进制不支持新 schema 时直接报错(用户
// 责任升级 codex,与 agentpipe 锁版本策略一致 — spec §3.2 follow-up:文档明示 min codex)。
// RawFinding.suggestion 仍 #[serde(default)] 兜底解析端(mock fixture / 边缘 JSON)。
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
