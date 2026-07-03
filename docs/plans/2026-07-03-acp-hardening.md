# ACP 二等公民收编（Phase A）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 [docs/specs/2026-07-03-acp-hardening-design.md](../specs/2026-07-03-acp-hardening-design.md) 落地 D1-D5：ACP step 在 budget、verify、权限、registry、审计可读性上与 claude / codex 同等公民。

**Architecture:** 全部改动在 crates/engine（manifest 校验 / executor 公共 verify-retry helper / acp runner 权限通道 / 新增 paths + agents 模块）+ crates/cli 与 src-tauri 的薄接线。不动协议 Event schema，不动 claude / codex runner。

**Tech Stack:** Rust（同步引擎 + acp runner 内 current-thread tokio）、serde_yml、toml、agent-client-protocol 1.x（schema v1）。

## Global Constraints

- 每个 task 收尾必须绿：`cargo test --workspace` 全过 + `cargo clippy --workspace --all-targets -- -D warnings` 零告警。
- serde 向后兼容：新字段一律 `#[serde(default)]`（bool 加 `skip_serializing_if = "is_false"`，Option 加 `skip_serializing_if = "Option::is_none"`），既有 YAML / 模板 / NDJSON 不破。
- fail-closed：解析失败 / 缺 verdict / 未知值一律走最保守分支，绝不静默放行。
- 报错可解释：Validation 错误必须带 step id + 修复建议（沿用 `require_non_empty` 的文案风格）。
- 提交信息用中文，一个 task 一个 commit。
- ACP 测试沿用 `crates/engine/examples/mock_acp_agent.rs` fixture + `MOCK_ACP_SCENARIO` env 模式（见 `crates/engine/tests/acp_runner.rs` 的 `run_scenario_full`）。

---

### Task 1: D1 budget 硬拦（allow_unmetered）

**Files:**
- Modify: `crates/engine/src/manifest.rs`（Manifest 字段 + validate + 测试）

**Interfaces:**
- Produces: `Manifest.allow_unmetered: bool`（后续 watch spec 复用此字段名）；validate 规则"budget_usd Some 且含 acp step 且未 ack → Err"。

- [ ] **Step 1: 写失败测试**（`manifest.rs` 底部 `mod tests` 内追加）

```rust
#[test]
fn budget_with_acp_step_rejected_without_ack() {
    let y = "version: 1\nname: t\ntarget: /tmp\nbudget_usd: 5.0\nsteps:\n  - id: a\n    kind: acp\n    agent: g\n    command: gemini --acp\n    prompt: hi\n";
    let err = Manifest::parse(y).unwrap().validate().unwrap_err().to_string();
    assert!(err.contains("a"), "错误必须点名 acp step id: {err}");
    assert!(err.contains("allow_unmetered"), "错误必须给出 ack 出路: {err}");
}

#[test]
fn budget_with_acp_step_allowed_with_ack() {
    let y = "version: 1\nname: t\ntarget: /tmp\nbudget_usd: 5.0\nallow_unmetered: true\nsteps:\n  - id: a\n    kind: acp\n    agent: g\n    command: gemini --acp\n    prompt: hi\n";
    assert!(Manifest::parse(y).unwrap().validate().is_ok());
}

#[test]
fn budget_without_acp_unaffected() {
    let y = "version: 1\nname: t\ntarget: /tmp\nbudget_usd: 5.0\nsteps:\n  - id: c\n    kind: claude\n    prompt: hi\n";
    assert!(Manifest::parse(y).unwrap().validate().is_ok());
}

#[test]
fn acp_without_budget_unaffected() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: g\n    command: c\n    prompt: p\n";
    assert!(Manifest::parse(y).unwrap().validate().is_ok());
}

#[test]
fn budget_detects_acp_inside_loop_body() {
    let y = "version: 1\nname: t\ntarget: /tmp\nbudget_usd: 5.0\nsteps:\n  - id: l\n    kind: loop\n    until: codex-clean\n    max: 2\n    body:\n      - id: r\n        kind: codex\n        action: review-mr\n        base: main\n      - id: inner\n        kind: acp\n        agent: g\n        command: c\n        prompt: p\n";
    let err = Manifest::parse(y).unwrap().validate().unwrap_err().to_string();
    assert!(err.contains("inner"), "loop body 内的 acp 必须被递归发现: {err}");
}
```

- [ ] **Step 2: 跑测确认失败**

Run: `cargo test -p agentpipe-engine budget_with_acp -- --nocapture`
Expected: FAIL（`budget_with_acp_step_rejected_without_ack` 断言失败——当前 validate 放行；`allow_unmetered` 字段不存在则是编译错，先做 Step 3 的字段部分再回来跑）

- [ ] **Step 3: 实现**

`Manifest` 结构体在 `budget_usd` 字段后加：

```rust
/// 显式确认:budget_usd 与不计费 step(acp,metrics 恒 None)并存。缺省 false =
/// validate 拒绝该组合(fail-closed)。见 acp-hardening spec D1。
#[serde(default, skip_serializing_if = "is_false")]
pub allow_unmetered: bool,
```

`validate()` 里 budget_usd 数值检查之后、`for step in &self.steps` 之前加：

```rust
if self.budget_usd.is_some() && !self.allow_unmetered {
    let mut acp_ids = Vec::new();
    Self::collect_acp_ids(&self.steps, &mut acp_ids);
    if !acp_ids.is_empty() {
        return Err(EngineError::Validation(format!(
            "budget_usd 已设置,但 step [{}] 是 acp 步骤,当前不上报 cost、不计入 budget(预算对其无效)。两条出路:去掉 budget_usd,或在 manifest 顶层显式声明 allow_unmetered: true",
            acp_ids.join(", ")
        )));
    }
}
```

impl Manifest 内加私有函数（与 `validate_step` 并列）：

```rust
/// 递归收集 acp step id(含 loop body),供 D1 budget 硬拦报错点名。
fn collect_acp_ids(steps: &[Step], out: &mut Vec<String>) {
    for s in steps {
        match &s.kind {
            StepKind::Acp { .. } => out.push(s.id.clone()),
            StepKind::Loop { body, .. } => Self::collect_acp_ids(body, out),
            _ => {}
        }
    }
}
```

- [ ] **Step 4: 全量验证**

