# Review Loop 收敛加固 — 执行计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 docs/specs/2026-07-02-review-loop-hardening-design.md:loop max fail-closed(P1)、severity 阈值收敛(P2)、findings 自反驳核验(P3)、轮间记忆插值(P4)、收敛短路(P6)。

**Architecture:** 全部收在 engine 层(executor / codex runner / manifest / context / protocol),新 manifest 字段全 optional 向后兼容;P1/P6 是 run_loop 控制流的有意行为变更(bug 修复,无开关)。CLI/GUI 只做渲染镜像。

**Tech Stack:** Rust(cargo workspace:agentpipe-engine / agentpipe-cli)、Tauri + React + vitest(ui/)、bash stub(tests/fixtures/)。

## Global Constraints

- spec 是唯一语义来源:docs/specs/2026-07-02-review-loop-hardening-design.md;与本计划冲突时以 spec 为准并回改计划。
- 每个 Task 结束:`cargo test --workspace` 全绿 + `cargo clippy --workspace --all-targets -- -D warnings` 0 warning,才允许 commit。
- 新 manifest 字段一律 `#[serde(default, skip_serializing_if = ...)]`(存量 YAML / composer 序列化零 diff)。
- fail-closed 底线:解析失败 / 未知值 / 核验失败,一律走保守分支(不收敛 / 按 critical / 保留原 findings)。
- 注释风格对齐仓库现状:中文、写"为什么"(设计裁决/防回退),不写"下一行做什么"。
- 提交信息中文,一个 Task 一个 commit。
- 测试用 env var 必须走 `ENV_LOCK` + `EnvGuard`(见 crates/engine/tests/executor_test.rs 顶部,防跨测试污染)。

---

### Task 1: 数据基座 — Severity / FindingItem / items / schema enum / LoopConverged.residual

**Files:**
- Modify: `crates/engine/src/context.rs`(Severity 枚举 + StepOutput.items)
- Modify: `crates/engine/src/protocol.rs`(FindingItem + ReviewResult.items + LoopConverged.residual)
- Modify: `crates/engine/src/runner/codex.rs`(schema severity enum + raw_to_result 产 items + parse_review 拆 Option 变体)
- Modify: `crates/engine/src/executor.rs`(codex 分支 record items;LoopConverged emit 处补 residual: 0 占位,Task 2 换真值)
- Test: `crates/engine/src/runner/codex.rs` 内联 `#[cfg(test)]`、`crates/engine/tests/protocol_serde_test.rs`

**Interfaces:**
- Produces(后续 Task 依赖,签名照抄):
  - `context::Severity`:`enum Severity { Nit, Minor, Major, Critical }`,derive `Ord`(声明序 = 严重度升序,`sev <= threshold` 即"不严于阈值"),`Severity::parse_lossy(s: &str) -> Severity`(未知 → Critical)。
  - `protocol::FindingItem { severity: Severity, file: String, line: i64, summary: String, suggestion: String }`。
  - `ReviewResult.items: Vec<FindingItem>`、`StepOutput.items: Vec<FindingItem>`。
  - `Event::LoopConverged { loop_id, iterations, residual: u32 }`(serde default)。
  - codex.rs 私有 `parse_review_file(out_file: &Path) -> Option<ReviewResult>`(Task 4 vet 用,无 fallback 包装)。

- [ ] **Step 1: context.rs 加 Severity + StepOutput.items**

```rust
/// finding 严重度。声明序 = 严重度升序,derive Ord 后 `sev <= threshold`
/// 自然表达"不严于阈值"(allow_residual 收敛判定用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Nit,
    Minor,
    Major,
    Critical,
}

impl Severity {
    /// 未知字符串 → Critical(fail-closed:阻塞收敛)。REVIEW_SCHEMA enum 已约束
    /// 真实 codex 只能输出四值;这里兜 stub / 旧二进制 / 手写 fixture 的任意串。
    pub fn parse_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "nit" => Self::Nit,
            "minor" => Self::Minor,
            "major" => Self::Major,
            "critical" => Self::Critical,
            _ => Self::Critical,
        }
    }
}
```

StepOutput 加字段(derive 不变,Vec 有 Default):

```rust
#[derive(Debug, Default, Clone)]
pub struct StepOutput {
    pub artifact: Option<String>,
    pub findings: Option<String>,
    pub verdict: Option<Verdict>,
    /// 结构化 findings(severity 收敛判定用)。只有 codex step 成功解析时非空;
    /// 解析 fallback / claude / human step 恒空。内部类型,不进 NDJSON。
    pub items: Vec<crate::protocol::FindingItem>,
}
```

注意 `StepOutput::field()` **不加** `items` 分支(插值面不暴露结构化数据,渲染串走 `findings`)。

- [ ] **Step 2: protocol.rs 加 FindingItem + ReviewResult.items + LoopConverged.residual**

```rust
/// 单条结构化 finding(engine 内部类型,不进 NDJSON 事件协议)。
/// severity 用 context::Severity(未知串在 codex runner 侧已 parse_lossy 归一)。
#[derive(Debug, Clone)]
pub struct FindingItem {
    pub severity: crate::context::Severity,
    pub file: String,
    pub line: i64,
    pub summary: String,
    pub suggestion: String,
}
```

ReviewResult 加 `pub items: Vec<FindingItem>`;LoopConverged 变为:

```rust
LoopConverged {
    loop_id: String,
    iterations: u32,
    /// 带残留收敛(allow_residual)时的遗留 finding 数;verdict-clean 收敛通常为 0。
    /// serde default 兼容老审计日志(缺字段 → 0,回放语义不变)。
    #[serde(default)]
    residual: u32,
},
```

- [ ] **Step 3: codex.rs — schema enum + items 填充 + parse_review_file 拆分**

REVIEW_SCHEMA 的 severity 行改为:

```
"severity":{"type":"string","enum":["critical","major","minor","nit"]},"file":{"type":"string"},
```

raw_to_result 产 items(suggestion 存原始串,渲染归一只在 render_finding):

```rust
fn raw_to_result(raw: RawReview) -> ReviewResult {
    let verdict = if raw.verdict == "clean" { Verdict::Clean } else { Verdict::ChangesRequested };
    let items = raw.findings.iter().map(|f| FindingItem {
        severity: Severity::parse_lossy(&f.severity),
        file: f.file.clone(),
        line: f.line,
        summary: f.summary.clone(),
        suggestion: f.suggestion.clone(),
    }).collect();
    let findings = raw.findings.iter().map(render_finding).collect::<Vec<_>>().join("\n");
    ReviewResult { verdict, findings, items, metrics: None }
}
```

parse_review 拆出无 fallback 的 Option 变体(Task 4 vet 依赖,解析失败必须可判别):

