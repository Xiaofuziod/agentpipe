# AgentPipe 引擎 + CLI 实施计划(Phase 1a)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 Rust 实现 AgentPipe 的 headless 引擎核心和一个命令行驱动,能解析 task.yaml、串行执行 claude/codex/human/loop 步骤、跑通 Codex "改到干净" 循环,全程在命令行端到端可验证。

**Architecture:** Cargo workspace。`engine` 纯逻辑库 crate(manifest 解析 / 上下文插值 / 状态机 / CLI runner / 协议类型),不依赖任何 UI。`cli` 二进制 crate 驱动引擎,把事件打到 stdout、用 stdin 处理 gate。CLI 子进程(claude/codex)当黑盒 spawn;单测用 stub 脚本替身,不打真 CLI。

**Tech Stack:** Rust(stable),serde + serde_yml(YAML)+ serde_json(Codex 结构化输出),thiserror(错误),std::process(子进程),std::sync::mpsc(事件/指令通道)。本 Phase 不引入 tokio、不引入 Tauri。

参考设计文档:`docs/specs/2026-06-16-design.md`。

---

## 文件结构

```
agentpipe/
├─ Cargo.toml                      # workspace
├─ crates/
│  ├─ engine/
│  │  ├─ Cargo.toml
│  │  └─ src/
│  │     ├─ lib.rs                 # pub mod 导出
│  │     ├─ manifest.rs            # Manifest / Step / StepKind / 解析 + 校验
│  │     ├─ context.rs             # RunContext / StepOutput / 插值
│  │     ├─ protocol.rs            # Event / Command 枚举
│  │     ├─ runner/
│  │     │  ├─ mod.rs              # CliOutcome + spawn 公共逻辑
│  │     │  ├─ claude.rs           # ClaudeRunner
│  │     │  └─ codex.rs            # CodexRunner + Verdict 解析
│  │     ├─ executor.rs            # Executor 状态机 + loop
│  │     └─ error.rs               # EngineError
│  └─ cli/
│     ├─ Cargo.toml
│     └─ src/main.rs               # agentpipe run <task.yaml>
├─ templates/                      # 内置 manifest 模板(Task 13)
└─ tests/fixtures/                 # stub 脚本 + 样例 manifest
```

每个文件单一职责:manifest 只管配置形状,context 只管产物与插值,runner 只管 spawn 一个 CLI,executor 只管编排状态机。

---

## Task 1: CLI 行为实测 spike(先验证再写码)

**目的:** spec 第 14 节三个未决点是 runner 的地基,必须先用真 CLI 实测,把确切调用方式和输出形状写成文档,后续 runner 按实测事实写,不按猜测。

**Files:**
- Create: `docs/specs/cli-behavior-findings.md`

- [ ] **Step 1: 实测 codex 结构化审查输出**

在一个有未提交改动的 git 仓库里(可用 sibling-repo 制造一处小改动)分别跑,记录确切行为:

```bash
# 准备一个 schema 文件
cat > /tmp/review-schema.json <<'EOF'
{
  "type": "object",
  "required": ["verdict", "findings"],
  "properties": {
    "verdict": { "type": "string", "enum": ["clean", "changes_requested"] },
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "severity": { "type": "string" },
          "file": { "type": "string" },
          "line": { "type": "integer" },
          "summary": { "type": "string" }
        }
      }
    }
  }
}
EOF

# 审仓库改动(review-mr 形态)
codex exec review --base dev --output-schema /tmp/review-schema.json -o /tmp/codex-out.json -s read-only
cat /tmp/codex-out.json

# 审一段文档(review-doc 形态,通用 exec)
echo "审查这份设计文档,按 schema 输出结论" | codex exec -s read-only --output-schema /tmp/review-schema.json -o /tmp/codex-doc.json -
cat /tmp/codex-doc.json
```

记录到 findings.md:`-o` 文件实际内容是不是纯 JSON、是否严格符合 schema、有无前后噪声、verdict 字段是否稳定出现。

- [ ] **Step 2: 实测 claude headless 触发 skill + 自主写码姿态**

```bash
# 在一个测试仓库 cwd 下
# a) 能否触发 skill
claude -p "/four-dimension-review 审查当前改动" --output-format text 2>&1 | head -40

# b) 自主写码 + 提交需要什么权限姿态(实测哪个 flag 能让它自主 edit/bash/commit)
claude -p "在 README 末尾加一行 'hello',然后 git commit" --permission-mode acceptEdits 2>&1 | tail -40
# 若上面被审批拦住,再试更宽松姿态并记录确切 flag
```

记录到 findings.md:`/skill` 是否真触发、自主写码所需的确切 flag、最终输出能否取到结果文本(供解析 MR URL)。

- [ ] **Step 3: 写结论文档并提交**

把 Step 1/2 的确切命令、输出样例、踩到的坑写进 `docs/specs/cli-behavior-findings.md`,标注"runner 实现以此为准"。

```bash
git add docs/specs/cli-behavior-findings.md
git commit -m "docs: claude/codex CLI 行为实测结论"
```

若实测发现 `--output-schema` 不稳定或 `/skill` 不触发,在文档里记录替代方案(如改用纯 prompt 约定 JSON / 用 settings 触发 skill),并据此调整后续 Task 的 runner 命令。

---

## Task 2: Cargo workspace 骨架

**Files:**
- Create: `Cargo.toml`
- Create: `crates/engine/Cargo.toml`, `crates/engine/src/lib.rs`
- Create: `crates/cli/Cargo.toml`, `crates/cli/src/main.rs`

- [ ] **Step 1: workspace Cargo.toml**

```toml
[workspace]
resolver = "2"
members = ["crates/engine", "crates/cli"]
```

- [ ] **Step 2: engine crate Cargo.toml**

```toml
[package]
name = "agentpipe-engine"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_yml = "0.0.12"
serde_json = "1"
thiserror = "1"
```

- [ ] **Step 3: engine lib.rs(空模块导出)**

```rust
pub mod context;
pub mod error;
pub mod executor;
pub mod manifest;
pub mod protocol;
pub mod runner;
```