Run: `cargo build --workspace`（若有 Manifest 结构体字面量构造点报缺字段，补 `allow_unmetered: false`）
Run: `cargo test --workspace` → PASS；`cargo clippy --workspace --all-targets -- -D warnings` → 零告警

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/manifest.rs
git commit -m "feat(engine): D1 budget 硬拦 — budget_usd 与 acp step 并存需显式 allow_unmetered"
```

---

### Task 2: 抽公共 verify-retry helper（行为不变重构）

**Files:**
- Modify: `crates/engine/src/executor.rs`（220-340 行的 Claude match arm 主体外提）

**Interfaces:**
- Produces: `enum VerifiedWork<'a> { Claude { prompt: &'a str, skill: Option<&'a str> }, Acp { agent: &'a str, command: &'a str, prompt: &'a str } }`（executor.rs 私有）；`fn run_verified_step(&mut self, step_id: &str, work: VerifiedWork<'_>, verify: Option<&Verify>) -> Result<(), ()>`。Task 3 依赖这两个符号。
- Consumes: 既有 `handle_failure` / `charge` / `check_budget` / `verify_once` / `decision_gate` / `emit_answer_preview` / `progress_sink` / `StepMetrics::sum`。

- [ ] **Step 1: 基线**

Run: `cargo test --workspace`
Expected: 全绿（重构守护基线，记录测试数量）

- [ ] **Step 2: 实现 helper**

executor.rs 中 `enum StepDecision` 下方加：

```rust
/// 干活步骤的种类:verify-retry 循环内"跑一次 attempt"的分派参数。
/// review §A finding #7 的差异体现在 Err 分支:Acp 的 runner-Err 不进决策门
/// (metrics 恒 None,budget 兜不住重试烧钱),失败 = fail + request_abort。
enum VerifiedWork<'a> {
    Claude { prompt: &'a str, skill: Option<&'a str> },
    Acp { agent: &'a str, command: &'a str, prompt: &'a str },
}
```

impl Executor 内新增方法（内容 = 现 Claude arm 220-340 行整体外提，差异点见注释）：

```rust
/// 公共 verify-retry 循环:跑一次 attempt → charge → (可选)verify → 未达标带反馈重试。
/// 从 Claude match arm 外提(acp-hardening spec D2),Claude / Acp 共用。
fn run_verified_step(
    &mut self,
    step_id: &str,
    work: VerifiedWork<'_>,
    verify: Option<&Verify>,
) -> Result<(), ()> {
    let mut on_line = self.progress_sink(step_id);
    let mut attempt = 0u32; // 校验重试计数(与失败重试独立)
    let mut feedback: Option<String> = None;
    let mut step_metrics: Option<StepMetrics> = None;
    let done_label = match &work {
        VerifiedWork::Claude { .. } => "done",
        VerifiedWork::Acp { .. } => "done · acp",
    };
    loop {
        let attempt_result = {
            let mut p = match &work {
                VerifiedWork::Claude { prompt, .. } => self.ctx.interpolate(prompt),
                VerifiedWork::Acp { prompt, .. } => self.ctx.interpolate(prompt),
            };
            if let Some(f) = &feedback {
                p.push_str(&format!("\n\n上一轮校验反馈:\n{f}\n请据此修正后重做。"));
            }
            match &work {
                VerifiedWork::Claude { skill, .. } => self
                    .claude
                    .run(&p, *skill, Some(self.control.as_ref()), &mut on_line, &self.ctx.cwd, false)
                    .map(|out| (out.answer, out.metrics))
                    .map_err(|e| e.to_string()),
                VerifiedWork::Acp { agent, command, .. } => {
                    let runner = crate::runner::acp::AcpRunner::new(crate::runner::acp::AcpConfig {
                        agent: (*agent).to_string(),
                        command: (*command).to_string(),
                    });
                    runner
                        .run(&p, Some(self.control.as_ref()), &mut on_line, &self.ctx.cwd)
                        .map(|out| (out.answer, out.metrics))
                        .map_err(|e| e.to_string())
                }
            }
        };
        let (answer, metrics) = match attempt_result {
            Ok(v) => v,
            Err(e) => match &work {
                VerifiedWork::Claude { .. } => match self.handle_failure(step_id, e) {
                    StepDecision::Retry => continue,
                    StepDecision::Skip => {
                        self.emit_skipped(step_id);
                        return Ok(());
                    }
                    StepDecision::Abort => return Err(()),
                },
                VerifiedWork::Acp { .. } => {
                    self.fail(step_id, e);
                    self.control.request_abort();
                    return Err(());
                }
            },
        };
        self.charge(&metrics);
        step_metrics = StepMetrics::sum(step_metrics, metrics);
        self.check_budget(step_id, &step_metrics)?;
        self.ctx.record(step_id, StepOutput {
            artifact: Some(answer.clone()),
            ..Default::default()
        });
        emit_answer_preview(&answer, &mut on_line);

        let v = match verify {
            None => {
                self.finish(step_id, done_label.into(), step_metrics);
                return Ok(());
            }
            Some(v) => v,
        };
        on_line("校验中…", None);
        let (verdict, findings, verifier_metrics) = self.verify_once(v, &mut on_line);
        self.charge(&verifier_metrics);
        step_metrics = StepMetrics::sum(step_metrics, verifier_metrics);
        self.check_budget(step_id, &step_metrics)?;
        self.ctx.record(step_id, StepOutput {
            artifact: Some(answer.clone()),
            findings: Some(findings.clone()),
            ..Default::default()
        });
        if matches!(verdict, Verdict::Clean) {
            on_line("校验通过", None);
            self.finish(step_id, format!("{done_label} · 已校验"), step_metrics);
            return Ok(());
        }
        if attempt < v.max_retries {
            attempt += 1;
            on_line(&format!("校验未通过,第 {attempt} 次重试"), None);
            feedback = if v.feedback { Some(findings) } else { None };
            continue;
        }
        match v.on_unmet {
            OnUnmet::Continue => {
                self.finish(step_id, format!("{done_label} · 未达标(continue)"), step_metrics);
                return Ok(());
            }
            OnUnmet::Fail => {
                self.fail_with_metrics(
                    step_id,
                    format!("校验未通过(已重试 {} 次)", v.max_retries),
                    step_metrics.clone(),
                );
                return Err(());
            }
            OnUnmet::Gate => {
                let suggestion = format!(
                    "校验未通过(重试 {} 次仍未达标),选择 重试 / 跳过 / 中止\n{findings}",
                    v.max_retries
                );
                match self.decision_gate(step_id, suggestion) {
                    StepDecision::Retry => {
                        attempt = 0;
                        feedback = if v.feedback { Some(findings) } else { None };
                        continue;
                    }
                    StepDecision::Skip => {
                        self.emit_skipped(step_id);
                        return Ok(());
                    }
                    StepDecision::Abort => return Err(()),
                }
            }
        }
    }
}
```

注意保留原注释里的关键行内注释（charge 顺序 / finding #4 / P4 等）随代码块一起搬——原 Claude arm 的注释是在案决策记录，不能在外提时丢失。

Claude match arm 整体替换为：

```rust
StepKind::Claude { prompt, skill, verify } => self.run_verified_step(
    &step.id,
    VerifiedWork::Claude { prompt, skill: skill.as_deref() },
    verify.as_ref(),
),
```

本 task 不动 Acp arm（仍走旧路径），保证行为差异为零。

- [ ] **Step 3: 验证行为不变**

Run: `cargo test --workspace` → 与 Step 1 同样全绿、同样数量
Run: `cargo clippy --workspace --all-targets -- -D warnings` → 零告警（`VerifiedWork::Acp` 未使用会报 dead_code——在 enum 上临时加 `#[allow(dead_code)] // Task 3 接入后移除`）

- [ ] **Step 4: Commit**

```bash
git add crates/engine/src/executor.rs
git commit -m "refactor(engine): verify-retry 循环外提为 run_verified_step(行为不变,为 acp verify 门铺路)"
```

---

### Task 3: D2 Acp verify 门

**Files:**
- Modify: `crates/engine/src/manifest.rs`（Acp 变体 + validate_verify 抽取 + 测试）
- Modify: `crates/engine/src/executor.rs`（Acp arm 切 helper，删 `#[allow(dead_code)]`）
- Create: `crates/engine/tests/acp_verify.rs`

**Interfaces:**
- Consumes: Task 2 的 `run_verified_step` / `VerifiedWork::Acp`；mock_acp_agent 的 `happy` 场景（answer 固定 `你好,我是 mock。`）。
- Produces: `StepKind::Acp` 新增 `verify: Option<Verify>` 字段；`Manifest::validate_verify(step_id, v)` 私有函数（Claude / Acp 共用）。

- [ ] **Step 1: 写失败测试**

manifest.rs tests 追加：

```rust
#[test]
fn acp_verify_codex_missing_action_rejected() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: g\n    command: c\n    prompt: p\n    verify:\n      by: codex\n";
    let err = Manifest::parse(y).unwrap().validate().unwrap_err().to_string();
    assert!(err.contains("verify by codex 需要 action"), "{err}");
}

#[test]
fn acp_verify_command_accepted() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: g\n    command: c\n    prompt: p\n    verify:\n      by: command\n      command: \"true\"\n";
    assert!(Manifest::parse(y).unwrap().validate().is_ok());
}
```

新建 `crates/engine/tests/acp_verify.rs`（fixture 复用 acp_runner.rs 的 mock 构建方式）：

```rust
//! Acp step + verify 门的 executor 级集成测试(spec D2)。

use agentpipe_engine::control::Control;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, RunStatus};
use std::path::PathBuf;
use std::process::Command as Proc;
use std::sync::{mpsc, Arc, Once};

static BUILD_MOCK: Once = Once::new();

fn mock_command() -> String {
    BUILD_MOCK.call_once(|| {
        let status = Proc::new("cargo")
            .args(["build", "--quiet", "--example", "mock_acp_agent"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("spawn cargo build");
        assert!(status.success());
    });
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bin = root.parent().unwrap().parent().unwrap().join("target/debug/examples/mock_acp_agent");
    format!("env MOCK_ACP_SCENARIO=happy {}", bin.to_string_lossy())
}

fn run_manifest(yaml: &str) -> (RunStatus, Vec<Event>) {
    let manifest = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_ctx, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::try_new(
        manifest,
        RunnerBins { claude: "claude-not-used".into(), codex: "codex-not-used".into() },
        Arc::new(Control::default()),
        etx,
        crx,
    )
    .unwrap();
    let status = ex.run();
    (status, erx.try_iter().collect())
}

#[test]
fn acp_step_with_command_verify_passes() {
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{}\"\n    prompt: hi\n    verify:\n      by: command\n      command: \"true\"\n",
        mock_command()
    );
    let (status, events) = run_manifest(&yaml);
    assert!(matches!(status, RunStatus::Success), "events: {events:?}");
    let done = events.iter().any(|e| matches!(e,
        Event::StepFinished { summary, .. } if summary.contains("已校验")));
    assert!(done, "StepFinished 应带 已校验 标记: {events:?}");
}

#[test]
fn acp_step_with_failing_verify_on_unmet_fail() {
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{}\"\n    prompt: hi\n    verify:\n      by: command\n      command: \"false\"\n      max_retries: 0\n      on_unmet: fail\n",
        mock_command()
    );
    let (status, events) = run_manifest(&yaml);
    assert!(matches!(status, RunStatus::Failed), "events: {events:?}");
    assert!(events.iter().any(|e| matches!(e, Event::StepFailed { .. })));
}
```

- [ ] **Step 2: 跑测确认失败**

Run: `cargo test -p agentpipe-engine acp_verify -- --nocapture`
Expected: 编译失败（Acp 变体无 verify 字段 / YAML 解析多出 verify 键报 unknown field）

- [ ] **Step 3: 实现**

manifest.rs `Acp` 变体加字段：

```rust
/// 可选校验门:与 claude step 同语义(裁判 codex/claude/command 与 step 类型解耦)。
/// 见 acp-hardening spec D2。
#[serde(default)]
verify: Option<Verify>,
```

把 Claude arm 里的 verify 校验体原样外提为（内容 = manifest.rs 现 Claude arm 的 `if let Some(v)` 块体，`step.id` 换 `step_id`，全文如下）：

```rust
/// verify 门配置校验(Claude / Acp step 共用,spec D2)。
fn validate_verify(step_id: &str, v: &Verify) -> Result<(), EngineError> {
    match v.by {
        Verifier::Codex => {
            let action = v.action.as_ref().ok_or_else(|| {
                EngineError::Validation(format!(
                    "step '{step_id}': verify by codex 需要 action 字段"
                ))
            })?;
            Self::validate_codex_fields(step_id, "verify codex", action, &v.path, &v.base, &v.prompt)?;
        }
        Verifier::Claude => {
            if v.prompt.is_none() {
                return Err(EngineError::Validation(format!(
                    "step '{step_id}': verify by claude 需要 prompt 字段(判定指令)"
                )));
            }
        }
        Verifier::Command => {
            Self::require_non_empty(
                step_id,
                "verify command",
                v.command.as_deref().unwrap_or(""),
                Some("shell 命令,例: command: \"cargo test\""),
            )?;
        }
    }
    if v.max_retries > MAX_VERIFY_RETRIES {
        return Err(EngineError::Validation(format!(
            "step '{step_id}': verify.max_retries 不能超过 {MAX_VERIFY_RETRIES}"
        )));
    }
    Ok(())
}
```

Claude arm 与 Acp arm 各调用 `if let Some(v) = verify { Self::validate_verify(&step.id, v)?; }`（Claude arm 原地内联体删除）。

executor.rs Acp arm 整体替换（原 345-383 行；原 arm 顶部的 finding #7 大段注释移到 `run_verified_step` 的 Err-分派处，已在 Task 2 落位）：

```rust
StepKind::Acp { agent, command, prompt, verify } => self.run_verified_step(
    &step.id,
    VerifiedWork::Acp { agent, command, prompt },
    verify.as_ref(),
),
```

删掉 Task 2 加的 `#[allow(dead_code)]`。

行为说明（预期差异，写进 commit message）：Acp 成功路径现在也会 `emit_answer_preview`（与 claude 对齐的体验增强）；summary 仍为 `done · acp`。

- [ ] **Step 4: 验证**

Run: `cargo test --workspace` → PASS（含新 acp_verify 集成测试）
Run: `cargo clippy --workspace --all-targets -- -D warnings` → 零告警

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/manifest.rs crates/engine/src/executor.rs crates/engine/tests/acp_verify.rs
git commit -m "feat(engine): D2 acp step 支持 verify 门 — 复用 run_verified_step,validate_verify 两 kind 共用"
```

---

### Task 4: D5 transcript 渲染去 Debug 格式

**Files:**
- Modify: `crates/engine/src/runner/acp.rs`（`format_update_for_log` + 单测）

**Interfaces:**
- Consumes: SDK schema v1 已核实字段——`ToolCall { title: String, kind: ToolKind, status: ToolCallStatus, .. }`；`ToolCallUpdate { tool_call_id, fields: ToolCallUpdateFields, .. }`，fields 内 `title/status/kind` 均为 Option；`Plan { entries: Vec<PlanEntry> }`，`PlanEntry { content: String, .. }`。

- [ ] **Step 1: 写失败测试**（acp.rs 底部新增 `#[cfg(test)] mod tests`）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, ToolCall, ToolCallId, ToolCallStatus, ToolKind};

    #[test]
    fn tool_call_renders_title_kind_status_not_debug() {
        let tc = ToolCall::new(ToolCallId::new("t1"), "读取配置文件")
            .kind(ToolKind::Read)
            .status(ToolCallStatus::InProgress);
        let line = format_update_for_log(&SessionUpdate::ToolCall(tc));
        assert!(line.starts_with("[tool] "), "{line}");
        assert!(line.contains("读取配置文件"), "{line}");
        assert!(!line.contains("ToolCall {"), "不许再用 Debug 全量输出: {line}");
    }

    #[test]
    fn plan_renders_entry_count_and_first_item() {
        let plan = Plan::new(vec![
            PlanEntry::new("先读代码", PlanEntryPriority::High, PlanEntryStatus::Pending),
            PlanEntry::new("再改", PlanEntryPriority::Low, PlanEntryStatus::Pending),
        ]);
        let line = format_update_for_log(&SessionUpdate::Plan(plan));
        assert!(line.starts_with("[plan] 2 项"), "{line}");
        assert!(line.contains("先读代码"), "{line}");
    }
}
```

（SDK 构造器均为"required 字段进 new,可选字段链式 setter"模式；若 `ToolCall::new` / `PlanEntry::new` 实参形状与上不符，以 `cargo doc -p agent-client-protocol --open` 的 schema v1 为准调整测试构造——断言不变。）

- [ ] **Step 2: 跑测确认失败**

Run: `cargo test -p agentpipe-engine tool_call_renders -- --nocapture`
Expected: FAIL（当前 `{:?}` 输出以 `[tool] ToolCall {` 开头）

- [ ] **Step 3: 实现**（替换 `format_update_for_log` 三个 arm）

```rust
SessionUpdate::ToolCall(tc) => format!(
    "[tool] {} · {:?} · {:?}",
    truncate(&tc.title, 60),
    tc.kind,
    tc.status
),
SessionUpdate::ToolCallUpdate(u) => {
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = &u.fields.title {
        parts.push(truncate(t, 60));
    }
    if let Some(s) = &u.fields.status {
        parts.push(format!("{s:?}"));
    }
    if parts.is_empty() {
        parts.push(format!("{:?}", u.tool_call_id));
    }
    format!("[tool-update] {}", parts.join(" · "))
}
SessionUpdate::Plan(p) => {
    let first = p.entries.first().map(|e| truncate(&e.content, 60)).unwrap_or_default();
    format!("[plan] {} 项 · {first}", p.entries.len())
}
```

（`ToolKind` / `ToolCallStatus` 是简单枚举，`{:?}` 输出 `Read` / `InProgress` 一类短词，可读，不属于"Debug 全量结构"问题。未知 update 类型的 `_ =>` 兜底分支保留。）

- [ ] **Step 4: 验证**

Run: `cargo test --workspace` → PASS；`cargo clippy --workspace --all-targets -- -D warnings` → 零告警

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/runner/acp.rs
git commit -m "fix(engine): D5 acp transcript 渲染 — ToolCall/Plan 字段级输出,不再 Debug 全量"
```

---

### Task 5: base_dir 公共 helper + agents registry 模块

**Files:**
- Create: `crates/engine/src/paths.rs`
- Create: `crates/engine/src/agents.rs`
- Modify: `crates/engine/src/lib.rs`（`pub mod paths; pub mod agents;`）
- Modify: `crates/engine/Cargo.toml`（`toml = "0.8"`）
- Modify: `crates/cli/src/main.rs`（`runs_dir` 改用 engine helper）
- Modify: `src-tauri/src/paths.rs`（`base()` 改用 engine helper）

**Interfaces:**
- Produces: `agentpipe_engine::paths::base_dir() -> PathBuf`（= `$AGENTPIPE_HOME|$HOME` + `/.agentpipe`）；`agents::AgentRegistry::{load_default, load_from, command_for}`。Task 6 依赖 registry；Phase B watch state 依赖 base_dir。

- [ ] **Step 1: 写失败测试**（`crates/engine/src/agents.rs` 直接带 tests 写入，见 Step 3 文件全文；先建文件跑测自然红）

- [ ] **Step 2: 实现 paths.rs**（全文）

```rust
//! 数据目录单一来源:$AGENTPIPE_HOME(替代 HOME)或 $HOME,拼 `.agentpipe`。
//! 语义与 cli runs_dir / src-tauri paths 既有约定一致(AGENTPIPE_HOME 是替代
//! HOME,不是替代 ~/.agentpipe)。runs / agents.toml / (Phase B) watch state 共用。

use std::path::PathBuf;

pub fn base_dir() -> PathBuf {
    let base = std::env::var("AGENTPIPE_HOME")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".agentpipe")
}
```

- [ ] **Step 3: 实现 agents.rs**（全文）

```rust
//! ACP agent registry:`base_dir()/agents.toml` 把"启动命令"从模板解耦,
//! 模板只写 agent 名即可跨机器分享。见 acp-hardening spec D4。
//!
//! ```toml
//! [agents.gemini]
//! command = "gemini --experimental-acp"
//! ```

use crate::error::EngineError;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct AgentRegistry {
    map: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
struct RegistryFile {
    #[serde(default)]
    agents: HashMap<String, RegistryEntry>,
}

#[derive(serde::Deserialize)]
struct RegistryEntry {
    command: String,
}

impl AgentRegistry {
    /// 文件不存在 = 空表(仅用内联 command 的用户零感知);存在但解析失败 =
    /// fail-loud(防"改了 registry 没生效"的静默漂移)。
    pub fn load_default() -> Result<Self, EngineError> {
        Self::load_from(&crate::paths::base_dir().join("agents.toml"))
    }

    pub fn load_from(path: &Path) -> Result<Self, EngineError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(EngineError::Validation(format!(
                    "读取 agents registry {} 失败: {e}",
                    path.display()
                )))
            }
        };
        let parsed: RegistryFile = toml::from_str(&text).map_err(|e| {
            EngineError::Validation(format!(
                "agents registry {} 解析失败(TOML): {e}",
                path.display()
            ))
        })?;
        Ok(Self {
            map: parsed.agents.into_iter().map(|(k, v)| (k, v.command)).collect(),
        })
    }

    pub fn command_for(&self, agent: &str) -> Option<&str> {
        self.map.get(agent).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_registry() {
        let r = AgentRegistry::load_from(Path::new("/nonexistent/agents.toml")).unwrap();
        assert!(r.command_for("gemini").is_none());
    }

    #[test]
    fn parses_entries() {
        let dir = std::env::temp_dir().join("ap-agents-test-ok");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("agents.toml");
        std::fs::write(&p, "[agents.gemini]\ncommand = \"gemini --acp\"\n").unwrap();
        let r = AgentRegistry::load_from(&p).unwrap();
        assert_eq!(r.command_for("gemini"), Some("gemini --acp"));
        assert!(r.command_for("unknown").is_none());
    }

    #[test]
    fn bad_toml_fails_loud() {
        let dir = std::env::temp_dir().join("ap-agents-test-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("agents.toml");
        std::fs::write(&p, "[agents.gemini\ncommand=").unwrap();
        let err = AgentRegistry::load_from(&p).unwrap_err().to_string();
        assert!(err.contains("解析失败"), "{err}");
    }
}
```

lib.rs 追加 `pub mod agents;` 与 `pub mod paths;`（按字母序插入）；engine Cargo.toml `[dependencies]` 加 `toml = "0.8"`。

- [ ] **Step 4: 两个宿主接线（去重复）**

`crates/cli/src/main.rs` 的 `runs_dir` 函数体替换为：

```rust
pub(crate) fn runs_dir() -> PathBuf {
    agentpipe_engine::paths::base_dir().join("runs")
}
```

`src-tauri/src/paths.rs` 的 `base()` 删除，`runs_dir` / `tasks_dir` 改为：

```rust
pub fn runs_dir() -> PathBuf {
    agentpipe_engine::paths::base_dir().join("runs")
}

pub fn tasks_dir() -> PathBuf {
    agentpipe_engine::paths::base_dir().join("tasks")
}
```

（`home()` / `resolve_task_path` 不动——`~` 展开语义故意不受 AGENTPIPE_HOME 影响，注释已写明。）

- [ ] **Step 5: 验证**

Run: `cargo test --workspace` → PASS（含 agents 三个新测试、src-tauri paths 既有测试）
Run: `cargo clippy --workspace --all-targets -- -D warnings` → 零告警

- [ ] **Step 6: Commit**

```bash
git add crates/engine/src/paths.rs crates/engine/src/agents.rs crates/engine/src/lib.rs crates/engine/Cargo.toml crates/cli/src/main.rs src-tauri/src/paths.rs Cargo.lock
git commit -m "feat(engine): D4 前置 — base_dir 公共 helper + agents.toml registry 模块,CLI/Tauri 路径去重"
```

---

### Task 6: D4 收尾 — command 可选化 + resolve_agents pre-pass + 宿主接线

**Files:**
- Modify: `crates/engine/src/manifest.rs`（Acp.command → Option + validate 文案 + 测试）
- Modify: `crates/engine/src/agents.rs`（resolve_agents + 测试）
- Modify: `crates/engine/src/executor.rs`（Acp arm 解 Option，防御 fail-loud）
- Modify: `crates/cli/src/main.rs`（load_manifest 接 pre-pass）
- Modify: `src-tauri/src/commands.rs`（3 处 Manifest::parse 后接 pre-pass）

**Interfaces:**
- Produces: `agents::resolve_agents(&mut Manifest, &AgentRegistry)`（纯回填，不报错——"仍为 None"由 validate 负责可解释报错，validate 保持无 I/O）。
- Consumes: Task 5 的 `AgentRegistry`；Task 2/3 的 `VerifiedWork::Acp`（`command: &str` 不变，arm 解 Option 后传入）。

- [ ] **Step 1: 写失败测试**

manifest.rs tests 追加：

```rust
#[test]
fn acp_missing_command_error_mentions_registry() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: gemini\n    prompt: p\n";
    let err = Manifest::parse(y).unwrap().validate().unwrap_err().to_string();
    assert!(err.contains("agents.toml"), "错误必须指向 registry 出路: {err}");
    assert!(err.contains("gemini"), "错误必须点名 agent: {err}");
}
```

agents.rs tests 追加：

```rust
#[test]
fn resolve_fills_only_missing_command_recursively() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: gemini\n    prompt: p\n  - id: b\n    kind: acp\n    agent: gemini\n    command: inline-wins\n    prompt: p\n  - id: l\n    kind: loop\n    until: codex-clean\n    max: 2\n    body:\n      - id: r\n        kind: codex\n        action: review-mr\n        base: main\n      - id: inner\n        kind: acp\n        agent: gemini\n        prompt: p\n";
    let mut m = crate::manifest::Manifest::parse(y).unwrap();
    let mut reg = AgentRegistry::default();
    reg.map.insert("gemini".into(), "gemini --acp".into());
    resolve_agents(&mut m, &reg);
    let cmds: Vec<Option<String>> = collect_acp_commands(&m.steps);
    assert_eq!(cmds, vec![
        Some("gemini --acp".into()),
        Some("inline-wins".into()),
        Some("gemini --acp".into()),
    ]);
}
```

（测试辅助 `collect_acp_commands` 与 resolve 的 walk 同构，写在 tests mod 内。）

- [ ] **Step 2: 跑测确认失败**

Run: `cargo test -p agentpipe-engine resolve_fills -- --nocapture`
Expected: 编译失败（command 还是必填 String / resolve_agents 不存在）

- [ ] **Step 3: 实现**

manifest.rs Acp 变体 command 字段改为：

```rust
/// 启动外部 ACP server 的完整命令(shell-words 切分)。可省略:由
/// `agents::resolve_agents` pre-pass 按 agent 名从 agents.toml 回填。
#[serde(default, skip_serializing_if = "Option::is_none")]
command: Option<String>,
```

validate 的 Acp arm 改为（require_non_empty 的 command 检查换成两段）：

```rust
StepKind::Acp { agent, command, prompt, verify } => {
    Self::require_non_empty(&step.id, "acp.agent", agent, Some("显示用名称"))?;
    match command {
        None => {
            return Err(EngineError::Validation(format!(
                "step '{}': acp.command 缺失,且 agents registry 未命中 '{agent}'。两条出路:在 step 内联 command,或在 {}/agents.toml 增加 [agents.{agent}] command = \"...\"",
                step.id,
                crate::paths::base_dir().display()
            )))
        }
        Some(c) => Self::require_non_empty(&step.id, "acp.command", c, Some("启动外部 agent 的完整命令"))?,
    }
    Self::require_non_empty(&step.id, "acp.prompt", prompt, None)?;
    if let Some(v) = verify {
        Self::validate_verify(&step.id, v)?;
    }
    Ok(())
}
```

（`base_dir()` 只读 env 不碰文件系统，validate 仍无 I/O。）

agents.rs 追加：

```rust
/// pre-pass:把 registry 命中回填进 command 为 None 的 acp step(含 loop body 递归)。
/// 查不到不报错 —— "仍为 None" 由 Manifest::validate 给可解释报错,validate 保持纯函数。
/// 调用时机:宿主 parse 之后、validate / Executor::try_new 之前。
pub fn resolve_agents(manifest: &mut crate::manifest::Manifest, registry: &AgentRegistry) {
    fn walk(steps: &mut [crate::manifest::Step], registry: &AgentRegistry) {
        for s in steps {
            match &mut s.kind {
                crate::manifest::StepKind::Acp { agent, command, .. } => {
                    if command.is_none() {
                        if let Some(c) = registry.command_for(agent) {
                            *command = Some(c.to_string());
                        }
                    }
                }
                crate::manifest::StepKind::Loop { body, .. } => walk(body, registry),
                _ => {}
            }
        }
    }
    walk(&mut manifest.steps, registry);
}
```

（`Step.kind` / `StepKind` 字段可见性不足时，把所需字段改 `pub`——manifest 类型本就整体 `pub` 导出，与既有风格一致。）

executor.rs Acp arm 解 Option（validate/try_new 双保险后防御性 fail-loud，不 panic）：

```rust
StepKind::Acp { agent, command, prompt, verify } => {
    let Some(cmd) = command.as_deref() else {
        self.fail(&step.id, format!(
            "acp step '{}' 的 command 未解析(resolve_agents 未跑或 registry 未命中)",
            step.id
        ));
        self.control.request_abort();
        return Err(());
    };
    self.run_verified_step(
        &step.id,
        VerifiedWork::Acp { agent, command: cmd, prompt },
        verify.as_ref(),
    )
}
```

CLI `load_manifest`（crates/cli/src/main.rs）在 parse 与 validate 之间插 pre-pass：

```rust
fn load_manifest(path: &str) -> Manifest {
    let yaml = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("failed to read {path}: {e}");
        std::process::exit(1);
    });
    match Manifest::parse(&yaml).and_then(|mut m| {
        let registry = agentpipe_engine::agents::AgentRegistry::load_default()?;
        agentpipe_engine::agents::resolve_agents(&mut m, &registry);
        m.validate()?;
        Ok(m)
    }) {
        // …既有 Ok/Err 分支不动…
    }
}
```

src-tauri/src/commands.rs 三处（32 附近的 validate 调用点、52 与 115 的 parse 调用点）统一为同一形状：

```rust
let mut manifest = Manifest::parse(&yaml).map_err(|e| e.to_string())?;
let registry = agentpipe_engine::agents::AgentRegistry::load_default().map_err(|e| e.to_string())?;
agentpipe_engine::agents::resolve_agents(&mut manifest, &registry);
manifest.validate().map_err(|e| e.to_string())?;
```

- [ ] **Step 4: 验证**

Run: `cargo test --workspace` → PASS（既有"内联 command"YAML 测试全部照旧——Option 化向后兼容）
Run: `cargo clippy --workspace --all-targets -- -D warnings` → 零告警

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/manifest.rs crates/engine/src/agents.rs crates/engine/src/executor.rs crates/cli/src/main.rs src-tauri/src/commands.rs
git commit -m "feat(engine/cli/tauri): D4 收尾 — acp.command 可省略,resolve_agents pre-pass 按 registry 回填"
```