```rust
/// 读 -o 文件解析;不可读 / 不可解析 → None(caller 决定 fallback 语义)。
fn parse_review_file(out_file: &Path) -> Option<ReviewResult> {
    let content = std::fs::read_to_string(out_file).ok()?;
    serde_json::from_str::<RawReview>(content.trim()).ok().map(raw_to_result)
}

fn parse_review(out_file: &Path) -> ReviewResult {
    parse_review_file(out_file).unwrap_or_else(|| ReviewResult {
        verdict: Verdict::ChangesRequested,
        findings: "(无法解析 Codex 输出,按需修改处理)".into(),
        items: vec![],
        metrics: None,
    })
}
```

- [ ] **Step 4: executor.rs 两处最小同步**

codex 分支 record 带 items(`StepKind::Codex` 的 `Ok(out)` 臂):

```rust
self.ctx.record(&step.id, StepOutput {
    findings: Some(out.findings),
    verdict: Some(out.verdict),
    items: out.items,
    ..Default::default()
});
```

(不同字段各自 partial move 合法;`summary` 在此之前已借用过 `out.verdict`,顺序不能倒。)run_loop 的 LoopConverged emit 处补 `residual: 0`(占位,Task 2 换 residual_count)。verify_once 的 `Ok(out) => (out.verdict, out.findings, out.metrics)` 不动(verify 门不消费 items)。

**LoopConverged 加字段的全 workspace 编译波及**(workspace members 含 src-tauri):

- `crates/cli/src/render.rs` 的 `Event::LoopConverged { loop_id, iterations }` 模式缺字段即 E0027,本 Task 先补成 `Event::LoopConverged { loop_id, iterations, .. }` 保编译,Task 6 再实现渲染。
- src-tauri 只对 `RunStarted / RunFinished / StepFinished` 做模式匹配(bridge.rs / commands.rs,已核),LoopConverged 不受影响,零改动。

- [ ] **Step 5: 单测(codex.rs 内联 #[cfg(test)] mod)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Severity;

    fn raw(verdict: &str, sev: &str) -> RawReview {
        RawReview {
            verdict: verdict.into(),
            findings: vec![RawFinding {
                severity: sev.into(), file: "a.rs".into(), line: 1,
                summary: "s".into(), suggestion: "do x".into(),
            }],
        }
    }

    #[test]
    fn items_map_known_severities() {
        for (s, want) in [("nit", Severity::Nit), ("minor", Severity::Minor),
                          ("MAJOR", Severity::Major), ("critical", Severity::Critical)] {
            let r = raw_to_result(raw("changes_requested", s));
            assert_eq!(r.items[0].severity, want, "severity 串 {s}");
        }
    }

    #[test]
    fn unknown_severity_fail_closed_critical() {
        // stub / 旧二进制的 "high" 一类未知串必须按最严处理,不给 allow_residual 放行机会
        let r = raw_to_result(raw("changes_requested", "high"));
        assert_eq!(r.items[0].severity, Severity::Critical);
    }

    #[test]
    fn severity_ord_matches_threshold_semantics() {
        assert!(Severity::Nit < Severity::Minor);
        assert!(Severity::Minor < Severity::Major);
        assert!(Severity::Major < Severity::Critical);
    }
}
```

- [ ] **Step 6: protocol_serde_test.rs 加 LoopConverged 向后兼容测试**

```rust
#[test]
fn loop_converged_without_residual_defaults_zero() {
    // 老审计日志无 residual 字段,回放必须解析成功且语义不变
    let old = r#"{"type":"LoopConverged","loop_id":"l","iterations":2}"#;
    let e: Event = serde_json::from_str(old).unwrap();
    match e {
        Event::LoopConverged { residual, iterations, .. } => {
            assert_eq!(residual, 0);
            assert_eq!(iterations, 2);
        }
        other => panic!("expected LoopConverged, got {other:?}"),
    }
}
```

- [ ] **Step 7: 编译 + 全测 + clippy**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 全绿。存量 codex 流程测试(fixture stub severity="high")不受影响 —— 它们只断言 verdict / findings 串。

- [ ] **Step 8: Commit**

```bash
git add crates/engine
git commit -m "feat(engine): 结构化 findings 基座 — Severity/FindingItem/items + schema severity enum + LoopConverged.residual"
```

---

### Task 2: run_loop 重构 — P1 MaxReached 决策门 + P6 锚点收敛短路

**Files:**
- Modify: `crates/engine/src/executor.rs`(run_loop 整体重写 + emit_skipped_with + residual_count)
- Test: `crates/engine/tests/executor_test.rs`

**Interfaces:**
- Consumes: Task 1 的 `StepOutput.items`、`LoopConverged.residual`。
- Produces: 行为契约 —— 自然耗尽 max 后必有 `StepAwaitingGate{gate_kind: Decision, step_id: <loop_id>}`;Retry 续号;Skip 发 `StepFinished{Skipped}`;锚点(body 最后一个 codex step)判 clean 后其后 body step 全部 `StepFinished{Skipped, summary:"loop 已收敛,跳过"}`。

- [ ] **Step 1: 先写测试(预期编译过、断言失败/挂死前先确认现状)**

在 executor_test.rs 追加。**注意**:P1 后 MaxReached 会阻塞等命令,所有打到 max 的测试必须预先向命令信道灌入决策(mpsc 无界,先 send 后 run 即可):

```rust
/// P1:自然耗尽 max → 决策门;预灌 Skip → StepFinished{Skipped} + run Success。
#[test]
fn loop_max_gates_and_skip_continues() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
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
        base: HEAD
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx, crx) = mpsc::channel::<Command>();
    ctx.send(Command::SkipStep { step_id: "fixloop".into() }).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    let status = ex.run();
    assert_eq!(status, RunStatus::Success);
    let events: Vec<Event> = erx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(e,
        Event::StepAwaitingGate { step_id, gate_kind: agentpipe_engine::protocol::GateKind::Decision, .. }
            if step_id == "fixloop")),
        "耗尽 max 必须弹决策门");
    assert!(events.iter().any(|e| matches!(e,
        Event::StepFinished { step_id, status: StepStatus::Skipped, .. } if step_id == "fixloop")),
        "Skip 必须留审计痕");
}

/// P1:预灌 Abort → RunStatus::Aborted。
#[test]
fn loop_max_gate_abort_aborts_run() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
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
        base: HEAD
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx, crx) = mpsc::channel::<Command>();
    ctx.send(Command::Abort).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    assert_eq!(ex.run(), RunStatus::Aborted);
    let _ = erx; // 事件断言可省:状态即契约
}

