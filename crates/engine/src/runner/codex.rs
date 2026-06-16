use super::run_command;
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
    pub fn review(
        &self,
        action: &CodexAction,
        doc_path: Option<&str>,
        base: Option<&str>,
        ask_prompt: Option<&str>,
        cwd: &Path,
    ) -> Result<ReviewResult, EngineError> {
        let seq = OUT_SEQ.fetch_add(1, Ordering::Relaxed);
        let out_file = std::env::temp_dir()
            .join(format!("agentpipe-codex-{}-{}.json", std::process::id(), seq));
        let out_str = out_file.to_string_lossy().to_string();
        let schema = write_schema()?;

        // review-doc 把文档内容经 stdin 喂给 codex(spec 7.2);其余 action 无 stdin。
        let (args, stdin): (Vec<String>, Option<String>) = match action {
            CodexAction::ReviewMr => (
                vec![
                    "exec".into(),
                    "review".into(),
                    "--base".into(),
                    base.unwrap_or("dev").into(),
                    "--output-schema".into(),
                    schema.clone(),
                    "-o".into(),
                    out_str.clone(),
                    "-s".into(),
                    "read-only".into(),
                ],
                None,
            ),
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

        run_command(&self.bin, &args, cwd, stdin.as_deref(), None)?;
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

const REVIEW_SCHEMA: &str = r#"{
  "type":"object","required":["verdict","findings"],
  "properties":{
    "verdict":{"type":"string","enum":["clean","changes_requested"]},
    "findings":{"type":"array","items":{"type":"object","properties":{
      "severity":{"type":"string"},"file":{"type":"string"},
      "line":{"type":"integer"},"summary":{"type":"string"}}}}
  }
}"#;