---

### Task 7: D3 前半 — runner 权限回调通道

**Files:**
- Modify: `crates/engine/src/runner/acp.rs`（PermissionMode/PermissionDecision + run 签名 + worker↔main 通道）
- Modify: `crates/engine/examples/mock_acp_agent.rs`（permission_probe / permission_probe_slow 场景）
- Modify: `crates/engine/tests/acp_runner.rs`（既有 callsite 补参数 + 新测试）
- Modify: `crates/engine/src/executor.rs`（helper 的 Acp attempt 补 `PermissionMode::Reject` 参数——本 task 行为仍是全拒）

**Interfaces:**
- Produces（Task 8 依赖）:

```rust
pub enum PermissionDecision { Approve, RejectOnce, Abort }
pub enum PermissionMode<'a> {
    Reject,
    Ask(&'a mut dyn FnMut(&str) -> PermissionDecision),
}
// run 签名(新增末参):
pub fn run(&self, prompt: &str, control: Option<&Control>,
    on_progress: &mut dyn FnMut(&str, Option<u32>), cwd: &Path,
    permission: PermissionMode<'_>) -> Result<AcpOutcome, EngineError>
```

- Consumes: SDK 已核实类型——`RequestPermissionRequest { tool_call: ToolCallUpdate, options: Vec<PermissionOption>, .. }`；`PermissionOption { option_id, name, kind: PermissionOptionKind::{AllowOnce, AllowAlways, RejectOnce, RejectAlways} }`；`RequestPermissionOutcome::{Cancelled, Selected(SelectedPermissionOutcome)}`。

