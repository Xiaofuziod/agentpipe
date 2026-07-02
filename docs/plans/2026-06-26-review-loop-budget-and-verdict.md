# review loop budget + verdict 改进执行计划

日期：2026-06-26
对应 spec：[2026-06-26-review-loop-budget-and-verdict-design.md](../specs/2026-06-26-review-loop-budget-and-verdict-design.md)
分支：feat/project-grouping (沿用,跟 ACP 同 MR;CLAUDE.md「一功能=一分支=一 MR」)

## Phase 0 — Pre-flight

1. `git status --porcelain` 干净(允许本 spec/plan untracked)
2. `cargo test --workspace` 当前 108 测试基线绿

## Phase 1 — USD budget cap

| 步骤 | 改动 | 验证 |
|---|---|---|
| 1.1 | `manifest.rs` `Manifest` 加 `budget_usd: Option<f64>` (serde skip_serializing_if Option::is_none),`validate` 拒绝 ≤ 0 | manifest 单测 +2:解析 + 非负校验 |
| 1.2 | `context.rs` `RunContext` 加 `cost_so_far_usd: f64` + `budget_usd: Option<f64>` + `record_metrics(&StepMetrics) -> bool`(返 over-budget flag)| context 单测 +1:累加 + over-budget 判定 |
| 1.3 | `executor.rs` Executor::new 接收 budget;step 成功后调 `record_metrics`,触发 over-budget 直接 `RunStatus::Aborted` + 错误 event | executor 单测 +1:fake metrics 触发 abort |
| 1.4 | `cli/main.rs` 渲染 over-budget 错误清晰,不只 "aborted" | 手验 |

**完工标准**:cargo test 全绿;clippy 0 warning;manifest 示例可写 `budget_usd: 5.0`。

## Phase 2 — Verdict 反馈深度

| 步骤 | 改动 | 验证 |
|---|---|---|
| 2.1 | `codex.rs` `REVIEW_SCHEMA` 加 `suggestion` 字段(strict required) | codex_runner_test 现有 fixture 加 suggestion 字段后仍解析通过 |
| 2.2 | `codex.rs` `RawReview::Finding` 加 `suggestion: String` (serde default "N/A") | 同上 |
| 2.3 | `codex.rs` review-mr / review-doc 两条 prompt 末尾加 "对每个 finding 提供 suggestion 字段..." 提示 | 集成验证 |
| 2.4 | `codex.rs` `raw_to_result` 渲染:suggestion 非 "N/A" 时附加 `\n  ↳ 建议: ...` 行 | codex_runner_test 新增 1 fixture:含 suggestion 验渲染;legacy fixture (无 suggestion) 渲染保持原样(走 serde default + 跳过 ↳ 行) |

**完工标准**:cargo test 全绿;clippy 0 warning;codex 新 fixture 测试通过。

## Phase 3 — verify + commit + push

| 步骤 | 改动 | 验证 |
|---|---|---|
| 3.1 | `npm run` 不适用,跑 `cargo test --workspace` 全绿 | 必过 |
| 3.2 | `cargo clippy --workspace --all-targets -- -D warnings` 0 warning | 必过 |
| 3.3 | `cargo fmt` 我自己改的文件(避免 baseline fmt 漂移) | 局部 |
| 3.4 | 四维度自查(链路连贯性 / 同构面 / 字面 vs 语义 / 默认值最坏 case) | 走查清单 |
| 3.5 | Phase 1 + Phase 2 分两个 commit(同分支) | git log 整洁 |
| 3.6 | `git push` | 远端绿 |

## 不可降级红线

- ✅ 每次 Write 后立即 `wc -l` + tail 验证落盘
- ✅ commit 前 `cargo test --workspace` 本地真跑过
- ✅ 不顺手 fmt 范围外文件(上次 ACP commit 教训)
- ✅ 任一测试失败禁止进下一 phase