/// P1:Approve(Retry)→ 再跑 max 轮且 iteration 续号(1,2,3,4),第二次门 Skip 收尾。
#[test]
fn loop_max_gate_retry_continues_iteration_numbering() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
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
        base: HEAD
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx, crx) = mpsc::channel::<Command>();
    ctx.send(Command::ApproveGate { step_id: "fixloop".into(), artifact: None }).unwrap();
    ctx.send(Command::SkipStep { step_id: "fixloop".into() }).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    assert_eq!(ex.run(), RunStatus::Success);
    let iters: Vec<u32> = erx.try_iter().filter_map(|e| match e {
        Event::LoopIteration { iteration, .. } => Some(iteration),
        _ => None,
    }).collect();
    assert_eq!(iters, vec![1, 2, 3, 4], "Retry 后编号必须续 3,4 而非重开 1,2");
}

/// P6:review 首轮即 clean → fix 被跳过(Skipped 且非 Started),LoopConverged 在场。
#[test]
fn loop_short_circuits_body_after_anchor_clean() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "clean");
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
        base: HEAD
      - id: fix
        kind: claude
        prompt: "修 {{rev.findings}}"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    assert_eq!(ex.run(), RunStatus::Success);
    let events: Vec<Event> = erx.try_iter().collect();
    assert!(!events.iter().any(|e| matches!(e,
        Event::StepStarted { step_id, .. } if step_id == "fix")),
        "收敛轮 fix 不得启动(P6 主修:不再空烧一次)");
    assert!(events.iter().any(|e| matches!(e,
        Event::StepFinished { step_id, status: StepStatus::Skipped, .. } if step_id == "fix")));
    assert!(events.iter().any(|e| matches!(e, Event::LoopConverged { iterations: 1, .. })));
}
```

- [ ] **Step 2: 跑新测试确认失败形态**

Run: `cargo test -p agentpipe-engine --test executor_test loop_max_gates -- --nocapture`
Expected: FAIL(现实现 MaxReached 不弹门;短路测试里 fix 有 StepStarted)。**若测试挂死而非失败,直接进 Step 3**(现实现无 gate,不应挂;挂死说明 yaml 写错)。

- [ ] **Step 3: 重写 run_loop + helper**

executor.rs 替换 run_loop 与 emit_skipped:

```rust
fn run_loop(&mut self, loop_id: &str, until: &str, max: u32, body: &[Step], gated: bool) -> Result<(), ()> {
    // 锚点 = body 最后一个 codex step(until: codex-clean 的收敛信号源,validate 已保证存在)。
    // 锚点跑完立即判收敛(P6):收敛则跳过其后 body step,不再空烧一轮 fix。
    // 锚点之前的 codex step 不判 —— 那时锚点在 ctx 里的 verdict 是上一轮残留,读了是脏值。
    let anchor = body.iter().rposition(|s| matches!(s.kind, StepKind::Codex { .. }));
    let mut base = 0u32; // P1 Retry 续号 offset:人工再批一份 max 预算,round 编号不重开
    loop {
        for n in (base + 1)..=(base + max) {
            if self.control.is_aborted() {
                let _ = self.events.send(Event::LoopMaxReached {
                    loop_id: loop_id.into(),
                    max: n.saturating_sub(1),
                    reason: LoopEndReason::Aborted,
                });
                return Err(());
            }
            let _ = self.events.send(Event::LoopIteration { loop_id: loop_id.into(), iteration: n });
            for (i, sub) in body.iter().enumerate() {
                if self.run_step(sub, gated).is_err() {
                    let _ = self.events.send(Event::LoopMaxReached {
                        loop_id: loop_id.into(),
                        max: n,
                        reason: LoopEndReason::SubStepFailed,
                    });
                    return Err(());
                }
                if Some(i) == anchor && self.eval_until(until, body) {
                    for skipped in &body[i + 1..] {
                        self.emit_skipped_with(&skipped.id, "loop 已收敛,跳过");
                    }
                    let _ = self.events.send(Event::LoopConverged {
                        loop_id: loop_id.into(),
                        iterations: n,
                        residual: self.residual_count(body),
                    });
                    return Ok(());
                }
            }
        }
        // P1:自然耗尽 max —— fail-closed 过决策门,绝不静默继续(README 既有承诺落地)。
        // 门 suggestion 附锚点末轮 findings,让人在门上直接看到"还剩什么没修"再决策。
        let _ = self.events.send(Event::LoopMaxReached {
            loop_id: loop_id.into(),
            max: base + max,
            reason: LoopEndReason::MaxReached,
        });
        let findings = anchor
            .and_then(|i| self.ctx.get(&body[i].id))
            .and_then(|o| o.findings.clone())
            .unwrap_or_default();
        let suggestion = format!(
            "loop 跑满 {} 轮仍未收敛,选择 重试(再跑 {max} 轮)/ 跳过 / 中止\n{findings}",
            base + max
        );
        match self.decision_gate(loop_id, suggestion) {
            StepDecision::Retry => {
                base += max;
                continue;
            }
            StepDecision::Skip => {
                self.emit_skipped(loop_id);
                return Ok(());
            }
            StepDecision::Abort => return Err(()),
        }
    }
}

/// 锚点 step 当前 items 数(LoopConverged.residual)。verdict-clean 收敛通常 0;
/// allow_residual 收敛(Task 3)时为遗留 finding 数。
fn residual_count(&self, body: &[Step]) -> u32 {
    body.iter().rev()
        .find(|s| matches!(s.kind, StepKind::Codex { .. }))
        .and_then(|s| self.ctx.get(&s.id))
        .map(|o| o.items.len() as u32)
        .unwrap_or(0)
}

fn emit_skipped_with(&self, step_id: &str, summary: &str) {
    let _ = self.events.send(Event::StepFinished {
        step_id: step_id.to_string(),
        status: StepStatus::Skipped,
        summary: summary.into(),
        metrics: None,
    });
}

