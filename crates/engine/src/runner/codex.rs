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

pub struct CodexRunner {
    bin: String,
}

#[derive(Deserialize)]
struct RawReview {
    verdict: String,
    #[serde(default)]
    findings: Vec<RawFinding>,
}

#[derive(Deserialize)]
struct RawFinding {
    #[serde(default)]
    severity: String,
    #[serde(default)]
    file: String,
    #[serde(default)]
    line: i64,
    #[serde(default)]
    summary: String,
}

impl CodexRunner {
    pub fn new(bin: String) -> Self {
        Self { bin }
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

        // 全部走通用 `codex exec`:实测 `codex exec review` 子命令的 -o 写的是散文,
        // 不认 --output-schema;只有通用 exec + 严格 schema 才把 -o 写成结构化 JSON。
        // review-doc 把文档内容经 stdin 喂给 codex(spec 7.2);其余 action 无 stdin。
        let (args, stdin): (Vec<String>, Option<String>) = match action {
            CodexAction::ReviewMr => {
                let b = base.unwrap_or("dev");
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
                            "审查当前工作区相对 `{b}` 分支的代码改动(查看 git diff {b}...HEAD 以及未提交改动),按 schema 输出 verdict(clean 或 changes_requested)和 findings"
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
                        format!("审查随附设计文档 {rel} 并按 schema 输出 verdict/findings"),
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
        run_command(&self.bin, &args, cwd, stdin.as_deref(), None, control, &mut raw_sink)?;
        Ok(parse_review(&out_file))
    }
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
    };
    let content = match std::fs::read_to_string(out_file) {
        Ok(c) => c,
        Err(_) => return fallback,
    };
    let raw: RawReview = match serde_json::from_str(content.trim()) {
        Ok(r) => r,
        Err(_) => return fallback,
    };
    let verdict = if raw.verdict == "clean" {
        Verdict::Clean
    } else {
        Verdict::ChangesRequested
    };
    let findings = raw
        .findings
        .iter()
        .map(|f| format!("[{}] {}:{} {}", f.severity, f.file, f.line, f.summary))
        .collect::<Vec<_>>()
        .join("\n");
    ReviewResult { verdict, findings }
}

// 必须是严格 JSON Schema:OpenAI 结构化输出要求每个 object 带 additionalProperties:false
// 且所有属性进 required,否则 API 报 invalid_json_schema(实测,见 cli-behavior-findings.md)。
const REVIEW_SCHEMA: &str = r#"{
  "type":"object","additionalProperties":false,
  "required":["verdict","findings"],
  "properties":{
    "verdict":{"type":"string","enum":["clean","changes_requested"]},
    "findings":{"type":"array","items":{
      "type":"object","additionalProperties":false,
      "required":["severity","file","line","summary"],
      "properties":{
        "severity":{"type":"string"},"file":{"type":"string"},
        "line":{"type":"integer"},"summary":{"type":"string"}}}}
  }
}"#;