先创建对应空文件(每个放一行 `// placeholder`),保证 `cargo build` 通过。

- [ ] **Step 4: cli crate Cargo.toml + main.rs 占位**

```toml
[package]
name = "agentpipe-cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "agentpipe"
path = "src/main.rs"

[dependencies]
agentpipe-engine = { path = "../engine" }
```

```rust
fn main() {
    println!("agentpipe");
}
```

- [ ] **Step 5: 构建验证**

Run: `cargo build`
Expected: 编译通过,无错误。

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/
git commit -m "chore: cargo workspace 骨架(engine + cli)"
```

---

## Task 3: Manifest 类型与解析

**Files:**
- Modify: `crates/engine/src/manifest.rs`
- Create: `crates/engine/tests/manifest_test.rs`
- Create: `tests/fixtures/sample-task.yaml`

- [ ] **Step 1: 写样例 manifest fixture**

`tests/fixtures/sample-task.yaml`:

```yaml
version: 1
name: "demo"
target: /tmp/demo-repo
mode: auto
steps:
  - id: design-codex
    kind: codex
    action: review-doc
    path: "docs/spec.md"
  - id: fix-loop
    kind: loop
    until: codex-clean
    max: 3
    body:
      - id: review-mr
        kind: codex
        action: review-mr
        base: dev
      - id: apply
        kind: claude
        prompt: "按反馈修改:{{review-mr.findings}}"
        allow_writes: true
```

- [ ] **Step 2: 写失败测试**

`crates/engine/tests/manifest_test.rs`:

```rust
use agentpipe_engine::manifest::{Manifest, Step, StepKind, CodexAction, RunMode};

#[test]
fn parses_sample_manifest() {
    let yaml = include_str!("../../../tests/fixtures/sample-task.yaml");
    let m = Manifest::parse(yaml).expect("should parse");
    assert_eq!(m.version, 1);
    assert_eq!(m.name, "demo");
    assert!(matches!(m.mode, RunMode::Auto));
    assert_eq!(m.steps.len(), 2);

    match &m.steps[0].kind {
        StepKind::Codex { action: CodexAction::ReviewDoc, path, .. } => {
            assert_eq!(path.as_deref(), Some("docs/spec.md"));
        }
        other => panic!("expected codex review-doc, got {:?}", other),
    }

    match &m.steps[1].kind {
        StepKind::Loop { until, max, body } => {
            assert_eq!(until, "codex-clean");
            assert_eq!(*max, 3);
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected loop, got {:?}", other),
    }
}

#[test]
fn rejects_invalid_yaml() {
    let err = Manifest::parse("not: [valid").unwrap_err();
    assert!(err.to_string().contains("parse"));
}
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cargo test -p agentpipe-engine manifest`
Expected: FAIL(类型未定义 / 编译错误)。

- [ ] **Step 4: 实现 manifest.rs**

```rust
use std::path::PathBuf;
use serde::Deserialize;
use crate::error::EngineError;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub name: String,
    pub target: PathBuf,
    #[serde(default)]
    pub mode: RunMode,
    pub steps: Vec<Step>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Step,
    #[default]
    Auto,
}