fn emit_skipped(&self, step_id: &str) {
    self.emit_skipped_with(step_id, "skipped");
}
```

- [ ] **Step 4: 修被 P1 行为变更波及的存量测试**

两个测试现会在决策门上 recv 阻塞(测试持有未 drop 的 sender → 永久挂死),必须预灌命令:

1. `loop_hits_max_when_never_clean`:`let (_c, crx)` 改 `let (ctx, crx)`,run 前 `ctx.send(Command::SkipStep { step_id: "fixloop".into() }).unwrap();`;既有断言(LoopMaxReached{max:2}、loop 不发 StepStarted)保持不变。
2. `loop_max_reached_carries_distinct_reason_for_each_termination_path`(executor_test.rs:698 起):检查其自然-max 分支,同样预灌 Skip;Aborted / SubStepFailed 两分支不走新门,不用动。

Run: `cargo test -p agentpipe-engine --test executor_test -- --nocapture`(**先单跑这两个名字确认不挂死**)
Expected: PASS。

- [ ] **Step 5: 全测 + clippy**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 全绿。

- [ ] **Step 6: Commit**

```bash
git add crates/engine
git commit -m "fix(engine): loop 耗尽 max 走决策门(fail-closed)+ 锚点收敛短路,收敛轮不再空烧 fix"
```

---

### Task 3: P2 allow_residual — severity 阈值收敛

**Files:**
- Modify: `crates/engine/src/manifest.rs`(Loop.allow_residual + validate)
- Modify: `crates/engine/src/executor.rs`(eval_until 签名 + 判定)
- Test: `crates/engine/tests/manifest_test.rs`、`crates/engine/tests/executor_test.rs`
- Modify: `tests/fixtures/stub-codex.sh`(STUB_SEVERITY 可控)

**Interfaces:**
- Consumes: Task 1 `Severity`(Ord)、`StepOutput.items`;Task 2 的 run_loop 结构。
- Produces: `Loop { until, max, allow_residual: Option<Severity>, body }`;`eval_until(&self, until: &str, body: &[Step], allow_residual: Option<Severity>) -> bool`。

- [ ] **Step 1: stub 扩展 severity 可控**

tests/fixtures/stub-codex.sh 的 verdict 行后加一行,findings 里引用:

```bash
verdict="${STUB_VERDICT:-changes_requested}"
severity="${STUB_SEVERITY:-high}"
findings="${STUB_FINDINGS:-[{\"severity\":\"$severity\",\"file\":\"a.rs\",\"line\":10,\"summary\":\"示例问题\",\"suggestion\":\"N/A\"}]}"
cat > "$out" <<EOF
{"verdict":"$verdict","findings":$findings}
EOF
echo "stub codex done"
```

默认 "high" 不变(存量测试零影响;"high" 经 parse_lossy → Critical,正好当"未知串阻塞"用例)。`STUB_FINDINGS='[]'` 用于构造 "items 空 + changes_requested" 的解析-fallback 同形状(spec §7 fail-closed 用例)。

- [ ] **Step 2: 写失败测试**

manifest_test.rs:

```rust
#[test]
fn loop_allow_residual_parses_and_rejects_critical() {
    let ok = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: l\n    kind: loop\n    until: codex-clean\n    max: 2\n    allow_residual: minor\n    body:\n      - id: r\n        kind: codex\n        action: review-mr\n        base: HEAD\n";
    let m = Manifest::parse(ok).unwrap();
    assert!(m.validate().is_ok());
    let bad = ok.replace("allow_residual: minor", "allow_residual: critical");
    let m = Manifest::parse(&bad).unwrap();
    let err = m.validate().unwrap_err();
    assert!(err.to_string().contains("allow_residual"), "err = {err}");
}

#[test]
fn loop_allow_residual_absent_not_serialized() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: l\n    kind: loop\n    until: codex-clean\n    max: 2\n    body:\n      - id: r\n        kind: codex\n        action: review-mr\n        base: HEAD\n";
    let m = Manifest::parse(y).unwrap();
    let out = serde_yml::to_string(&m).unwrap();
    assert!(!out.contains("allow_residual"), "缺省不应序列化:\n{out}");
}
```

executor_test.rs(核心三态:阈值内收敛 / 超阈值不收敛 / 解析失败 fail-closed。第三态用 STUB_VERDICT=changes_requested + 让 stub 输出坏 JSON 不好做 —— 改用"未知 severity=high→Critical 阻塞"覆盖 fail-closed 面,items-空的 fallback 形状由 eval_until 单元逻辑保证,见 Step 3 注释):

```rust
/// P2:全 minor findings + allow_residual: minor → 带残留收敛,residual=1。
#[test]
fn loop_converges_with_residual_below_threshold() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
    let _s = EnvGuard::set("STUB_SEVERITY", "minor");
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
    allow_residual: minor
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: HEAD
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (_c, crx) = mpsc::channel::<Command>();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    assert_eq!(ex.run(), RunStatus::Success);
    let events: Vec<Event> = erx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(e,
        Event::LoopConverged { iterations: 1, residual: 1, .. })),
        "minor ≤ minor 应第 1 轮带残留收敛: {events:?}");
}

/// P2:severity=high(未知串 → Critical)超过 minor 阈值 → 不收敛,耗尽 max 走门。
#[test]
fn loop_blocks_when_severity_above_threshold() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
    let _s = EnvGuard::set("STUB_SEVERITY", "high"); // 未知串,fail-closed → Critical
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
    allow_residual: minor
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: HEAD
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx, crx) = mpsc::channel::<Command>();
    ctx.send(Command::SkipStep { step_id: "fixloop".into() }).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    assert_eq!(ex.run(), RunStatus::Success);
    let events: Vec<Event> = erx.try_iter().collect();
    assert!(!events.iter().any(|e| matches!(e, Event::LoopConverged { .. })));
    assert!(events.iter().any(|e| matches!(e,
        Event::LoopMaxReached { reason: LoopEndReason::MaxReached, .. })));
}