- [ ] **Step 1: mock 加场景**（examples/mock_acp_agent.rs 的 prompt handler match 追加；import 补 `PermissionOption, PermissionOptionId, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest, ToolCallId, ToolCallUpdate`）

```rust
"permission_probe" | "permission_probe_slow" | "permission_probe_noallow" => {
    // 发权限请求 → 按 client outcome 决定 answer:Selected→granted / Cancelled→denied。
    // slow 变体在回 chunk 后 sleep 10s 才 EndTurn,给 abort 测试确定性窗口。
    // noallow 变体只提供 reject 类选项,钉"Approve 但无 allow 选项 → 回退 Cancelled"。
    let slow = scenario == "permission_probe_slow";
    let noallow = scenario == "permission_probe_noallow";
    let cx2 = cx.clone();
    let sid = prompt.session_id.clone();
    cx.spawn(async move {
        let options = if noallow {
            vec![PermissionOption::new(
                PermissionOptionId::new("reject-1"), "拒绝", PermissionOptionKind::RejectOnce,
            )]
        } else {
            vec![
                PermissionOption::new(PermissionOptionId::new("allow-1"), "允许一次", PermissionOptionKind::AllowOnce),
                PermissionOption::new(PermissionOptionId::new("reject-1"), "拒绝", PermissionOptionKind::RejectOnce),
            ]
        };
        let req = RequestPermissionRequest::new(
            sid.clone(),
            ToolCallUpdate::new(ToolCallId::new("tc-perm-1")),
            options,
        );
        let resp = cx2.send_request(req).block_task().await;
        let text = match resp {
            Ok(r) if matches!(r.outcome, RequestPermissionOutcome::Selected(_)) => "granted",
            _ => "denied",
        };
        let _ = cx2.send_notification(SessionNotification::new(
            sid.clone(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(text),
            ))),
        ));
        if slow {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        responder.respond(PromptResponse::new(StopReason::EndTurn))
    })
}
```

