# review loop 加固：USD budget cap + 反馈深度

日期：2026-06-26
对应调研：2026-06-25 ACP 集成后 `/simplify` 流程外延的 multi-agent 业内调研报告(本会话产出)。

## 0. 背景

agentpipe 现在的 verify loop 有两条业内验证过的最高 ROI 改进缺口:

1. **无 USD budget cap**:[manifest.rs:141](../../crates/engine/src/manifest.rs) `MAX_VERIFY_RETRIES=10` + `Loop::max` 是步数/轮数 cap,但**没有 cost cap**。业内两起公开 postmortem(LangChain $47K / 11 天循环,$4.2K / 63h 429-retry 风暴)的根因总结都是「无 step cap 还能撑过去,**无 USD budget 就是定时炸弹**」。
2. **反馈深度不足**:[codex.rs:184-198](../../crates/engine/src/runner/codex.rs) 现在 codex 输出已结构化(`findings[].{severity,file,line,summary}`)但**没有具体修改建议**——下游 Claude 拿到的反馈是「文件 X 第 Y 行有问题」,不是「文件 X 第 Y 行用 A 替换 B,因为 C」。学术(Self-Refine / arXiv 2509.16330)与工业(Anthropic Claude Code Review)共识:**feedback depth—actionable critiques and clear error localization—is crucial; generic or superficial feedback can degrade or plateau performance**。

## 1. 目标

- **不破坏** 现有 `Verdict` / `StepOutput` / `Event` 公共抽象。
- **不动** Claude / ACP runner、不动 verify gate 现有决策机制。
- 改动**局限到 manifest + executor + codex runner**,目标 ≤ 4 文件 + 测试。
- 新字段全部 **optional**,存量 YAML / 旧 codex 二进制行为不变。

## 2. 非目标 (留 V2)

- ❌ 完整重构 Verdict 为 AutoGen-style 4-field {correctness, efficiency, safety, approval} —— 现状 Verdict 已贯穿 context/protocol/executor/audit,大破坏面;suggested_changes 拿到 80% 收益。
- ❌ ACP step / Claude step 改输出 schema(这俩本就是自由文本 + verify 门判定,不强制结构化反馈)。
- ❌ Per-step USD budget(MVP 只做 per-run 总额;per-step 留下个 release)。
- ❌ Token-level budget(只用 cost_usd,与 audit aggregate 同源)。

## 3. 方案

### 3.1 USD budget cap

**Manifest 字段**(可选):
```yaml
version: 1
name: my-run
budget_usd: 5.0    # 新增:per-run 总额上限;省略 = 无限(向后兼容)
target: .
steps: [...]
```

**Executor 行为**:
- `RunContext` 新增 `cost_so_far_usd: f64`(累计)+ `budget_usd: Option<f64>`。
- 每个 step 完成后 `record_metrics` 时累加;**累计 > budget 立即 fail-loud**,发 `Event::RunFinished { status: Aborted }` + 终止后续 step。
- 错误信息明确:`"超出 USD budget (spent $X.XX > cap $Y.YY at step '<id>')"`。
- 边界:刚好等于 budget 不触发(`>` 而非 `>=`),避免浮点抖动误杀;超出时已花的成本是真实的(已 spawn 完),不撤销。

**为什么不加新 Event 变体**:`Event::RunFinished{Aborted}` 已覆盖语义,新增 `BudgetExceeded` 反而要 cli/render + Tauri bridge 双向同步,违反范围纪律。`StepFailed` 的 `error` 字段已有具体信息可以渲染。

**测试**:
- manifest 解析含 budget 的 YAML
- manifest 验证非负数(`budget_usd < 0` reject)
- Executor 模拟:跑 2 个 fake step,人工注入 cost,验第 2 步触发后 RunFinished{Aborted}

### 3.2 Verdict 反馈深度:codex schema 加 `suggested_changes`

**Schema 增量**(REVIEW_SCHEMA 里 finding 项):
```json
"properties": {
  "severity": {"type":"string"},
  "file": {"type":"string"},
  "line": {"type":"integer"},
  "summary": {"type":"string"},
  "suggestion": {
    "type":"string",
    "description":"具体修改建议:'用 X 替换 Y' 或 '在第 N 行后插入 Z',让 fixer 能直接照搬"
  }
}
```
注:严格 JSON Schema(`additionalProperties:false`)要求 `suggestion` 进 `required` 才能保证模型输出。我们让 codex prompt 显式要求"为每个 finding 给出具体可执行 suggestion(无则填 'N/A')",并把 `suggestion` 加进 `required`。

**Prompt 增量**(codex.rs review-mr / review-doc 两个分支):
- 在现有 prompt 末尾加一句:`"对每个 finding 提供 suggestion 字段(具体修改建议,例:'第 42 行 nil-check 改成 if let Some(x) = y { ... }';无具体建议填 'N/A')"`。

**渲染**(raw_to_result):
- 当前格式:`"[severity] file:line summary"`
- 新格式:`"[severity] file:line summary\n  ↳ 建议: <suggestion>"`(suggestion="N/A" 时省略 ↳ 行,避免噪音)。
- 下游 Claude 拼到 prompt 时,findings 字符串质量大幅提升。

**向后兼容**:
- 旧 codex 二进制不识别新 schema 字段 → OpenAI 端可能 reject(strict mode)。**MVP 接受这个风险**:agentpipe 已锁 codex CLI 版本,升级前同步即可。若发现真实兼容问题,fallback 路径(parse_review)已经 ChangesRequested+占位 findings,不会假成功。
- RawReview 的 Finding 结构体新增 `suggestion: String`(non-optional with default "N/A" via serde),解析失败仍走 fallback。

**测试**:
- codex_runner_test 加 1 个 fixture stdout:含 suggestion 字段,验渲染正确
- 缺 suggestion 字段的 legacy fixture → 旧测试继续通过(serde default)

## 4. 范围摘要

| 文件 | 改动 |
|---|---|
| `crates/engine/src/manifest.rs` | + `Manifest.budget_usd: Option<f64>`,validate 非负 |
| `crates/engine/src/context.rs` | + `RunContext.cost_so_far_usd` + helper `add_cost`/`over_budget` |
| `crates/engine/src/executor.rs` | step 完成后累加 cost,超 budget fail-loud + Abort |
| `crates/engine/src/runner/codex.rs` | schema 加 suggestion + prompt 改 + raw_to_result 渲染 |
| `crates/engine/tests/*` | 新增 budget 触发 + suggestion 解析测试 |

预计 +200 ~ +300 行,4 文件 + 2 测试文件。

## 5. 风险

| 风险 | 缓解 |
|---|---|
| budget cap 浮点累加误差 | 用 `>` 而非 `>=`;cost_usd 通常 0.001 量级精度,误差不会跨越 cap |
| codex 模型不按 schema 输出 suggestion | strict schema + required 字段强制;fallback 路径已 ChangesRequested 不假成功 |
| 旧 codex 二进制不认识新字段 | 用户责任(升级 codex);agentpipe 锁版本时同步 |
| budget 触发时已经花了 cost,可能体验"突然中断" | 文档明示;后续 V2 可加 "soft warning at 80%" 但不在 MVP |

## 6. 验收

- `cargo test --workspace` 全绿
- `cargo clippy --all-targets -- -D warnings` 0 warning
- 新增至少 3 个测试:budget 配置解析 / budget 触发 abort / suggestion 渲染
- 单个 manifest 示例(yaml fixture)能跑通带 budget 配置
- 文档:本 spec 入仓