/// P2 fail-closed:items 空 + changes_requested(= 解析 fallback 的形状)必须不收敛,
/// 即便配了 allow_residual —— 放行等于把"无法解析 Codex 输出"判过。
#[test]
fn loop_never_converges_on_empty_items_with_changes_requested() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
    let _f = EnvGuard::set("STUB_FINDINGS", "[]");
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
    allow_residual: major
    body:
      - id: rev
        kind: codex
        action: review-mr
        base: HEAD
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx, crx) = mpsc::channel::<Command>();
    ctx.send(Command::SkipStep { step_id: "fixloop".into() }).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    assert_eq!(ex.run(), RunStatus::Success);
    let events: Vec<Event> = erx.try_iter().collect();
    assert!(!events.iter().any(|e| matches!(e, Event::LoopConverged { .. })),
        "items 空 + 非 clean 绝不能收敛(fail-closed)");
}
```

Run: `cargo test -p agentpipe-engine allow_residual`
Expected: FAIL(字段不存在,编译错)。

- [ ] **Step 3: 实现**

manifest.rs Loop variant:

```rust
Loop {
    until: String,
    max: u32,
    /// 可选 severity 阈值收敛:findings 全部 ≤ 此级别时视同收敛(residual 保留不修)。
    /// 缺省 = 只认 verdict clean。取值 nit|minor|major;critical 被 validate 拒绝。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allow_residual: Option<crate::context::Severity>,
    body: Vec<Step>,
},
```

validate 的 Loop 臂加:

```rust
StepKind::Loop { body, until, allow_residual, .. } => {
    // ...现有 until / body 校验不动...
    if matches!(allow_residual, Some(crate::context::Severity::Critical)) {
        return Err(EngineError::Validation(format!(
            "step '{}': allow_residual 不能为 critical(残留 critical 无意义,收敛即放行严重缺陷)",
            step.id
        )));
    }
    // ...
}
```

executor.rs:run_step 的 Loop 臂解构补 `allow_residual`,透传 run_loop;run_loop 签名加 `allow_residual: Option<Severity>`,两处 `self.eval_until(until, body)` 改 `self.eval_until(until, body, allow_residual)`;eval_until 改为:

```rust
/// 收敛判定。verdict clean 恒收敛;配置 allow_residual 时,findings 全部 ≤ 阈值
/// 也算收敛(带残留)。items 空 + 非 clean 是解析 fallback 的形状 —— fail-closed
/// 不收敛(放行等于把"无法解析 Codex 输出"判过)。
fn eval_until(&self, until: &str, body: &[Step], allow_residual: Option<Severity>) -> bool {
    if until != "codex-clean" {
        return false;
    }
    for sub in body.iter().rev() {
        if matches!(sub.kind, StepKind::Codex { .. }) {
            if let Some(out) = self.ctx.get(&sub.id) {
                if matches!(out.verdict, Some(Verdict::Clean)) {
                    return true;
                }
                if let Some(t) = allow_residual {
                    return !out.items.is_empty() && out.items.iter().all(|i| i.severity <= t);
                }
                return false;
            }
        }
    }
    false
}
```

(import:`use crate::context::{..., Severity};`)

- [ ] **Step 4: 跑测 + clippy**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 全绿(含 Task 2 的门/短路测试:eval_until 改签名后其调用点同步)。

- [ ] **Step 5: Commit**

```bash
git add crates/engine tests/fixtures/stub-codex.sh
git commit -m "feat(engine): loop 支持 allow_residual severity 阈值收敛(fail-closed:未知/解析失败不放行)"
```

---

### Task 4: P3 vet — codex findings 自反驳核验

**Files:**
- Modify: `crates/engine/src/manifest.rs`(Codex.vet + validate 拒 ask+vet)
- Modify: `crates/engine/src/runner/codex.rs`(review 签名加 vet + vet_pass)
- Modify: `crates/engine/src/executor.rs`(两处 codex.review 调用点)
- Modify: `tests/fixtures/stub-codex.sh`(调用计数 + 第 2 次调用行为)
- Test: `crates/engine/tests/codex_runner_test.rs`、`crates/engine/tests/manifest_test.rs`

**Interfaces:**
- Consumes: Task 1 `parse_review_file`、`ReviewResult.items`。
- Produces: `CodexRunner::review(&self, action, doc_path, base, ask_prompt, vet: bool, control, on_progress, cwd)`(vet 参数插在 ask_prompt 后);manifest `Codex { ..., vet: bool }`。

- [ ] **Step 1: stub 加调用计数(默认零行为变化)**

tests/fixtures/stub-codex.sh 的变量段改为如下**最终合成态**(注意保留 Task 3 加的 `STUB_FINDINGS` 覆盖能力,不得写死 findings 把它抹掉 —— 否则 Task 3 的空 items 测试回归失败):

```bash
verdict="${STUB_VERDICT:-changes_requested}"
severity="${STUB_SEVERITY:-high}"
findings="${STUB_FINDINGS:-[{\"severity\":\"$severity\",\"file\":\"a.rs\",\"line\":10,\"summary\":\"示例问题\",\"suggestion\":\"N/A\"}]}"
# vet 测试用调用计数:STUB_COUNT_FILE 未设时 n=1,存量测试零影响。
n=1
if [ -n "${STUB_COUNT_FILE:-}" ]; then
  n=$(( $(cat "$STUB_COUNT_FILE" 2>/dev/null || echo 0) + 1 ))
  echo "$n" > "$STUB_COUNT_FILE"
fi
if [ "$n" -ge 2 ]; then
  if [ "${STUB_FAIL_ON_CALL_2:-}" = "1" ]; then echo "vet boom" >&2; exit 1; fi
  verdict="${STUB_VERDICT_2:-clean}"
  findings="${STUB_FINDINGS_2:-[]}"
fi
cat > "$out" <<EOF
{"verdict":"$verdict","findings":$findings}
EOF
```

- [ ] **Step 2: 写失败测试**

manifest_test.rs:

```rust
#[test]
fn codex_vet_rejects_ask_action() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: q\n    kind: codex\n    action: ask\n    prompt: \"hi\"\n    vet: true\n";
    let m = Manifest::parse(y).unwrap();
    let err = m.validate().unwrap_err();
    assert!(err.to_string().contains("vet"), "err = {err}");
}

#[test]
fn codex_vet_absent_not_serialized() {
    let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: r\n    kind: codex\n    action: review-mr\n    base: HEAD\n";
    let m = Manifest::parse(y).unwrap();
    let out = serde_yml::to_string(&m).unwrap();
    assert!(!out.contains("vet"), "false 不应序列化:\n{out}");
}
```

codex_runner_test.rs(先看文件现有 helper 复用其构造方式;核心三用例):

```rust
/// vet 开启 + 首轮 changes_requested → 第 2 次调用(vet)返回 clean 空 findings,
/// 结果被替换 → verdict clean。
#[test]
fn vet_replaces_result_when_all_refuted() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
    let count = std::env::temp_dir().join(format!("ap-vet-count-{}", std::process::id()));
    let _ = std::fs::remove_file(&count);
    let _c = EnvGuard::set("STUB_COUNT_FILE", count.to_str().unwrap());
    let runner = CodexRunner::new(fixture("stub-codex.sh"));
    let r = runner.review(&CodexAction::ReviewMr, None, Some("HEAD"), None, true,
                          None, &mut |_l, _r| {}, Path::new(".")).unwrap();
    assert!(matches!(r.verdict, Verdict::Clean), "vet 全驳回应翻 clean");
    assert_eq!(std::fs::read_to_string(&count).unwrap().trim(), "2", "必须恰好调 2 次");
}

/// vet 调用失败(exit 1)→ 保留首轮结果(fail-closed 不丢 review 信号)。
#[test]
fn vet_failure_keeps_first_result() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
    let _f = EnvGuard::set("STUB_FAIL_ON_CALL_2", "1");
    let count = std::env::temp_dir().join(format!("ap-vet-fail-{}", std::process::id()));
    let _ = std::fs::remove_file(&count);
    let _c = EnvGuard::set("STUB_COUNT_FILE", count.to_str().unwrap());
    let runner = CodexRunner::new(fixture("stub-codex.sh"));
    let r = runner.review(&CodexAction::ReviewMr, None, Some("HEAD"), None, true,
                          None, &mut |_l, _r| {}, Path::new(".")).unwrap();
    assert!(matches!(r.verdict, Verdict::ChangesRequested));
    assert!(r.findings.contains("示例问题"), "首轮 findings 必须保留");
}

