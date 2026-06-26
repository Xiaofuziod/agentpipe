# review-2 修复 spec

日期：2026-06-26 (第二轮 /code-review 后)
对应:`/code-review xhigh` 二轮跑出的 15 findings(本会话产出)

## 0. 范围 / 跳过

修(13 条):#1-13。
跳(2 条):#14 / #15 process 问题(分支命名 + spec/code 同 commit) — 事后改 branch 等于 force-push 历史,不动;下次开 MR 时按 CLAUDE.md「一功能=一分支」拆。

## 1. Phase 分块

### Phase A — schema 矛盾(#1)+ hint 稀释(#6)

**问题 #1**:`REVIEW_SCHEMA` 把 suggestion 从 required[] 移出 + 仍在 properties + `additionalProperties:false` → 触发 docs/specs/cli-behavior-findings.md:18 写的 OpenAI 422 `invalid_json_schema`。真实 codex review 整次 spawn 失败,触发 loop 活锁。

**问题 #6**:SUGGESTION_HINT 文案从「必须提供 suggestion」改为「可选提供 ... 无具体建议时省略或填 N/A」,模型走最省路径默认略过,spec §3.2「提升反馈深度」主目标实质打折。

**修法**:
- REVIEW_SCHEMA suggestion 加回 required[](回归 §D 改造前)
- SUGGESTION_HINT 改回「强烈建议提供 suggestion,无具体建议时填 N/A」(强约束 prompt)
- RawFinding.suggestion 仍 #[serde(default)] 兜底解析端(mock fixture / 边缘情况)
- spec §3.2 「向后兼容旧 codex 二进制」承诺改为「需 codex CLI 支持 suggestion 字段」+ follow-up:文档明示 minimum codex version

**取舍**:接受旧 codex 二进制不兼容,推用户升级 codex(spec §3.2 自承「agentpipe 已锁 codex CLI 版本,升级前同步即可」)。

### Phase B — cost UI/audit 同根因(#2 + #5 + #7)

**问题 #2**:audit::aggregate_cost 只读 StepFinished.metrics;本 PR cost 进 ctx 守 budget,但 finish 传末次 attempt 一份 → audit 总额低估 N-1×attempt + 全部 verifier。
**问题 #5**:codex verifier 路径 verifier_metrics 始终 None → 'verifier 钱进 budget' 在主用例失效。
**问题 #7**:OnUnmet::Continue / Verifier Clean 路径 finish 仅 attempt metrics,verifier 那份 GUI 不可见。

**修法**:
- Claude branch 新增 `cumulative_metrics: Option<StepMetrics>` 局部累加(attempt + verifier_metrics);每次 attempt + verify_once 后用 sum_metrics 累进;finish 传 cumulative 给 StepFinished
- 加 helper `fn sum_metrics(a: Option<StepMetrics>, b: Option<StepMetrics>) -> Option<StepMetrics>`(num_turns + duration_ms + cost_usd 三字段累加)
- codex verifier metrics None 加 doc 说明「codex CLI 当前不出 token usage,此 cost 已知漏统计(charge_and_check 累加 0)」,并 spawn_task 让 follow-up 跟踪

### Phase C — set_budget fail-open(#3)+ 测试假绿(#4)+ RAII EnvGuard(#10)

**问题 #3**:set_budget(Some(NaN/-1/0)) 静默落 None + warn,fail-OPEN 反 safety fail-closed 基线。Executor::new 已 expect panic,该路径生产死代码。

**问题 #4**:测试 `l.contains("无 ")`(尾随空格)与实际渲染 `"  ↳ 建议: 无"`(末尾无字符)永不匹配 → 测试假绿。

**问题 #10**:STUB_CLAUDE_RESULT cleanup 在 ex.run() 之后,panic 跳过 → 跨测试污染。

**修法**:
- set_budget 失败 panic(与 Executor::new 一致);删 fail-open + warn 分支
- 测试改 `l.ends_with(": 无")` (锚定后缀)
- 抽 `struct EnvGuard(&'static str)` + Drop impl 自动 remove_var;两处 STUB_* 改用 EnvGuard

### Phase D — 核心字段缺失测试(#8)+ placeholder 收窄(#9)+ trim 冗余(#13)

**问题 #8**:RawFinding 去 default 后无测试覆盖「核心字段缺一个 → 整次解析失败 → fallback ChangesRequested 假 findings」降级路径。

**问题 #9**:SUGGESTION_PLACEHOLDERS 含 "no" / "-" / "todo" 整串等值匹配太激进,误吞合法短建议。

**问题 #13**:is_placeholder_suggestion 内 `s.to_lowercase().trim()` 冗余(caller 已 trim)。

**修法**:
- 新 fixture stub-codex-malformed-finding.sh(severity 缺失),新测试断言走 fallback + 错误信息可观测
- SUGGESTION_PLACEHOLDERS 收窄到 `["n/a", "none", "无", "tbd", ""]`(删 "no" / "-" / "todo")
- is_placeholder_suggestion 删 inner trim

### Phase E — try_new(#12)+ run_loop 终止事件(#11)

**问题 #12**:Executor::new 内 .expect panic — 库构造器内 panic 是错的高度。SDK 嵌入时整线程崩。

**问题 #11**:run_loop body charge_and_check Err 透传时无 loop 终止事件 → GUI 显示『iter N 进行中』然后突然全局 Aborted。

**修法**:
- 加 `pub fn try_new(...) -> Result<Self, EngineError>`;`new` 保留为 thin shim `try_new(...).expect("...")`,兼容现有 callsite。CLI/Tauri 路径已 validate 不用改;tests 用 new 也不破。
- run_loop body 返 Err 透传前 emit `Event::LoopMaxReached`(语义降级覆盖「loop 因外因停止」,复用现有 variant 避新 protocol 变体)。文档明示该 event 现在双语义(『超 max』+『budget/control abort 中段停』)。

### 不修(spec §0):

- #14 分支命名 / #15 spec/code 同 commit — process 问题,事后改 branch 等于 force-push 历史,不动

## 2. Commit 划分

按 phase 顺序 5 个 commit,同分支同 MR:
- `fix(engine): codex schema 矛盾 + hint 稀释(回归 §D 改造前)`
- `fix(engine): cost UI/audit 累积 metrics(verifier cost 也进 StepFinished)`
- `fix(engine): set_budget fail-loud + 测试假绿修正 + EnvGuard RAII`
- `test(engine): 核心字段缺失 fallback 覆盖 + 占位词收窄`
- `refactor(engine): Executor::try_new Result API + run_loop budget-abort 事件`

## 3. 验收

- `cargo test --workspace` 全绿(预计 121 → 124+)
- `cargo clippy --workspace --all-targets -- -D warnings` 0 warning
- 5 个 commit 全推 origin/feat/project-grouping
- 不破坏 baseline + ACP + 已有 budget 测试