（responder 移进 spawn 延迟应答；若 SDK 的 `cx.spawn` 闭包签名要求返回 `Result<(), Error>`，按 fs_probe 场景的既有写法包一层 `Ok(())` 并把 respond 放 spawn 末尾。构造器实参以 schema v1 required 字段为准：PermissionOption 必填 option_id/name/kind，RequestPermissionRequest 必填 session_id/tool_call/options。）

- [ ] **Step 2: runner 实现**（acp.rs）

新增公开类型（AcpOutcome 旁）：

```rust
/// 权限请求的宿主决策(spec D3)。
pub enum PermissionDecision {
    /// 批准:runner 在 agent options 里优先选 AllowOnce,其次 AllowAlways;
    /// 无 allow 选项 → 回 Cancelled 并在 transcript 记一行。
    Approve,
    /// 拒绝该次请求(session 继续)。
    RejectOnce,
    /// 中止 run:runner 回 Cancelled 并立即触发 abort 收尾。
    Abort,
}

/// 权限策略:Reject = 现状语义(一律 Cancelled);Ask = 每个请求同步回调宿主。
pub enum PermissionMode<'a> {
    Reject,
    Ask(&'a mut dyn FnMut(&str) -> PermissionDecision),
}

/// worker → main 的权限请求消息。
struct PermissionAskMsg {
    description: String,
    reply: tokio::sync::oneshot::Sender<PermissionReply>,
}

enum PermissionReply {
    Approve,
    Reject,
}
```