/// clean 首轮不触发 vet(恰好 1 次调用)。
#[test]
fn vet_not_triggered_on_clean() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "clean");
    let count = std::env::temp_dir().join(format!("ap-vet-clean-{}", std::process::id()));
    let _ = std::fs::remove_file(&count);
    let _c = EnvGuard::set("STUB_COUNT_FILE", count.to_str().unwrap());
    let runner = CodexRunner::new(fixture("stub-codex.sh"));
    let _ = runner.review(&CodexAction::ReviewMr, None, Some("HEAD"), None, true,
                          None, &mut |_l, _r| {}, Path::new(".")).unwrap();
    assert_eq!(std::fs::read_to_string(&count).unwrap().trim(), "1");
}
```

(codex_runner_test.rs 若无 ENV_LOCK/EnvGuard/fixture helper,从 executor_test.rs 顶部复制同款;review-mr 走 base_ref_resolvable 预检,`base: HEAD` 在本仓 cwd `.` 可解析。)

Run: `cargo test -p agentpipe-engine vet`
Expected: FAIL(review 无 vet 参数,编译错)。

- [ ] **Step 3: 实现**

manifest.rs Codex variant 加:

```rust
Codex {
    action: CodexAction,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    /// 可选自反驳核验:review 结果非 clean 时追加一次 read-only codex 调用,
    /// 逐条用代码证据复核 findings,误报在喂给下游 fixer 前被过滤。
    /// 仅 review-mr / review-doc;ask 配 vet 被 validate 拒绝。
    #[serde(default, skip_serializing_if = "is_false")]
    vet: bool,
},
```

validate 的 Codex 臂加(在 validate_codex_fields 调用后):

```rust
StepKind::Codex { action, path, base, prompt, vet } => {
    Self::validate_codex_fields(&step.id, "codex", action, path, base, prompt)?;
    if *vet && *action == CodexAction::Ask {
        return Err(EngineError::Validation(format!(
            "step '{}': vet 仅支持 review-mr / review-doc(ask 无结构化 findings 可核)",
            step.id
        )));
    }
    Ok(())
}
```

codex.rs:review 签名在 `ask_prompt` 后加 `vet: bool`;函数末尾(emit_findings_summary 前)改:

```rust
let mut result = parse_review_stdout(&stdout).unwrap_or_else(|| parse_review(&out_file));

// P3(spec §3.3):自反驳核验。仅非 clean 且有结构化 items 时触发(fallback 路径
// items 空,无从核起,保留原样);核验失败保留首轮 —— fail-closed 方向是"不丢
// review 信号,宁可多修不可漏修"。
if vet && matches!(result.verdict, Verdict::ChangesRequested) && !result.items.is_empty() {
    on_progress("核验 findings(自反驳)…", None);
    match self.vet_pass(&result, control, on_progress, cwd) {
        Ok(vetted) => result = vetted,
        Err(e) => on_progress(&format!("核验失败,保留原 findings: {e}"), None),
    }
}

