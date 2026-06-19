use super::run_command;
use crate::control::Control;
use crate::error::EngineError;
use crate::protocol::StepMetrics;
use serde_json::Value;
use std::path::Path;

/// claude 单步墙钟上限(秒)。比 codex review 宽得多——实现步骤合法可跑十几分钟;
/// 仅作挂死 / provider 失联的兜底,到点 killpg 整组、run() 返回 Err 走 step 失败决策门。
/// 可经 `AGENTPIPE_CLAUDE_TIMEOUT_SECS` 覆盖(>0 生效)。
const DEFAULT_CLAUDE_TIMEOUT_SECS: u64 = 1800;

pub struct ClaudeRunner {
    bin: String,
    timeout_secs: u64,
}

pub struct ClaudeOutcome {
    /// 最终答案文本(供下游 `{{step}}` 插值)。来自 stream-json 的 `result.result`,
    /// 回退链见 StreamParser::answer。**绝不是 stdout 最后一行**(那是 result JSON)。
    pub answer: String,
    pub metrics: Option<StepMetrics>,
    /// 原始 NDJSON,留作调试。
    pub full_output: String,
}

impl ClaudeRunner {
    pub fn new(bin: String) -> Self {
        let timeout_secs = std::env::var("AGENTPIPE_CLAUDE_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_CLAUDE_TIMEOUT_SECS);
        Self { bin, timeout_secs }
    }

    /// 显式指定超时(秒),供测试注入小值;生产走 `new` 的默认 / env。
    pub fn with_timeout(bin: String, timeout_secs: u64) -> Self {
        Self { bin, timeout_secs }
    }

    /// 干活步骤(`read_only=false`)以 bypassPermissions 跑:headless 下唯有它能让 claude
    /// 自主 edit + bash(提交/建 MR 需要),acceptEdits 只放行编辑、挡 bash。
    /// 校验步骤(`read_only=true`)以 `--permission-mode plan` 跑:只读探查、不改仓库,
    /// 用于 claude-as-verifier 判定(fail-closed 安全:verifier 不应改动 target)。
    /// 见 docs/specs/cli-behavior-findings.md。墙钟超时(默认 1800s)兜底挂死,另有控制台 Interrupt。
    ///
    /// 用 `--output-format stream-json --verbose` 拿逐行 NDJSON:每个 `assistant` 行 = 一轮
    /// 模型请求(经 `on_progress(label, Some(round))` 上报),终态 `result` 行带轮次/耗时/成本
    /// 与最终答案。解析见 StreamParser、设计见 docs/specs/2026-06-17-step-progress-streaming-design.md。
    pub fn run(
        &self,
        prompt: &str,
        skill: Option<&str>,
        control: Option<&Control>,
        on_progress: &mut dyn FnMut(&str, Option<u32>),
        cwd: &Path,
        read_only: bool,
    ) -> Result<ClaudeOutcome, EngineError> {
        let full_prompt = match skill {
            Some(s) => format!("/{s} {prompt}"),
            None => prompt.to_string(),
        };
        let permission_mode = if read_only { "plan" } else { "bypassPermissions" };
        let args = vec![
            "--permission-mode".to_string(),
            permission_mode.to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "-p".to_string(),
            full_prompt,
        ];

        let mut parser = StreamParser::new();
        let started = std::time::Instant::now();
        // 用块限定 raw_sink 的借用,使其在 run_command 返回后立即释放 parser/on_progress。
        let (stdout, success) = {
            let mut raw_sink = |raw: &str| {
                if let Some(turn) = parser.feed(raw) {
                    on_progress(&turn.label, Some(turn.round));
                }
            };
            run_command(&self.bin, &args, cwd, None, Some(self.timeout_secs), control, &mut raw_sink)?
        };
        if !success {
            // 超时:run_command 到点 killpg 返回 success=false,用墙钟区分超时与普通非零退出。
            // 两路都 fail-closed 为 Err → executor 走 step 失败决策门,挂死不会冻住整个 run。
            if started.elapsed() >= std::time::Duration::from_secs(self.timeout_secs) {
                return Err(EngineError::Cli(format!(
                    "claude 步骤超时(>{}s),已中止",
                    self.timeout_secs
                )));
            }
            return Err(EngineError::Cli("claude 非零退出".into()));
        }
        Ok(ClaudeOutcome {
            answer: parser.answer(),
            metrics: parser.metrics(),
            full_output: stdout,
        })
    }
}

/// 一轮模型请求的进度信号。
struct Turn {
    round: u32,
    label: String,
}

/// 逐行喂 claude stream-json,产出轮次进度并累积终态(答案 + 度量)。
struct StreamParser {
    turns: u32,
    last_assistant_text: Option<String>,
    answer: Option<String>,
    metrics: Option<StepMetrics>,
}

impl StreamParser {
    fn new() -> Self {
        Self {
            turns: 0,
            last_assistant_text: None,
            answer: None,
            metrics: None,
        }
    }