`run` 加末参 `mut permission: PermissionMode<'_>`；建通道 `let (perm_tx, perm_rx) = std_mpsc::channel::<PermissionAskMsg>();`，`perm_tx` 传给 `run_acp_session`（签名加参）。主线程 drain loop 的 abort 检查后加：

```rust
// 权限请求:非阻塞取,同步回调宿主(Ask 下可能长阻塞在决策门 —— worker 侧
// handler 在 oneshot 上 .await,同一 select! 的 timeout/abort 分支保持活性,
// spec D3/F2)。门等待期间本 loop 暂停 drain,已知边界见 spec §5 末条。
while let Ok(ask) = perm_rx.try_recv() {
    let decision = match &mut permission {
        PermissionMode::Reject => PermissionDecision::RejectOnce,
        PermissionMode::Ask(cb) => cb(&ask.description),
    };
    let reply = match decision {
        PermissionDecision::Approve => PermissionReply::Approve,
        PermissionDecision::RejectOnce => PermissionReply::Reject,
        PermissionDecision::Abort => {
            abort_notify.notify_waiters();
            PermissionReply::Reject
        }
    };
    // send 失败 = worker 已死(超时先到),按 spec §5 反方向条目:忽略,
    // step 终态以 worker 侧 Err 为准。
    let _ = ask.reply.send(reply);
}
```