#[derive(Debug, Deserialize)]
pub struct Step {
    pub id: String,
    #[serde(flatten)]
    pub kind: StepKind,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StepKind {
    Claude {
        prompt: String,
        #[serde(default)]
        skill: Option<String>,
        #[serde(default)]
        allow_writes: bool,
        #[serde(default)]
        timeout: Option<u64>,
    },
    Codex {
        action: CodexAction,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        base: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
    },
    Human {
        instruction: String,
        #[serde(default)]
        expects: Option<String>,
    },
    Loop {
        until: String,
        max: u32,
        body: Vec<Step>,
    },
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexAction {
    ReviewDoc,
    ReviewMr,
    Ask,
}

impl Manifest {
    pub fn parse(yaml: &str) -> Result<Self, EngineError> {
        serde_yml::from_str(yaml).map_err(|e| EngineError::Parse(e.to_string()))
    }
}
```

- [ ] **Step 5: 定义 EngineError(error.rs)**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("manifest parse error: {0}")]
    Parse(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("cli error: {0}")]
    Cli(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test -p agentpipe-engine manifest`
Expected: PASS(2 个测试)。

- [ ] **Step 7: Commit**

```bash
git add crates/engine/src/manifest.rs crates/engine/src/error.rs crates/engine/tests/manifest_test.rs tests/fixtures/sample-task.yaml
git commit -m "feat(engine): manifest 类型与 YAML 解析"
```

---

## Task 4: 校验(必填字段按 kind 约束)

**Files:**
- Modify: `crates/engine/src/manifest.rs`
- Modify: `crates/engine/tests/manifest_test.rs`

- [ ] **Step 1: 写失败测试**

追加到 `manifest_test.rs`:

```rust
#[test]
fn validate_codex_review_doc_requires_path() {
    let yaml = r#"
version: 1
name: bad
target: /tmp
steps:
  - id: x
    kind: codex
    action: review-doc
"#;
    let m = Manifest::parse(yaml).unwrap();
    let err = m.validate().unwrap_err();
    assert!(err.to_string().contains("review-doc") && err.to_string().contains("path"));
}

#[test]
fn validate_codex_review_mr_requires_base() {
    let yaml = r#"
version: 1
name: bad
target: /tmp
steps:
  - id: x
    kind: codex
    action: review-mr
"#;
    let m = Manifest::parse(yaml).unwrap();
    assert!(m.validate().is_err());
}

#[test]
fn validate_accepts_sample() {
    let yaml = include_str!("../../../tests/fixtures/sample-task.yaml");
    Manifest::parse(yaml).unwrap().validate().expect("sample should be valid");
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p agentpipe-engine validate`
Expected: FAIL(`validate` 未定义)。

- [ ] **Step 3: 实现 validate**

追加到 `manifest.rs` 的 `impl Manifest`:

```rust
    pub fn validate(&self) -> Result<(), EngineError> {
        for step in &self.steps {
            Self::validate_step(step)?;
        }
        Ok(())
    }

    fn validate_step(step: &Step) -> Result<(), EngineError> {
        match &step.kind {
            StepKind::Codex { action, path, base, prompt } => match action {
                CodexAction::ReviewDoc if path.is_none() =>
                    Err(EngineError::Validation(format!(
                        "step '{}': codex review-doc 需要 path 字段", step.id))),
                CodexAction::ReviewMr if base.is_none() =>
                    Err(EngineError::Validation(format!(
                        "step '{}': codex review-mr 需要 base 字段", step.id))),
                CodexAction::Ask if prompt.is_none() =>
                    Err(EngineError::Validation(format!(
                        "step '{}': codex ask 需要 prompt 字段", step.id))),
                _ => Ok(()),
            },
            StepKind::Loop { body, until, .. } => {
                if until != "codex-clean" {
                    return Err(EngineError::Validation(format!(
                        "step '{}': Phase 1 仅支持 until: codex-clean", step.id)));
                }
                for s in body { Self::validate_step(s)?; }
                Ok(())
            }
            _ => Ok(()),
        }
    }
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p agentpipe-engine`
Expected: PASS(全部 manifest 测试)。

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/manifest.rs crates/engine/tests/manifest_test.rs
git commit -m "feat(engine): manifest 按 step kind 校验必填字段"
```

---

## Task 5: 运行上下文与插值

**Files:**
- Modify: `crates/engine/src/context.rs`
- Create: `crates/engine/tests/context_test.rs`

- [ ] **Step 1: 写失败测试**

`crates/engine/tests/context_test.rs`:

```rust
use agentpipe_engine::context::{RunContext, StepOutput, Verdict};
use std::path::PathBuf;

#[test]
fn interpolates_recorded_artifacts() {
    let mut ctx = RunContext::new(PathBuf::from("/tmp/repo"));
    ctx.record("brainstorm", StepOutput { artifact: Some("docs/spec.md".into()), ..Default::default() });
    ctx.record("review-mr", StepOutput {
        findings: Some("两处空指针".into()),
        verdict: Some(Verdict::ChangesRequested),
        ..Default::default()
    });

    let out = ctx.interpolate("审查 {{brainstorm.artifact}};修复 {{review-mr.findings}}");
    assert_eq!(out, "审查 docs/spec.md;修复 两处空指针");
}

#[test]
fn unknown_reference_left_empty() {
    let ctx = RunContext::new(PathBuf::from("/tmp"));
    assert_eq!(ctx.interpolate("x={{nope.artifact}}"), "x=");
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p agentpipe-engine context`
Expected: FAIL。

- [ ] **Step 3: 实现 context.rs**

```rust
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Clean,
    ChangesRequested,
}

#[derive(Debug, Default, Clone)]
pub struct StepOutput {
    pub artifact: Option<String>,
    pub findings: Option<String>,
    pub verdict: Option<Verdict>,
}

impl StepOutput {
    fn field(&self, name: &str) -> Option<String> {
        match name {
            "artifact" => self.artifact.clone(),
            "findings" => self.findings.clone(),
            "verdict" => self.verdict.as_ref().map(|v| match v {
                Verdict::Clean => "clean".into(),
                Verdict::ChangesRequested => "changes_requested".into(),
            }),
            _ => None,
        }
    }
}

pub struct RunContext {
    pub cwd: PathBuf,
    outputs: HashMap<String, StepOutput>,
}

impl RunContext {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd, outputs: HashMap::new() }
    }

    pub fn record(&mut self, step_id: &str, out: StepOutput) {
        self.outputs.insert(step_id.to_string(), out);
    }

    pub fn get(&self, step_id: &str) -> Option<&StepOutput> {
        self.outputs.get(step_id)
    }