    /// 喂一行原始 stdout。返回 Some(Turn) 表示一轮 assistant 响应(上报进度);
    /// 非 JSON / 非关注类型一律 None(display 侧 fail-open 忽略,不当进度)。
    fn feed(&mut self, raw: &str) -> Option<Turn> {
        let v: Value = serde_json::from_str(raw.trim()).ok()?;
        match v.get("type").and_then(Value::as_str)? {
            "assistant" => {
                self.turns += 1;
                let content = v.get("message").and_then(|m| m.get("content")).and_then(Value::as_array);
                if let Some(text) = first_text_block(content) {
                    self.last_assistant_text = Some(text);
                }
                Some(Turn {
                    round: self.turns,
                    label: derive_label(content),
                })
            }
            "result" => {
                self.answer = v.get("result").and_then(Value::as_str).map(str::to_string);
                self.metrics = Some(StepMetrics {
                    num_turns: v
                        .get("num_turns")
                        .and_then(Value::as_u64)
                        .unwrap_or(self.turns as u64) as u32,
                    duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
                    cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
                });
                None
            }
            _ => None,
        }
    }

    /// 答案 fail-closed 回退链:`result.result` → 末个 assistant 的 text 块 → 空串。
    /// 绝不返回原始 JSON,防下游 `{{step}}` 拿到一坨结构。
    fn answer(&self) -> String {
        self.answer
            .clone()
            .or_else(|| self.last_assistant_text.clone())
            .unwrap_or_default()
    }

    fn metrics(&self) -> Option<StepMetrics> {
        self.metrics.clone()
    }
}

/// 从一轮 assistant 的 content 块派生人读 label:工具调用 > 文本 > 思考。
fn derive_label(content: Option<&Vec<Value>>) -> String {
    let blocks = match content {
        Some(b) => b,
        None => return String::new(),
    };
    let mut tool_names = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|b| b.get("name").and_then(Value::as_str));
    if let Some(first) = tool_names.next() {
        return if tool_names.next().is_some() {
            format!("调用 {first} 等")
        } else {
            format!("调用 {first}")
        };
    }
    if let Some(text) = first_text_block(content) {
        let flat = truncate_flat(&text, 60);
        if !flat.is_empty() {
            return flat;
        }
    }
    if blocks.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("thinking")) {
        return "思考中".to_string();
    }
    String::new()
}

fn first_text_block(content: Option<&Vec<Value>>) -> Option<String> {
    content?
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|b| b.get("text").and_then(Value::as_str))
        .map(str::to_string)
}

/// 压扁空白 + 按字符截断(CJK 安全),超长加省略号。
fn truncate_flat(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(content: &str) -> String {
        format!(r#"{{"type":"assistant","message":{{"role":"assistant","content":{content}}}}}"#)
    }

    #[test]
    fn tool_use_label_and_round_increment() {
        let mut p = StreamParser::new();
        let t = p
            .feed(&assistant(r#"[{"type":"tool_use","name":"Bash","input":{}}]"#))
            .expect("turn");
        assert_eq!(t.round, 1);
        assert_eq!(t.label, "调用 Bash");
        let t2 = p
            .feed(&assistant(r#"[{"type":"tool_use","name":"Read"},{"type":"tool_use","name":"Edit"}]"#))
            .expect("turn");
        assert_eq!(t2.round, 2);
        assert_eq!(t2.label, "调用 Read 等");
    }

    #[test]
    fn text_label_truncates_and_thinking_fallback() {
        let mut p = StreamParser::new();
        let long = "一".repeat(80);
        let t = p
            .feed(&assistant(&format!(r#"[{{"type":"text","text":"{long}"}}]"#)))
            .expect("turn");
        assert!(t.label.ends_with('…'));
        assert_eq!(t.label.chars().count(), 61); // 60 字 + 省略号
        let t2 = p.feed(&assistant(r#"[{"type":"thinking","thinking":"..."}]"#)).expect("turn");
        assert_eq!(t2.label, "思考中");
    }

    #[test]
    fn result_yields_answer_and_metrics() {
        let mut p = StreamParser::new();
        assert!(p
            .feed(r#"{"type":"result","subtype":"success","num_turns":3,"duration_ms":3266,"total_cost_usd":0.49,"result":"最终答案"}"#)
            .is_none());
        assert_eq!(p.answer(), "最终答案");
        let m = p.metrics().expect("metrics");
        assert_eq!(m.num_turns, 3);
        assert_eq!(m.duration_ms, 3266);
        assert!((m.cost_usd - 0.49).abs() < 1e-9);
    }

    #[test]
    fn answer_falls_back_to_last_assistant_text_then_empty() {
        // 无 result 行 → 回退末个 assistant text 块
        let mut p = StreamParser::new();
        p.feed(&assistant(r#"[{"type":"text","text":"草稿答案"}]"#));
        assert_eq!(p.answer(), "草稿答案");
        // 什么都没有 → 空串(绝不返回 JSON)
        let p2 = StreamParser::new();
        assert_eq!(p2.answer(), "");
    }

    #[test]
    fn non_json_and_unknown_types_are_ignored() {
        let mut p = StreamParser::new();
        assert!(p.feed("not json at all").is_none());
        assert!(p.feed(r#"{"type":"system","subtype":"init"}"#).is_none());
        assert!(p.feed(r#"{"type":"rate_limit_event"}"#).is_none());
        // 忽略行不消耗轮次计数
        let t = p.feed(&assistant(r#"[{"type":"text","text":"hi"}]"#)).expect("turn");
        assert_eq!(t.round, 1);
    }
}