`run_acp_session` 里替换现有 `RequestPermissionRequest` 的 handler（原 acp.rs:266-275）：

```rust
.on_receive_request(
    {
        let perm_tx = perm_tx.clone();
        let transcript_tx = transcript_tx.clone();
        move |request: RequestPermissionRequest, responder, _cx| {
            let perm_tx = perm_tx.clone();
            let transcript_tx = transcript_tx.clone();
            async move {
                let description = request
                    .tool_call
                    .fields
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", request.tool_call.tool_call_id));
                let _ = transcript_tx.send(format!("[permission] 请求: {description}"));
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let outcome = if perm_tx
                    .send(PermissionAskMsg { description, reply: reply_tx })
                    .is_err()
                {
                    RequestPermissionOutcome::Cancelled
                } else {
                    match reply_rx.await {
                        Ok(PermissionReply::Approve) => {
                            let pick = request
                                .options
                                .iter()
                                .find(|o| matches!(o.kind, PermissionOptionKind::AllowOnce))
                                .or_else(|| {
                                    request.options.iter().find(|o| {
                                        matches!(o.kind, PermissionOptionKind::AllowAlways)
                                    })
                                });
                            match pick {
                                Some(o) => RequestPermissionOutcome::Selected(
                                    SelectedPermissionOutcome::new(o.option_id.clone()),
                                ),
                                None => {
                                    let _ = transcript_tx.send(
                                        "[permission] 批准但 agent 未提供 allow 选项,回退 Cancelled".into(),
                                    );
                                    RequestPermissionOutcome::Cancelled
                                }
                            }
                        }
                        Ok(PermissionReply::Reject) | Err(_) => RequestPermissionOutcome::Cancelled,
                    }
                };
                responder.respond(RequestPermissionResponse::new(outcome))
            }
        }
    },
    agent_client_protocol::on_receive_request!(),
)
```

import 追加 `PermissionOptionKind, SelectedPermissionOutcome`。既有 callsite 补参：executor.rs 的 helper Acp attempt（Task 2 落位处）传 `crate::runner::acp::PermissionMode::Reject`；tests/acp_runner.rs 的 `run_scenario_full` 传 `PermissionMode::Reject`（并加一个带回调的变体，见 Step 3）。

- [ ] **Step 3: 测试**（tests/acp_runner.rs 追加）

```rust
fn run_scenario_perm(
    scenario: &str,
    cb: &mut dyn FnMut(&str) -> agentpipe_engine::runner::acp::PermissionDecision,
) -> Result<AcpOutcome, String> {
    let command = mock_command();
    let full_cmd = format!("env MOCK_ACP_SCENARIO={scenario} {command}");
    let runner = AcpRunner::with_timeout(
        AcpConfig { agent: format!("mock-{scenario}"), command: full_cmd },
        30,
    );
    let cwd = std::env::current_dir().unwrap();
    runner
        .run("go", None, &mut |_l, _r| {}, &cwd,
            agentpipe_engine::runner::acp::PermissionMode::Ask(cb))
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn permission_reject_mode_yields_denied() {
    // 缺省 Reject 模式:与旧行为一致,agent 收 Cancelled。
    let out = run_scenario("permission_probe", "go").expect("应正常完成");
    assert_eq!(out.answer, "denied");
}

#[test]
fn permission_ask_approve_yields_granted() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    let mut asked = Vec::new();
    let out = run_scenario_perm("permission_probe", &mut |desc| {
        asked.push(desc.to_string());
        PermissionDecision::Approve
    })
    .expect("应正常完成");
    assert_eq!(out.answer, "granted");
    assert_eq!(asked.len(), 1, "回调应被调用一次");
}

#[test]
fn permission_ask_reject_yields_denied() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    let out = run_scenario_perm("permission_probe", &mut |_| PermissionDecision::RejectOnce)
        .expect("应正常完成");
    assert_eq!(out.answer, "denied");
}

#[test]
fn permission_ask_approve_without_allow_option_falls_back_cancelled() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    // spec §6 第五条路径:批准但 agent 未提供 allow 选项 → 回 Cancelled,agent 视角=denied。
    let out = run_scenario_perm("permission_probe_noallow", &mut |_| PermissionDecision::Approve)
        .expect("应正常完成(回退 Cancelled 不是错误)");
    assert_eq!(out.answer, "denied");
    assert!(
        out.full_transcript.contains("未提供 allow 选项"),
        "transcript 必须记录回退原因: {}",
        out.full_transcript
    );
}

#[test]
fn permission_ask_abort_aborts_run() {
    use agentpipe_engine::runner::acp::PermissionDecision;
    // slow 变体:agent 回 chunk 后拖 10s 才 EndTurn → abort 分支确定性先赢。
    let err = run_scenario_perm("permission_probe_slow", &mut |_| PermissionDecision::Abort)
        .expect_err("Abort 决策必须中止 run");
    assert!(err.contains("中止"), "{err}");
}
```

（`run_scenario` 的 transcript 断言场景不受影响；`fs_reverse_request_is_rejected_without_hang` 等既有测试只需在 `run_scenario_full` 里补 `PermissionMode::Reject` 参数。）

- [ ] **Step 4: 验证**

