# review findings 修复 spec

日期：2026-06-26
对应:`/code-review xhigh` 跑出的 15 findings(本会话产出),除分支命名(process)外全部修。

## 0. 范围 / 不范围

修(14 条):cost tracking 系统性低估 + NaN 守护 + Event 语义 + schema/serde 一致性 + UI 镜像 + 散点。

跳(1 条):**分支命名** `feat/project-grouping` 与 budget/verdict 主题不匹配 —— process 问题,事后改 branch name 等于 force-push 历史,不动。下次开 MR 时按 CLAUDE.md「一功能=一分支」拆。

## 1. Phase 分块

### Phase A — cost tracking 下沉(最关键,修 spec 主目标失败)

**问题**:
- Claude verify-retry 中除末次外的所有 attempt cost 全丢(executor.rs:202 `continue` 路径 + retry 循环只把末次 metrics 传 finish)
- OnUnmet::Fail 走 `self.fail()` 完全绕过 finish,整个 step 的真实花费消失
- `verify_once` 内 spawn 的 codex/claude verifier cost 从未上报

**方案**(最小侵入,不抽 trait):
1. `executor.rs` 新增 `charge_metrics(metrics: &Option<StepMetrics>)` helper,内部调 `ctx.add_cost`。所有调用 `claude.run` / `codex.review` 的位置在拿到 Ok 后立即调用此 helper(包括 verify-retry 每次 attempt 与 verify_once)。
2. `verify_once` 签名扩为 `(Verdict, String, Option<StepMetrics>)`,把 verifier 的 metrics 抛出来让上层 charge。
3. `OnUnmet::Fail` 路径在 `self.fail()` 之前调 `charge_metrics(&metrics)` —— 即使 step 失败,真花的钱也入账。
4. `codex.rs` `ReviewResult` 加 `pub metrics: Option<StepMetrics>` 字段(currently 始终 None,schema 准备好;未来 codex CLI 输出 token usage 时填)。
5. 任何 charge 调用后用 `ctx.is_over_budget()` 检测,触发即 emit StepFailed("超出 USD budget") + return Err。这部分逻辑封装在 `charge_metrics` 返回 `Result<(), ()>`,callsite 用 `?`。

**测试**:
- 新 executor 测试:Claude verify max_retries=2 + on_unmet=fail + 每次 attempt cost=0.01 + budget=0.025 → 第 3 次 attempt(末次)后超额 abort,total cost=0.03 入账
- 新 executor 测试:verify_once 路径 codex verifier cost=0.005(模拟)累加到 ctx
- 现有 budget_exceeded_aborts_run_at_first_overrun 保持通过

### Phase B — NaN 守护 + observability

1. `context.rs::set_budget` 内部校验 NaN/inf/≤0,无效输入静默落 None(配 warn 日志),确保 invariant 不依赖 caller 自觉
2. `executor.rs::new` 调一次 `manifest.validate()`,失败 panic(`expect`)—— Executor 不接受未验证 Manifest
3. `finish` 内 `unwrap_or(f64::NAN)` 改 `expect("is_over_budget guarantees Some")`,文档 invariant
4. `context.rs::add_cost` 收到 NaN/负数 改 `tracing::warn!`(无 tracing 则 eprintln),不静默吞

### Phase C — Event 语义修复

1. over-budget 不双 emit:finish() 改为「先 charge → 若超额则 emit StepFinished{Done, summary 带 budget 说明} + return Err(()),不 emit StepFailed」。Run 层仍走 RunStatus::Aborted。callsite 看到一个 step 一个终态,语义对齐。
2. `run_loop` 在 sub-step 返 Err 透传前发 `Event::LoopMaxReached`(语义"loop 中止")。或新增 `Event::LoopAborted{loop_id}` —— 实际选前者(复用已有变体,不动 protocol)。

### Phase D — schema/serde 一致性

1. `codex.rs::REVIEW_SCHEMA` 把 `suggestion` 移出 required[] (改回 spec §3.2 设计意图:向后兼容,suggestion 可缺)
2. `codex.rs::RawFinding` 核心字段 `severity` / `file` / `summary` 去 `#[serde(default)]`(改回必填,与 schema required 对齐;只有 suggestion 留 default)
3. `codex.rs::render_finding` 占位集合扩到 `{"n/a", "none", "无", "tbd", "todo", ""}`,trim 后大小写不敏感判断
4. `codex.rs::SUGGESTION_HINT` 标点统一:起头不带句号,要求 prompt 自带结尾 `。`

### Phase E — UI mirror + cosmetics

1. `ui/src/types.ts` Manifest 类型加 `budget_usd?: number | null` 镜像 backend
2. `templates/mr-review-loop.yaml`(主烧钱场景)加注释样例 `# budget_usd: 10.0  # 可选:per-run USD 上限`
3. `context.rs::add_cost` doc-comment 加一行 "用 f64 累加,精度 ~1e-15;USD-cents 量级预算够用,需要绝对精度时改 i64 micros"
4. (可选)`stub-codex-with-suggestion.sh` 参数化合并到 `stub-codex.sh`(用 `STUB_INCLUDE_SUGGESTION=1` env)—— 若工作量 < 5 分钟做,否则跳

## 2. Commit 划分

5 个 commit,同分支同 MR,按 phase 顺序提交:
- `fix(engine): cost tracking 下沉到每个 spawn 点 — verify-retry/Fail/verifier 路径都入账`
- `fix(engine): NaN budget 守护 + Executor::new 强制 validate + add_cost 静默改 warn`
- `fix(engine): over-budget 不双 emit + run_loop 加终止事件`
- `fix(engine): codex schema suggestion 改 optional + RawFinding 核心字段必填 + 占位集合扩展`
- `chore: UI types budget_usd 镜像 + template 示例 + context drift 文档`

## 3. 验收

- `cargo test --workspace` 全绿(预计 119 → 125+ 测试)
- `cargo clippy --workspace --all-targets -- -D warnings` 0 warning
- 5 个 commit 全部 push 到 origin/feat/project-grouping
- 不破坏 baseline tests / ACP runner tests

## 4. 风险

| 风险 | 缓解 |
|---|---|
| Phase A 改 executor 主路径,可能破现有 verify gate 行为 | 现有 6 个 verify 相关测试是 regression 网,任一红就停 |
| ReviewResult 加 metrics 字段是公开 API 改动 | 该 struct 仅 codex/executor 内部用,不破外部 |
| set_budget 静默落 None 改 caller-visible 错误? | 用 tracing::warn + 落 None,fail-soft 不 fail-loud(invariant 由 manifest.validate 兜底);文档明示 |
| UI types 加字段后 GUI 不一定能展示控件 | 仅类型镜像,GUI 控件留 follow-up |