emit_findings_summary(&result, &mut |line| on_progress(line, None));
Ok(result)
```

vet_pass(CodexRunner impl 内):

```rust
/// 二次 read-only codex 调用:逐条复核首轮 findings。输出不可解析 → Err
/// (caller 保留首轮,不同于 review 主路径的 fallback-ChangesRequested 语义:
/// vet 的 fallback 若替换首轮,等于用"无法解析"占位符抹掉真实 findings)。
fn vet_pass(
    &self,
    first: &ReviewResult,
    control: Option<&Control>,
    on_progress: &mut dyn FnMut(&str, Option<u32>),
    cwd: &Path,
) -> Result<ReviewResult, EngineError> {
    let seq = OUT_SEQ.fetch_add(1, Ordering::Relaxed);
    let out_file = std::env::temp_dir()
        .join(format!("agentpipe-codex-vet-{}-{}.json", std::process::id(), seq));
    let out_str = out_file.to_string_lossy().to_string();
    let schema = write_schema()?;
    let prompt = format!(
        "以下是你刚对当前工作区给出的 code review findings。逐条重新到代码里核实:\
         尝试用具体代码证据反驳每一条;删除证据不足、误读或幻觉的条目,保留确认成立的。\
         驳回必须给出代码证据,不确定时保留。按 schema 重新输出最终 verdict 和 findings{SUGGESTION_HINT}\n\n{}",
        first.findings
    );
    let args: Vec<String> = vec![
        "exec".into(), "-s".into(), "read-only".into(),
        "--output-schema".into(), schema,
        "-o".into(), out_str,
        prompt,
    ];
    let mut raw_sink = |line: &str| on_progress(line, None);
    let started = std::time::Instant::now();
    let (stdout, success) = run_command(
        &self.bin, &args, cwd, None, Some(self.timeout_secs), control, &mut raw_sink,
    )?;
    if !success && started.elapsed() >= std::time::Duration::from_secs(self.timeout_secs) {
        return Err(EngineError::Cli(format!("Codex 核验超时(>{}s),已中止", self.timeout_secs)));
    }
    if !success {
        return Err(EngineError::Cli("Codex 核验进程非零退出".into()));
    }
    parse_review_stdout(&stdout)
        .or_else(|| parse_review_file(&out_file))
        .ok_or_else(|| EngineError::Cli("Codex 核验输出不可解析".into()))
}
```

executor.rs 两处调用点:`StepKind::Codex` 分支解构加 `vet`,`self.codex.review(action, path_i.as_deref(), base_i.as_deref(), prompt_i.as_deref(), *vet, ...)`;verify_once 的 Verifier::Codex 传 `false`(verify 门不做 vet,YAGNI,注释注明)。

**签名变更波及的存量调用点**:`crates/engine/tests/codex_runner_test.rs` 现有若干 `runner.review(...)` 调用(如 :318 附近)全部在 `ask_prompt` 参数后补 `false`,逐个 grep `\.review(` 核对,漏一个即编译红。

- [ ] **Step 4: 跑测 + clippy**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/engine tests/fixtures/stub-codex.sh
git commit -m "feat(engine): codex step 可选 vet 自反驳核验 — 误报在喂给 fixer 前过滤,核验失败保留原 findings"
```

---

### Task 5: P4 history — 轮间记忆插值 + 模板接入

**Files:**
- Modify: `crates/engine/src/context.rs`(histories + archive_findings + history + interpolate)
- Modify: `crates/engine/src/executor.rs`(codex 分支 record 前 archive)
- Modify: `templates/mr-review-loop.yaml`、`templates/full-pipeline.yaml`(fix prompt 加 history 段)
- Test: `crates/engine/tests/context_test.rs`、`crates/engine/tests/executor_test.rs`

**Interfaces:**
- Produces: `RunContext::archive_findings(&mut self, step_id: &str)`、`RunContext::history(&self, step_id: &str) -> Option<String>`;插值语法 `{{<id>.history}}`(此前轮次,不含当前轮;无历史 → 空串)。

- [ ] **Step 1: 写失败测试(context_test.rs)**

```rust
#[test]
fn history_accumulates_previous_findings_only() {
    let mut ctx = RunContext::new(PathBuf::from("."));
    // 第 1 轮:record 前 archive(无既有 findings → 无操作)
    ctx.archive_findings("rev");
    ctx.record("rev", StepOutput { findings: Some("第一轮问题".into()), ..Default::default() });
    assert_eq!(ctx.interpolate("{{rev.history}}"), "", "第 1 轮 history 必须为空");
    // 第 2 轮
    ctx.archive_findings("rev");
    ctx.record("rev", StepOutput { findings: Some("第二轮问题".into()), ..Default::default() });
    let h = ctx.interpolate("{{rev.history}}");
    assert!(h.contains("── 第 1 轮 ──") && h.contains("第一轮问题"), "h = {h}");
    assert!(!h.contains("第二轮问题"), "history 不含当前轮");
    // 第 3 轮:两段历史,带分隔
    ctx.archive_findings("rev");
    ctx.record("rev", StepOutput { findings: Some("第三轮问题".into()), ..Default::default() });
    let h = ctx.interpolate("{{rev.history}}");
    assert!(h.contains("── 第 2 轮 ──") && h.contains("第二轮问题"), "h = {h}");
}

#[test]
fn history_skips_empty_findings_and_unknown_step() {
    let mut ctx = RunContext::new(PathBuf::from("."));
    ctx.record("rev", StepOutput { findings: Some("  ".into()), ..Default::default() });
    ctx.archive_findings("rev"); // 空白 findings 不归档
    assert_eq!(ctx.interpolate("{{rev.history}}"), "");
    assert_eq!(ctx.interpolate("{{nobody.history}}"), "", "未知 step 与其他字段同语义:空串");
}
```

Run: `cargo test -p agentpipe-engine --test context_test history`
Expected: FAIL(方法不存在,编译错)。

- [ ] **Step 2: 实现 context.rs**

RunContext 加字段 `histories: HashMap<String, Vec<String>>`(new() 里 `HashMap::new()`);方法:

```rust
/// 把 step 现有非空 findings 归档进历史。executor 在 codex step record 新结果前调用;
/// 只有 loop 内同 id 重跑才会累积,直线 step 恒无历史。
pub fn archive_findings(&mut self, step_id: &str) {
    if let Some(f) = self.get(step_id).and_then(|o| o.findings.clone()) {
        if !f.trim().is_empty() {
            self.histories.entry(step_id.to_string()).or_default().push(f);
        }
    }
}

/// 渲染 step 的历史轮次 findings(不含当前轮);无历史 → None。
fn history(&self, step_id: &str) -> Option<String> {
    let h = self.histories.get(step_id)?;
    if h.is_empty() {
        return None;
    }
    Some(
        h.iter()
            .enumerate()
            .map(|(i, f)| format!("── 第 {} 轮 ──\n{f}", i + 1))
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}
```

interpolate 的取值闭包改(`history` 是 RunContext 级字段,不进 StepOutput::field):

```rust
let value = token
    .split_once('.')
    .and_then(|(id, field)| {
        if field == "history" {
            self.history(id)
        } else {
            self.get(id).and_then(|o| o.field(field))
        }
    })
    .unwrap_or_default();
```

- [ ] **Step 3: executor.rs codex 分支接入**

`StepKind::Codex` 的 `Ok(out)` 臂,`self.ctx.record(...)` 之前加一行:

```rust
// P4:record 覆盖前把上一轮 findings 归档,{{<id>.history}} 供 fix prompt 防振荡
self.ctx.archive_findings(&step.id);
```

- [ ] **Step 4: 端到端测试(executor_test.rs)**

```rust
/// P4 端到端:loop 第 2 轮 fix prompt 里 {{rev.history}} 展开为第 1 轮 findings。
/// 可观测面:tests/fixtures/stub-claude.sh 的 assistant 行回显 "STUB CLAUDE 收到: <prompt压扁>",
/// 经 StreamParser derive_label → StepProgress.line,但 label 截断 60 字符 ——
/// 所以 fix prompt 故意写短("H:{{rev.history}}"),保证 "第 1 轮" 落在截断窗口内。
#[test]
fn fix_prompt_receives_history_on_second_round() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _e = EnvGuard::set("STUB_VERDICT", "changes_requested");
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
        base: HEAD
      - id: fix
        kind: claude
        prompt: "H:{{rev.history}}"
"#;
    let m = Manifest::parse(yaml).unwrap();
    let (etx, erx) = mpsc::channel();
    let (ctx, crx) = mpsc::channel::<Command>();
    ctx.send(Command::SkipStep { step_id: "fixloop".into() }).unwrap();
    let mut ex = Executor::new(m, stub_bins(), test_control(), etx, crx);
    assert_eq!(ex.run(), RunStatus::Success);
    let fix_lines: Vec<String> = erx.try_iter().filter_map(|e| match e {
        Event::StepProgress { step_id, line, .. } if step_id == "fix" => Some(line),
        _ => None,
    }).collect();
    // 第 1 轮 history 空("STUB CLAUDE 收到: H:"),第 2 轮含第 1 轮 findings 头
    assert!(fix_lines.iter().any(|l| l.contains("第 1 轮")),
        "第 2 轮 fix prompt 必须展开 history,实际 progress 行: {fix_lines:?}");
}
```

- [ ] **Step 5: 模板更新**

先 Read 两个模板当前内容,再改。`templates/mr-review-loop.yaml` 的 fix prompt(保持 YAML 转义合法):

```yaml
      - id: fix
        kind: claude
        prompt: "Fix Codex's findings on {{mr.artifact}} and push.\n\n{{review.findings}}\n\n此前轮次 Codex 已反馈过的问题(参考,避免来回改 / 重复修;为空表示首轮):\n{{review.history}}"
```

`templates/full-pipeline.yaml` 的 apply-feedback 同形:

```yaml
      - id: apply-feedback
        kind: claude
        prompt: "按 Codex 反馈修改并提交:{{codex-review-mr.findings}}\n\n此前轮次已反馈过的问题(参考,避免来回改 / 重复修;为空表示首轮):\n{{codex-review-mr.history}}"
```

- [ ] **Step 6: 跑测 + clippy + 模板 dry-run**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Run: `cargo run -p agentpipe-cli -- validate templates/mr-review-loop.yaml && cargo run -p agentpipe-cli -- validate templates/full-pipeline.yaml`(CLI 二进制名以 `crates/cli/Cargo.toml` 为准)
Expected: 全绿;两个模板 validate 通过。

- [ ] **Step 7: Commit**

```bash
git add crates/engine templates
git commit -m "feat(engine): {{id.history}} 轮间记忆插值 + 模板 fix prompt 接入(防 review-fix 振荡)"
```

---

### Task 6: CLI 渲染 — LoopConverged residual

**Files:**
- Modify: `crates/cli/src/render.rs`
- Test: 同文件内联 tests

- [ ] **Step 1: 写失败测试(render.rs 内联 tests mod)**

```rust
#[test]
fn renders_loop_converged_with_residual() {
    let e = Event::LoopConverged { loop_id: "l".into(), iterations: 2, residual: 3 };
    assert_eq!(render_event(&e), "  ✓ l converged in 2 round(s), 3 residual finding(s) allowed");
    let e0 = Event::LoopConverged { loop_id: "l".into(), iterations: 2, residual: 0 };
    assert_eq!(render_event(&e0), "  ✓ l converged in 2 round(s)");
}
```

- [ ] **Step 2: 实现**

render.rs LoopConverged 臂改:

```rust
Event::LoopConverged { loop_id, iterations, residual } => {
    if *residual > 0 {
        format!("  ✓ {loop_id} converged in {iterations} round(s), {residual} residual finding(s) allowed")
    } else {
        format!("  ✓ {loop_id} converged in {iterations} round(s)")
    }
}
```

- [ ] **Step 3: 跑测 + Commit**

Run: `cargo test -p agentpipe-cli && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS。

```bash
git add crates/cli
git commit -m "feat(cli): LoopConverged 渲染带残留 finding 数"
```

---

### Task 7: GUI 镜像 — residual 展示 + Skipped-without-Started 容错验证

**Files:**
- Modify: `ui/src/types.ts:50`(LoopConverged 加 `residual?: number`)
- Modify: `ui/src/state/runReducer.ts:124-125`
- Test: `ui/src/state/runReducer.test.ts`

**Interfaces:**
- Consumes: engine 事件 JSON(residual 字段;Tauri 桥透传 serde JSON,`u32` → number)。

- [ ] **Step 1: 写失败测试(runReducer.test.ts,沿用文件内既有构造 helper)**

```ts
it("LoopConverged 带 residual 时结果文案含残留数", () => {
  let s = initialRunState();
  s = runReducer(s, { type: "LoopConverged", loop_id: "l", iterations: 2, residual: 3 });
  expect(s.loops["l"].result).toContain("残留 3");
});

it("LoopConverged 无 residual(老日志)回退纯收敛文案", () => {
  let s = initialRunState();
  s = runReducer(s, { type: "LoopConverged", loop_id: "l", iterations: 2 });
  expect(s.loops["l"].result).toBe("收敛");
});

it("无 StepStarted 的 StepFinished(Skipped) upsert 出可见条目", () => {
  // P1 Skip(loop_id)与 P6 短路跳过都会产生 Finished-without-Started;
  // setStep 是 upsert(runReducer.ts:57-63,`?? { status: "Pending" }` + order 补插),
  // 本测试钉死这个容错语义防未来回退。
  let s = initialRunState();
  s = runReducer(s, {
    type: "StepFinished", step_id: "fix", status: "Skipped", summary: "loop 已收敛,跳过",
  });
  expect(s.steps["fix"].status).toBe("Skipped");
  expect(s.order).toContain("fix");
});
```

- [ ] **Step 2: 实现**

types.ts:

```ts
| { type: "LoopConverged"; loop_id: string; iterations: number; residual?: number }
```

runReducer.ts LoopConverged 臂:

```ts
case "LoopConverged": {
  const residual = e.residual ?? 0;
  const result = residual > 0 ? `收敛(残留 ${residual})` : "收敛";
  return { ...prev, loops: { ...prev.loops, [e.loop_id]: { iteration: e.iterations, result } } };
}
```

StepFinished 臂:零改动 —— `setStep`(runReducer.ts:57-63)已是 upsert(`prev.steps[id] ?? { status: "Pending" }`,order 不含则补插),Finished-without-Started 天然容错;Step 1 的第三个测试只是钉死该语义防回退。

- [ ] **Step 3: 跑测 + Commit**

Run: `cd ui && npm test`(以 ui/package.json scripts 为准)
Expected: PASS。

```bash
git add ui/src
git commit -m "feat(ui): LoopConverged 残留数展示 + Skipped-without-Started 容错钉死"
```

---

### Task 8: 收尾 — 全量 verify + demo 回归 + 四维自查

**Files:** 无新改动(修自查发现的问题除外)

- [ ] **Step 1: 全量验证**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd ui && npm test && cd ..
```

Expected: 全绿。任一红 → 修复后重跑,不带病收尾。

- [ ] **Step 2: demo stub 端到端回归(README Quickstart 流程)**

```bash
cargo build --release
rm -rf /tmp/ap-demo && mkdir -p /tmp/ap-demo/repo && (cd /tmp/ap-demo/repo && git init -q)
AGENTPIPE_CLAUDE_BIN=$PWD/demo/stub-claude.sh \
AGENTPIPE_CODEX_BIN=$PWD/demo/stub-codex.sh \
AGENTPIPE_HOME=/tmp/ap-demo \
./target/release/agentpipe run demo/demo-task.yaml
```

Expected: 第 1 轮 changes_requested → fix → 第 2 轮 clean 收敛,exit 0。注意 demo/stub-codex.sh 输出旧格式 findings(无 suggestion → 解析 fallback),verdict 语义不受影响;若第 2 轮 clean 短路了 demo 里锚点后的 step,确认输出含 "loop 已收敛,跳过" 而非报错。

- [ ] **Step 3: 四维自查(走 /four-dimension-review checklist)**

重点面:
- 链路连贯性:`eval_until` 新签名的所有 callsite;`codex.review` 新 vet 参数的两个 callsite(step / verify_once);`emit_skipped` 改 wrapper 后语义不变。
- 同构面:`LoopConverged` 结构变更三处消费端(engine tests / cli render / ui types+reducer)全部同步;stub 两份(tests/fixtures + demo)行为差异是否影响断言。
- 字面 vs 语义:`LoopMaxReached.max` 现在是"累计已跑轮数"(Retry 后 > 配置 max),render 文案 "hit max N" 是否误导 → 若误导改 "ran N round(s), still not clean"。
- 默认值最坏 case:`allow_residual` 缺省、`vet` 缺省、`residual` 老日志缺字段、history 空串 —— 各自最坏路径是否与改动前行为完全一致。

- [ ] **Step 4: 提交自查修复(如有)**

```bash
git add -A
git commit -m "fix(engine/ui): 四维自查修复"
```