Run: `cargo test -p agentpipe-engine --test acp_runner -- --nocapture` → 新旧全 PASS
Run: `cargo test --workspace` → PASS；`cargo clippy --workspace --all-targets -- -D warnings` → 零告警

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/runner/acp.rs crates/engine/src/executor.rs crates/engine/examples/mock_acp_agent.rs crates/engine/tests/acp_runner.rs
git commit -m "feat(engine): D3 前半 — acp runner 权限回调通道(tokio oneshot,Reject 缺省语义不变)"
```

---

### Task 8: D3 收尾 — on_permission 策略 + 决策门接线

**Files:**
- Modify: `crates/engine/src/manifest.rs`（PermissionPolicy 枚举 + Acp 字段 + 测试）
- Modify: `crates/engine/src/executor.rs`（VerifiedWork::Acp 带策略 + Ask→decision_gate 闭包）
- Modify: `crates/engine/tests/acp_verify.rs`（executor 级权限门集成测试）

**Interfaces:**
- Consumes: Task 7 的 `PermissionMode` / `PermissionDecision`；既有 `decision_gate`（Approve 命令=Retry 变体 / Skip / 其余 Abort，Abort 自动 request_abort）；CLI 的 Decision 门 stdin-EOF→Abort 语义（main.rs:171,无需改动）。
- Produces: `PermissionPolicy { Reject(default), Ask }`（manifest 公开导出，Phase C 的 parallel 校验会引用"ask 被拒"规则）。

- [ ] **Step 1: 写失败测试**

manifest.rs tests：

```rust
#[test]
fn acp_on_permission_defaults_reject_and_parses_ask() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: g\n    command: c\n    prompt: p\n    on_permission: ask\n";
    let m = Manifest::parse(y).unwrap();
    assert!(m.validate().is_ok());
    let y2 = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: g\n    command: c\n    prompt: p\n";
    assert!(Manifest::parse(y2).unwrap().validate().is_ok(), "缺字段 = reject 缺省,向后兼容");
}
```

tests/acp_verify.rs 追加（复用该文件的 mock_command / 通道骨架，scenario 换 permission_probe）：

```rust
#[test]
fn acp_ask_policy_opens_decision_gate_and_approve_grants() {
    let command = mock_command().replace("MOCK_ACP_SCENARIO=happy", "MOCK_ACP_SCENARIO=permission_probe");
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{command}\"\n    prompt: go\n    on_permission: ask\n"
    );
    let manifest = Manifest::parse(&yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx_tx, crx) = mpsc::channel::<Command>();
    // 预置批准指令:权限门弹出时 decision_gate 的 recv 立即拿到 Approve。
    ctx_tx.send(Command::ApproveGate { step_id: "a".into(), artifact: None }).unwrap();
    let mut ex = Executor::try_new(
        manifest,
        RunnerBins { claude: "unused".into(), codex: "unused".into() },
        Arc::new(Control::default()),
        etx,
        crx,
    )
    .unwrap();
    let status = ex.run();
    let events: Vec<Event> = erx.try_iter().collect();
    assert!(matches!(status, RunStatus::Success), "{events:?}");
    use agentpipe_engine::protocol::GateKind;
    assert!(
        events.iter().any(|e| matches!(e,
            Event::StepAwaitingGate { gate_kind: GateKind::Decision, suggestion, .. }
                if suggestion.contains("权限请求"))),
        "ask 策略必须弹 Decision 门: {events:?}"
    );
}

#[test]
fn acp_default_reject_policy_never_opens_gate() {
    let command = mock_command().replace("MOCK_ACP_SCENARIO=happy", "MOCK_ACP_SCENARIO=permission_probe");
    let yaml = format!(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: mock\n    command: \"{command}\"\n    prompt: go\n"
    );
    let (status, events) = run_manifest(&yaml);
    assert!(matches!(status, RunStatus::Success), "{events:?}");
    assert!(
        !events.iter().any(|e| matches!(e, Event::StepAwaitingGate { .. })),
        "缺省 reject 不得弹任何门: {events:?}"
    );
}
```

- [ ] **Step 2: 跑测确认失败**

Run: `cargo test -p agentpipe-engine acp_on_permission -- --nocapture`
Expected: FAIL（on_permission 字段不存在 → YAML unknown field）

- [ ] **Step 3: 实现**

manifest.rs 新增枚举（OnUnmet 旁）+ Acp 字段：

```rust
/// acp 反向权限请求策略:reject = 一律拒(缺省,fail-closed 现状);
/// ask = 经 GateKind::Decision 决策门问宿主。见 acp-hardening spec D3。
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum PermissionPolicy {
    #[default]
    Reject,
    Ask,
}
```

```rust
#[serde(default)]
on_permission: PermissionPolicy,
```

executor.rs：`VerifiedWork::Acp` 加 `on_permission: PermissionPolicy`；helper 的 Acp attempt 分支改为（块作用域让回调借用在 charge 前结束）：

```rust
VerifiedWork::Acp { agent, command, on_permission, .. } => {
    let runner = crate::runner::acp::AcpRunner::new(crate::runner::acp::AcpConfig {
        agent: (*agent).to_string(),
        command: (*command).to_string(),
    });
    let result = {
        use crate::runner::acp::{PermissionDecision, PermissionMode};
        let mut ask_cb;
        let permission = match on_permission {
            PermissionPolicy::Reject => PermissionMode::Reject,
            PermissionPolicy::Ask => {
                ask_cb = |desc: &str| {
                    let suggestion = format!(
                        "acp step 权限请求:{desc}。批准=允许一次 / 跳过=拒绝该请求 / 中止=终止 run"
                    );
                    match self.decision_gate(step_id, suggestion) {
                        StepDecision::Retry => PermissionDecision::Approve,
                        StepDecision::Skip => PermissionDecision::RejectOnce,
                        StepDecision::Abort => PermissionDecision::Abort,
                    }
                };
                PermissionMode::Ask(&mut ask_cb)
            }
        };
        runner.run(&p, Some(self.control.as_ref()), &mut on_line, &self.ctx.cwd, permission)
    };
    result.map(|out| (out.answer, out.metrics)).map_err(|e| e.to_string())
}
```

（`decision_gate` 是 `&self`，闭包只持不可变借用，与 `runner.run` 的 `&self.ctx.cwd` 共存；块结束后借用释放，后续 `self.charge(&mut self)` 合法。`decision_gate` 的 Abort 分支已自带 `request_abort`——runner 主循环下一轮检查 `is_aborted` 即 notify，与 Task 7 的 Abort 路径汇合。）

Acp match arm 传字段：`VerifiedWork::Acp { agent, command: cmd, prompt, on_permission: *on_permission }`。

- [ ] **Step 4: 验证**

Run: `cargo test --workspace` → PASS
Run: `cargo clippy --workspace --all-targets -- -D warnings` → 零告警
手工冒烟（可选）：`agentpipe validate` 一个带 `on_permission: ask` 的 task.yaml → 通过；CLI headless（stdin 关闭)下 ask 策略权限门走 EOF→Abort 既有语义，无需新代码。

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/manifest.rs crates/engine/src/executor.rs crates/engine/tests/acp_verify.rs
git commit -m "feat(engine): D3 收尾 — acp on_permission: reject|ask,ask 经 Decision 决策门(缺省 reject 不变)"
```

---

## 收尾核对（全部 task 完成后）

- `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 全绿。
- README.md 的 Step types 表 acp 行补一句：可带 verify / on_permission / command 可由 agents.toml 解析（文档改动并入最后一个 commit 或单独 docs commit）。
- 对照 spec §4 D1-D5 逐条勾验收；§5 错误路径逐条有测试或有明确的"由既有机制覆盖"结论。
- spec §6 的"demo stub 流程加带 verify 的 ACP step 场景"：端到端验收已由 `tests/acp_verify.rs`（真实 mock agent + 真实 executor）覆盖；demo/ GIF 场景属演示素材，是否补由用户在验收时定夺，不算本计划的完工门槛。