    /// 替换所有 {{step-id.field}};未知引用替换为空串。
    pub fn interpolate(&self, template: &str) -> String {
        let mut result = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            result.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            if let Some(end) = after.find("}}") {
                let token = after[..end].trim();
                let value = token
                    .split_once('.')
                    .and_then(|(id, field)| self.get(id).and_then(|o| o.field(field)))
                    .unwrap_or_default();
                result.push_str(&value);
                rest = &after[end + 2..];
            } else {
                result.push_str("{{");
                rest = after;
            }
        }
        result.push_str(rest);
        result
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p agentpipe-engine context`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/context.rs crates/engine/tests/context_test.rs
git commit -m "feat(engine): 运行上下文与 step 间插值"
```

---

## Task 6: 协议类型(事件 / 指令)

**Files:**
- Modify: `crates/engine/src/protocol.rs`

- [ ] **Step 1: 实现 protocol.rs**

(纯类型,直接写实现 + 一个 smoke 测试)

```rust
use crate::context::Verdict;

#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    AwaitingGate,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub enum Event {
    RunStarted { name: String },
    StepStarted { step_id: String, kind: String },
    StepProgress { step_id: String, line: String },
    StepAwaitingGate { step_id: String, suggestion: String, expects_artifact: bool },
    StepFinished { step_id: String, status: StepStatus, summary: String },
    StepFailed { step_id: String, error: String },
    LoopIteration { loop_id: String, iteration: u32 },
    LoopConverged { loop_id: String, iterations: u32 },
    LoopMaxReached { loop_id: String, max: u32 },
    RunFinished { status: RunStatus },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunStatus {
    Success,
    Failed,
    Aborted,
}

#[derive(Debug, Clone)]
pub enum Command {
    ApproveGate { step_id: String, artifact: Option<String> },
    SkipStep { step_id: String },
    Interrupt,
    Resume,
    Abort,
}

// 供 codex runner 复用的结果类型
#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub verdict: Verdict,
    pub findings: String,
}
```

- [ ] **Step 2: 编译验证**

Run: `cargo build -p agentpipe-engine`
Expected: 通过。

- [ ] **Step 3: Commit**

```bash
git add crates/engine/src/protocol.rs
git commit -m "feat(engine): 运行时事件与指令协议类型"
```

---

## Task 7: CodexRunner(stub 替身测试)

**Files:**
- Modify: `crates/engine/src/runner/mod.rs`, `crates/engine/src/runner/codex.rs`
- Create: `crates/engine/tests/codex_runner_test.rs`
- Create: `tests/fixtures/stub-codex.sh`

- [ ] **Step 1: 写 stub codex 脚本**

`tests/fixtures/stub-codex.sh`(模拟 codex:解析出 `-o <file>`,写入预设 JSON;JSON 内容由环境变量 `STUB_VERDICT` 控制):

```bash
#!/usr/bin/env bash
# 解析 -o 后面的输出文件路径
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
verdict="${STUB_VERDICT:-changes_requested}"
cat > "$out" <<EOF
{"verdict":"$verdict","findings":[{"severity":"high","file":"a.rs","line":10,"summary":"示例问题"}]}
EOF
echo "stub codex done"
```

```bash
chmod +x tests/fixtures/stub-codex.sh
```

- [ ] **Step 2: 写失败测试**

`crates/engine/tests/codex_runner_test.rs`:

```rust
use agentpipe_engine::context::Verdict;
use agentpipe_engine::runner::codex::CodexRunner;
use agentpipe_engine::manifest::CodexAction;
use std::path::PathBuf;

fn stub() -> CodexRunner {
    CodexRunner::new("tests/fixtures/stub-codex.sh".into())
}

#[test]
fn parses_changes_requested() {
    std::env::set_var("STUB_VERDICT", "changes_requested");
    let r = stub().review(&CodexAction::ReviewMr, None, Some("dev"), None, &PathBuf::from("."))
        .expect("review ok");
    assert_eq!(r.verdict, Verdict::ChangesRequested);
    assert!(r.findings.contains("示例问题"));
}

#[test]
fn parses_clean() {
    std::env::set_var("STUB_VERDICT", "clean");
    let r = stub().review(&CodexAction::ReviewMr, None, Some("dev"), None, &PathBuf::from("."))
        .unwrap();
    assert_eq!(r.verdict, Verdict::Clean);
}

#[test]
fn unparseable_output_is_changes_requested() {
    // stub 写好 JSON,这里用一个会产出非法 JSON 的 verdict 名校验 fail-closed
    std::env::set_var("STUB_VERDICT", "\"broken");
    let r = stub().review(&CodexAction::ReviewMr, None, Some("dev"), None, &PathBuf::from("."))
        .unwrap();
    assert_eq!(r.verdict, Verdict::ChangesRequested); // fail-closed
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p agentpipe-engine codex_runner`
Expected: FAIL(`CodexRunner` 未定义)。

- [ ] **Step 4: 实现 runner/mod.rs 公共 spawn**

```rust
pub mod claude;
pub mod codex;

use std::path::Path;
use std::process::Command;
use crate::error::EngineError;

/// spawn 一个命令,返回 (stdout, exit_success)。黑盒:不解析协议,只收文本。
pub fn run_command(bin: &str, args: &[String], cwd: &Path) -> Result<(String, bool), EngineError> {
    let output = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| EngineError::Cli(format!("spawn {bin} 失败: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok((stdout, output.status.success()))
}
```

- [ ] **Step 5: 实现 runner/codex.rs**

```rust
use std::path::Path;
use serde::Deserialize;
use crate::context::Verdict;
use crate::error::EngineError;
use crate::manifest::CodexAction;
use crate::protocol::ReviewResult;
use super::run_command;

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
    #[serde(default)] severity: String,
    #[serde(default)] file: String,
    #[serde(default)] line: i64,
    #[serde(default)] summary: String,
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
        let out_file = std::env::temp_dir().join(format!("agentpipe-codex-{}.json", std::process::id()));
        let out_str = out_file.to_string_lossy().to_string();
        let schema = write_schema()?;

        let args: Vec<String> = match action {
            CodexAction::ReviewMr => vec![
                "exec".into(), "review".into(),
                "--base".into(), base.unwrap_or("dev").into(),
                "--output-schema".into(), schema.clone(),
                "-o".into(), out_str.clone(),
                "-s".into(), "read-only".into(),
            ],
            CodexAction::ReviewDoc => vec![
                "exec".into(), "-s".into(), "read-only".into(),
                "--output-schema".into(), schema.clone(),
                "-o".into(), out_str.clone(),
                format!("审查设计文档 {} 并按 schema 输出结论", doc_path.unwrap_or("")),
            ],
            CodexAction::Ask => vec![
                "exec".into(), "-s".into(), "read-only".into(),
                "-o".into(), out_str.clone(),
                ask_prompt.unwrap_or("").into(),
            ],
        };

        run_command(&self.bin, &args, cwd)?;
        Ok(parse_review(&out_file))
    }
}

fn write_schema() -> Result<String, EngineError> {
    let path = std::env::temp_dir().join("agentpipe-review-schema.json");
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
    let verdict = if raw.verdict == "clean" { Verdict::Clean } else { Verdict::ChangesRequested };
    let findings = raw.findings.iter()
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
```

- [ ] **Step 6: 运行确认通过**

Run: `cargo test -p agentpipe-engine codex_runner`
Expected: PASS(3 个测试,含 fail-closed)。

- [ ] **Step 7: Commit**

```bash
git add crates/engine/src/runner/ crates/engine/tests/codex_runner_test.rs tests/fixtures/stub-codex.sh
git commit -m "feat(engine): CodexRunner + 结构化 verdict 解析(fail-closed)"
```

---

## Task 8: ClaudeRunner(stub 替身测试)

**Files:**
- Modify: `crates/engine/src/runner/claude.rs`
- Create: `crates/engine/tests/claude_runner_test.rs`
- Create: `tests/fixtures/stub-claude.sh`

- [ ] **Step 1: 写 stub claude 脚本**

`tests/fixtures/stub-claude.sh`(回显最后一个参数当作 prompt,输出固定结果文本):

```bash
#!/usr/bin/env bash
last="${@: -1}"
echo "STUB CLAUDE 收到: $last"
echo "https://gitlab.example.com/mr/42"
```

```bash
chmod +x tests/fixtures/stub-claude.sh
```

- [ ] **Step 2: 写失败测试**

`crates/engine/tests/claude_runner_test.rs`:

```rust
use agentpipe_engine::runner::claude::ClaudeRunner;
use std::path::PathBuf;

#[test]
fn runs_and_captures_last_line_as_artifact() {
    let r = ClaudeRunner::new("tests/fixtures/stub-claude.sh".into());
    let out = r.run("实现功能", None, false, &PathBuf::from(".")).expect("ok");
    assert!(out.full_output.contains("STUB CLAUDE 收到: 实现功能"));
    assert_eq!(out.last_line.trim(), "https://gitlab.example.com/mr/42");
}

#[test]
fn skill_prefixes_prompt() {
    let r = ClaudeRunner::new("tests/fixtures/stub-claude.sh".into());
    let out = r.run("审查", Some("four-dimension-review"), false, &PathBuf::from(".")).unwrap();
    assert!(out.full_output.contains("/four-dimension-review"));
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p agentpipe-engine claude_runner`
Expected: FAIL。

- [ ] **Step 4: 实现 runner/claude.rs**

```rust
use std::path::Path;
use crate::error::EngineError;
use super::run_command;

pub struct ClaudeRunner {
    bin: String,
}

pub struct ClaudeOutcome {
    pub full_output: String,
    pub last_line: String,
}

impl ClaudeRunner {
    pub fn new(bin: String) -> Self {
        Self { bin }
    }

    /// allow_writes 决定权限姿态(确切 flag 以 Task 1 实测为准,这里给默认形态)。
    pub fn run(
        &self,
        prompt: &str,
        skill: Option<&str>,
        allow_writes: bool,
        cwd: &Path,
    ) -> Result<ClaudeOutcome, EngineError> {
        let full_prompt = match skill {
            Some(s) => format!("/{s} {prompt}"),
            None => prompt.to_string(),
        };
        let mut args = vec!["-p".to_string(), full_prompt];
        if allow_writes {
            // 占位:Task 1 实测确认确切 flag 后替换
            args.push("--permission-mode".into());
            args.push("acceptEdits".into());
        }
        let (stdout, success) = run_command(&self.bin, &args, cwd)?;
        if !success {
            return Err(EngineError::Cli("claude 非零退出".into()));
        }
        let last_line = stdout.lines().rev().find(|l| !l.trim().is_empty())
            .unwrap_or("").to_string();
        Ok(ClaudeOutcome { full_output: stdout, last_line })
    }
}
```

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p agentpipe-engine claude_runner`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/engine/src/runner/claude.rs crates/engine/tests/claude_runner_test.rs tests/fixtures/stub-claude.sh
git commit -m "feat(engine): ClaudeRunner(skill 前缀 + 结果取最后一行)"
```

---

## Task 9: Executor 串行执行(claude / codex / human)

**Files:**
- Modify: `crates/engine/src/executor.rs`
- Create: `crates/engine/tests/executor_test.rs`

执行器用注入的 bin 路径构造两个 runner(测试传 stub),事件经 `mpsc::Sender<Event>` 输出,gate 经 `mpsc::Receiver<Command>` 阻塞等待。

- [ ] **Step 1: 写失败测试(auto 模式,无 gate)**

`crates/engine/tests/executor_test.rs`:

```rust
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Event, Command, RunStatus};
use std::sync::mpsc;

fn stub_bins() -> RunnerBins {
    RunnerBins {
        claude: "tests/fixtures/stub-claude.sh".into(),
        codex: "tests/fixtures/stub-codex.sh".into(),
    }
}

#[test]
fn runs_simple_codex_then_claude_in_auto_mode() {
    std::env::set_var("STUB_VERDICT", "clean");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: rev
    kind: codex
    action: review-mr
    base: dev
  - id: fix
    kind: claude
    prompt: "用 {{rev.findings}}"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_ctx, crx) = mpsc::channel::<Command>();

    let mut ex = Executor::new(m, stub_bins(), etx, crx);
    let status = ex.run();

    assert_eq!(status, RunStatus::Success);
    let events: Vec<Event> = erx.try_iter().collect();
    // 至少出现两个 StepStarted 和一个 RunFinished
    let started = events.iter().filter(|e| matches!(e, Event::StepStarted{..})).count();
    assert_eq!(started, 2);
    assert!(events.iter().any(|e| matches!(e, Event::RunFinished{ status: RunStatus::Success })));
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p agentpipe-engine executor`
Expected: FAIL。

- [ ] **Step 3: 实现 executor.rs(先不含 loop / step 模式 / human gate,占位返回)**

```rust
use std::sync::mpsc::{Receiver, Sender};
use crate::context::{RunContext, StepOutput};
use crate::manifest::{CodexAction, Manifest, Step, StepKind, RunMode};
use crate::protocol::{Command, Event, RunStatus, StepStatus};
use crate::runner::claude::ClaudeRunner;
use crate::runner::codex::CodexRunner;

pub struct RunnerBins {
    pub claude: String,
    pub codex: String,
}

pub struct Executor {
    manifest: Manifest,
    ctx: RunContext,
    claude: ClaudeRunner,
    codex: CodexRunner,
    events: Sender<Event>,
    commands: Receiver<Command>,
}

impl Executor {
    pub fn new(manifest: Manifest, bins: RunnerBins, events: Sender<Event>, commands: Receiver<Command>) -> Self {
        let ctx = RunContext::new(manifest.target.clone());
        Self {
            claude: ClaudeRunner::new(bins.claude),
            codex: CodexRunner::new(bins.codex),
            manifest, ctx, events, commands,
        }
    }

    pub fn run(&mut self) -> RunStatus {
        let _ = self.events.send(Event::RunStarted { name: self.manifest.name.clone() });
        let steps = std::mem::take(&mut self.manifest.steps);
        for step in &steps {
            match self.run_step(step) {
                Ok(_) => {}
                Err(_) => {
                    let _ = self.events.send(Event::RunFinished { status: RunStatus::Failed });
                    return RunStatus::Failed;
                }
            }
        }
        let _ = self.events.send(Event::RunFinished { status: RunStatus::Success });
        RunStatus::Success
    }

    fn run_step(&mut self, step: &Step) -> Result<(), ()> {
        let kind_name = match &step.kind {
            StepKind::Claude { .. } => "claude",
            StepKind::Codex { .. } => "codex",
            StepKind::Human { .. } => "human",
            StepKind::Loop { .. } => "loop",
        };
        let _ = self.events.send(Event::StepStarted { step_id: step.id.clone(), kind: kind_name.into() });

        match &step.kind {
            StepKind::Codex { action, path, base, prompt } => {
                let out = self.codex.review(
                    action, path.as_deref(), base.as_deref(), prompt.as_deref(), &self.ctx.cwd,
                ).map_err(|e| self.fail(&step.id, e.to_string()))?;
                let summary = format!("verdict={:?}", out.verdict);
                self.ctx.record(&step.id, StepOutput {
                    findings: Some(out.findings), verdict: Some(out.verdict), ..Default::default()
                });
                self.finish(&step.id, summary);
                Ok(())
            }
            StepKind::Claude { prompt, skill, allow_writes, .. } => {
                let p = self.ctx.interpolate(prompt);
                let out = self.claude.run(&p, skill.as_deref(), *allow_writes, &self.ctx.cwd)
                    .map_err(|e| self.fail(&step.id, e.to_string()))?;
                self.ctx.record(&step.id, StepOutput {
                    artifact: Some(out.last_line), ..Default::default()
                });
                self.finish(&step.id, "done".into());
                Ok(())
            }
            StepKind::Human { instruction, expects } => {
                self.run_human(step, instruction, expects.is_some())
            }
            StepKind::Loop { .. } => {
                // Task 10 实现
                self.finish(&step.id, "loop(占位)".into());
                Ok(())
            }
        }
    }

    fn run_human(&mut self, step: &Step, instruction: &str, expects_artifact: bool) -> Result<(), ()> {
        let _ = self.events.send(Event::StepAwaitingGate {
            step_id: step.id.clone(),
            suggestion: instruction.to_string(),
            expects_artifact,
        });
        match self.commands.recv() {
            Ok(Command::ApproveGate { artifact, .. }) => {
                self.ctx.record(&step.id, StepOutput { artifact, ..Default::default() });
                self.finish(&step.id, "approved".into());
                Ok(())
            }
            Ok(Command::SkipStep { .. }) => {
                let _ = self.events.send(Event::StepFinished {
                    step_id: step.id.clone(), status: StepStatus::Skipped, summary: "skipped".into(),
                });
                Ok(())
            }
            _ => Err(self.fail(&step.id, "aborted".into())),
        }
    }

    fn finish(&self, step_id: &str, summary: String) {
        let _ = self.events.send(Event::StepFinished {
            step_id: step_id.to_string(), status: StepStatus::Done, summary,
        });
    }

    fn fail(&self, step_id: &str, error: String) {
        let _ = self.events.send(Event::StepFailed { step_id: step_id.to_string(), error });
    }

    // 供 loop 复用:auto 模式下子步骤直接跑
    fn mode(&self) -> &RunMode { &self.manifest.mode }
    // 便于 Task 11 引用
    #[allow(dead_code)]
    fn codex_action_is_mr(a: &CodexAction) -> bool { matches!(a, CodexAction::ReviewMr) }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p agentpipe-engine executor`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/executor.rs crates/engine/tests/executor_test.rs
git commit -m "feat(engine): Executor 串行执行 codex/claude/human"
```

---

## Task 10: loop 执行 + codex-clean 收敛

**Files:**
- Modify: `crates/engine/src/executor.rs`
- Modify: `crates/engine/tests/executor_test.rs`

- [ ] **Step 1: 写失败测试(收敛 + 到上限两种)**

追加到 `executor_test.rs`:

```rust
use agentpipe_engine::protocol::Event::{LoopConverged, LoopMaxReached};

#[test]
fn loop_converges_when_codex_clean() {
    std::env::set_var("STUB_VERDICT", "clean"); // 第一轮就 clean
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: fixloop
    kind: loop
    until: codex-clean
    max: 3
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: dev
      - id: fix
        kind: claude
        prompt: "修 {{rev.findings}}"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = std::sync::mpsc::channel();
    let (_c, crx) = std::sync::mpsc::channel();
    let mut ex = Executor::new(m, stub_bins(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(e, LoopConverged{ iterations: 1, .. })));
}

#[test]
fn loop_hits_max_when_never_clean() {
    std::env::set_var("STUB_VERDICT", "changes_requested");
    let yaml = r#"
version: 1
name: t
target: .
mode: auto
steps:
  - id: fixloop
    kind: loop
    until: codex-clean
    max: 2
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: dev
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = std::sync::mpsc::channel();
    let (_c, crx) = std::sync::mpsc::channel();
    let mut ex = Executor::new(m, stub_bins(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(e, LoopMaxReached{ max: 2, .. })));
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p agentpipe-engine executor`
Expected: FAIL(loop 仍是占位)。

- [ ] **Step 3: 实现 loop**

替换 executor.rs 里 `StepKind::Loop` 占位分支:

```rust
            StepKind::Loop { until, max, body } => {
                self.run_loop(&step.id, until, *max, body)
            }
```

新增方法到 `impl Executor`:

```rust
    fn run_loop(&mut self, loop_id: &str, until: &str, max: u32, body: &[Step]) -> Result<(), ()> {
        for n in 1..=max {
            let _ = self.events.send(Event::LoopIteration { loop_id: loop_id.into(), iteration: n });
            for sub in body {
                self.run_step(sub)?;
            }
            if self.eval_until(until, body) {
                let _ = self.events.send(Event::LoopConverged { loop_id: loop_id.into(), iterations: n });
                return Ok(());
            }
        }
        let _ = self.events.send(Event::LoopMaxReached { loop_id: loop_id.into(), max });
        Ok(())
    }

    /// 目前只支持 codex-clean:找 body 里最后一个 codex step 的 verdict。
    fn eval_until(&self, until: &str, body: &[Step]) -> bool {
        if until != "codex-clean" { return false; }
        for sub in body.iter().rev() {
            if matches!(sub.kind, StepKind::Codex { .. }) {
                if let Some(out) = self.ctx.get(&sub.id) {
                    return matches!(out.verdict, Some(crate::context::Verdict::Clean));
                }
            }
        }
        false // 没找到 codex step → fail-closed 不收敛
    }
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p agentpipe-engine`
Expected: PASS(全部 engine 测试)。

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/executor.rs crates/engine/tests/executor_test.rs
git commit -m "feat(engine): loop 执行 + codex-clean 收敛(fail-closed)"
```

---

## Task 11: step 模式门控

**Files:**
- Modify: `crates/engine/src/executor.rs`
- Modify: `crates/engine/tests/executor_test.rs`

`mode: step` 时,每个 step 执行前先发 `StepAwaitingGate` 等批准;收到 `SkipStep` 则跳过。

- [ ] **Step 1: 写失败测试**

追加到 `executor_test.rs`:

```rust
#[test]
fn step_mode_waits_for_approval_each_step() {
    std::env::set_var("STUB_VERDICT", "clean");
    let yaml = r#"
version: 1
name: t
target: .
mode: step
steps:
  - id: rev
    kind: codex
    action: review-mr
    base: dev
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = std::sync::mpsc::channel();
    let (ctx_tx, crx) = std::sync::mpsc::channel();
    // 预先放一个批准指令
    ctx_tx.send(agentpipe_engine::protocol::Command::ApproveGate {
        step_id: "rev".into(), artifact: None,
    }).unwrap();
    let mut ex = Executor::new(m, stub_bins(), etx, crx);
    ex.run();
    let events: Vec<_> = erx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(e,
        agentpipe_engine::protocol::Event::StepAwaitingGate{ step_id, .. } if step_id == "rev")));
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p agentpipe-engine step_mode`
Expected: FAIL。

- [ ] **Step 3: 实现门控**

在 `run_step` 开头(发 `StepStarted` 之前)加入。注意 loop 的 body 子步骤不重复门控(顶层门控即可),用一个参数区分:

```rust
    fn run_step(&mut self, step: &Step) -> Result<(), ()> {
        if matches!(self.manifest.mode, RunMode::Step) && !matches!(step.kind, StepKind::Loop{..}) {
            let _ = self.events.send(Event::StepAwaitingGate {
                step_id: step.id.clone(),
                suggestion: format!("即将执行 step '{}'", step.id),
                expects_artifact: false,
            });
            match self.commands.recv() {
                Ok(Command::ApproveGate { .. }) => {}
                Ok(Command::SkipStep { .. }) => {
                    let _ = self.events.send(Event::StepFinished {
                        step_id: step.id.clone(), status: StepStatus::Skipped, summary: "skipped".into(),
                    });
                    return Ok(());
                }
                _ => return Err(self.fail(&step.id, "aborted".into())),
            }
        }
        // ... 原有 StepStarted + match step.kind 逻辑不变
```

(human step 自身已有 gate,step 模式下它会先过这道顶层门控再进入 human 等待;两次等待对 human 是合理的——先批准开始,再提交产物。)

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p agentpipe-engine`
Expected: PASS(全部)。

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/executor.rs crates/engine/tests/executor_test.rs
git commit -m "feat(engine): step 模式逐步门控"
```

---

## Task 12: CLI 驱动(agentpipe run)

**Files:**
- Modify: `crates/cli/src/main.rs`
- Modify: `crates/cli/Cargo.toml`

CLI 在独立线程跑 Executor,主线程消费事件打印;遇 `StepAwaitingGate` 从 stdin 读 `y`(批准,可带产物)/ `s`(跳过)。claude/codex 用真实 bin(`claude` / `codex`),也支持 env 覆盖成 stub。

- [ ] **Step 1: cli Cargo.toml 加依赖**

```toml
[dependencies]
agentpipe-engine = { path = "../engine" }
```

(不引入额外 crate;用 std。)

- [ ] **Step 2: 实现 main.rs**

```rust
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::thread;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, StepStatus};

fn main() {
    let path = match std::env::args().nth(2) {
        // 用法:agentpipe run <task.yaml>
        Some(p) if std::env::args().nth(1).as_deref() == Some("run") => p,
        _ => { eprintln!("用法: agentpipe run <task.yaml>"); std::process::exit(2); }
    };

    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("读取 {path} 失败: {e}"); std::process::exit(1);
    });
    let manifest = match Manifest::parse(&yaml).and_then(|m| { m.validate()?; Ok(m) }) {
        Ok(m) => m,
        Err(e) => { eprintln!("manifest 错误: {e}"); std::process::exit(1); }
    };

    let bins = RunnerBins {
        claude: std::env::var("AGENTPIPE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        codex: std::env::var("AGENTPIPE_CODEX_BIN").unwrap_or_else(|_| "codex".into()),
    };

    let (etx, erx) = mpsc::channel::<Event>();
    let (ctx, crx) = mpsc::channel::<Command>();

    let handle = thread::spawn(move || {
        let mut ex = Executor::new(manifest, bins, etx, crx);
        ex.run()
    });

    for event in erx {
        match event {
            Event::RunStarted { name } => println!("▶ Run: {name}"),
            Event::StepStarted { step_id, kind } => println!("  ▷ [{kind}] {step_id}"),
            Event::StepProgress { line, .. } => println!("    {line}"),
            Event::StepFinished { step_id, status, summary } => {
                let mark = if matches!(status, StepStatus::Skipped) { "⏭" } else { "✓" };
                println!("  {mark} {step_id}: {summary}");
            }
            Event::StepFailed { step_id, error } => println!("  ✗ {step_id}: {error}"),
            Event::LoopIteration { loop_id, iteration } => println!("  ↻ {loop_id} 第 {iteration} 轮"),
            Event::LoopConverged { loop_id, iterations } => println!("  ✓ {loop_id} {iterations} 轮收敛"),
            Event::LoopMaxReached { loop_id, max } => println!("  ⚠ {loop_id} 到上限 {max} 仍未干净"),
            Event::StepAwaitingGate { step_id, suggestion, expects_artifact } => {
                println!("  ⏸ {step_id}: {suggestion}");
                let cmd = prompt_gate(&step_id, expects_artifact);
                let _ = ctx.send(cmd);
            }
            Event::RunFinished { status } => { println!("■ 结束: {status:?}"); break; }
        }
    }

    let _ = handle.join();
}

fn prompt_gate(step_id: &str, expects_artifact: bool) -> Command {
    let hint = if expects_artifact { "[y <产物> / s 跳过]" } else { "[y 批准 / s 跳过]" };
    print!("    > {hint} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok();
    let line = line.trim();
    if line.starts_with('s') {
        Command::SkipStep { step_id: step_id.to_string() }
    } else {
        let artifact = line.strip_prefix("y ").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Command::ApproveGate { step_id: step_id.to_string(), artifact }
    }
}
```

- [ ] **Step 3: 用 stub 端到端 smoke**

```bash
cargo build
AGENTPIPE_CLAUDE_BIN=$PWD/tests/fixtures/stub-claude.sh \
AGENTPIPE_CODEX_BIN=$PWD/tests/fixtures/stub-codex.sh \
STUB_VERDICT=clean \
./target/debug/agentpipe run tests/fixtures/sample-task.yaml
```

Expected: 打印 Run demo → review-doc ✓ → fix-loop 第 1 轮 → review-mr verdict=Clean → 收敛 → 结束 Success。(sample-task.yaml 的 target 是 /tmp/demo-repo,smoke 前先 `mkdir -p /tmp/demo-repo`。)

- [ ] **Step 4: Commit**

```bash
git add crates/cli/
git commit -m "feat(cli): agentpipe run 驱动引擎 + stdin gate"
```

---

## Task 13: 内置模板 + README

**Files:**
- Create: `templates/full-pipeline.yaml`, `templates/codex-gate-only.yaml`
- Create: `README.md`

- [ ] **Step 1: full-pipeline 模板**

`templates/full-pipeline.yaml`(对应 spec 4.2 的完整 8 步,target 用占位注释):

```yaml
version: 1
name: "<任务名>"
target: <目标仓库绝对路径>
mode: step
steps:
  - id: brainstorm
    kind: human
    instruction: "在 Claude Code 跑 brainstorming,产出设计 spec"
    expects: artifact
  - id: design-review-claude
    kind: claude
    skill: four-dimension-review
    prompt: "审查设计文档 {{brainstorm.artifact}}"
  - id: design-review-codex
    kind: codex
    action: review-doc
    path: "{{brainstorm.artifact}}"
  - id: plan
    kind: human
    instruction: "在 Claude Code 跑 writing-plans"
    expects: artifact
  - id: implement
    kind: claude
    allow_writes: true
    prompt: "按 {{plan.artifact}} 实现,提交,建 MR,最后只输出 MR URL"
  - id: self-review
    kind: human
    instruction: "code-review + simplify,提交到 MR"
  - id: codex-loop
    kind: loop
    until: codex-clean
    max: 5
    body:
      - id: codex-review-mr
        kind: codex
        action: review-mr
        base: dev
      - id: apply-feedback
        kind: claude
        allow_writes: true
        prompt: "按 Codex 反馈修改并提交:{{codex-review-mr.findings}}"
```

- [ ] **Step 2: codex-gate-only 模板**

`templates/codex-gate-only.yaml`(只跑代码闸门循环,用于已有 MR 的二次审):

```yaml
version: 1
name: "<已有改动的 Codex 审查>"
target: <目标仓库绝对路径>
mode: auto
steps:
  - id: codex-loop
    kind: loop
    until: codex-clean
    max: 5
    body:
      - id: review
        kind: codex
        action: review-mr
        base: dev
      - id: fix
        kind: claude
        allow_writes: true
        prompt: "按 Codex 反馈修改并提交:{{review.findings}}"
```

- [ ] **Step 3: README**

`README.md`:写明定位(个人编排客户端,Phase 1a 为引擎+CLI)、构建(`cargo build`)、用法(`agentpipe run <task.yaml>`,env 覆盖 bin)、模板说明、指向 `docs/specs/2026-06-16-design.md`。

- [ ] **Step 4: Commit**

```bash
git add templates/ README.md
git commit -m "docs: 内置 manifest 模板 + README"
```

---

## Task 14: 全量回归 + 收口

- [ ] **Step 1: 跑全部测试**

Run: `cargo test`
Expected: 全绿。

- [ ] **Step 2: clippy 收口**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 无 warning(有则修)。

- [ ] **Step 3: 用 stub 跑两套模板 smoke**

```bash
mkdir -p /tmp/demo-repo
AGENTPIPE_CLAUDE_BIN=$PWD/tests/fixtures/stub-claude.sh \
AGENTPIPE_CODEX_BIN=$PWD/tests/fixtures/stub-codex.sh \
STUB_VERDICT=clean \
./target/debug/agentpipe run tests/fixtures/sample-task.yaml
```

Expected: 走完不报错。

- [ ] **Step 4: Commit(若有修)**

```bash
git add -A
git commit -m "chore: Phase 1a 收口(clippy + 回归)"
```

---

## 后续(不在本计划)

- Phase 1b:Tauri 壳 + 运行时协议桥接 + React UI(编排页 + 运行控制台)。单独成计划。
- 真实 CLI 联调:把 stub 换成真 claude/codex,按 Task 1 findings 校准 ClaudeRunner 的权限 flag、CodexRunner 的 schema 行为。
- StepProgress 实时流式(当前黑盒收尾输出,后续改 BufRead 行级转发)。
