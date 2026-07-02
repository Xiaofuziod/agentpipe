# review loop 收敛加固:max fail-closed + severity 阈值 + findings 核验 + 轮间记忆

日期:2026-07-02
对应调研:2026-07-02 对本仓「模型交互流转 / 对抗验证 / 自动循环」的架构评估(本会话产出),扫出 6 个问题(P1-P6,按严重度编号)。

## 0. 背景:扫描出的问题清单

| # | 问题 | 严重度 | 现状证据 |
|---|---|---|---|
| P1 | loop 跑满 max 后 run 继续走并最终 Success,fail-open,与 README「stops and asks a human」承诺矛盾 | bug 级 | executor.rs run_loop MaxReached 分支 emit 事件后返回 Ok(()),CLI/Tauri 无一端拦截该事件转 gate |
| P2 | 收敛判据只认布尔 verdict,reviewer 每轮挑新 nit 时永不收敛,烧满 max | 设计缺口 | eval_until 只 matches Verdict::Clean;severity 字段 schema 里有但无人消费 |
| P3 | findings 无核验直接喂 fixer,reviewer 幻觉出的误报会被照单全修 | 设计缺口 | executor codex 分支 record findings 后直接插值进 fix prompt |
| P4 | fixer 无轮间记忆,可能振荡(第 1 轮按反馈 A→B,第 3 轮又被挑回 A) | 设计缺口 | 每轮 fix 只拿 {{review.findings}} 当前轮内容 |
| P5 | codex 不上报 usage,codex step / Verifier::Codex 不进 budget_usd | 上游限制 | codex.rs WARN_CODEX_COST_BYPASS 一次性告警,已可观测 |
| P6 | 收敛那一轮多烧一次 fix:body 全跑完才 eval until,review 判 clean 后 fix 仍带空 findings 跑一遍 | 浪费 | run_loop 先 for sub in body 再 eval_until |

## 1. 策略级候选(先定大方向)

| 候选 | 内容 | trade-off |
|---|---|---|
| A. 引擎收口(推荐) | 全部在 engine 层修(executor + codex runner + manifest 可选字段),模板最小改动,新字段全 optional 向后兼容;P1 是有意的行为变更(fail-open → fail-closed) | 改动集中、可测试、根治;代价是 manifest 面积 +2 字段、ReviewResult 加内部结构 |
| B. 模板/prompt 层修 | 不动引擎,靠 YAML 工程(fix prompt 手工带历史、review prompt 要求"只报 major+") | 零引擎风险,但 P1/P6 在模板层无法修(引擎控制流问题),P2 靠 prompt 约束 reviewer 不确定性大,不治本 |
| C. Verdict 抽象重构 | AutoGen-style 多维 verdict + 结构化 finding 贯穿 protocol/audit/UI | 最彻底,但破坏面大(2026-06-26 spec 已裁决为 V2 非目标),本次问题不需要它 |

**裁决:A。** B 留作模板侧配合(fix prompt 引用新插值变量);C 维持 V2 押后。

## 2. 目标

- P1:loop 自然耗尽 max 必须过决策门(重试/跳过/中止),run 不再静默 Success —— README 的既有承诺落地。
- P2:loop 支持 severity 阈值收敛(可选),低于阈值的 residual findings 不阻塞收敛。
- P3:codex review 支持可选的自反驳核验 pass,误报在喂给 fixer 前被过滤。
- P4:引擎提供 `{{<id>.history}}` 插值变量,fix prompt 可引用此前轮次的 findings 防振荡。
- P6:review 判 clean 后短路 body 剩余 step,收敛轮不再多烧一次 fix。
- 全部新 manifest 字段 optional,存量 YAML 行为不变(P1/P6 是引擎行为变更,无字段开关,理由见 §3.1/§3.5)。

## 3. 方案

### 3.1 P1:MaxReached 走决策门(fail-closed)

**行为**:`run_loop` 自然耗尽 max 后,保留现有 `LoopMaxReached{reason: MaxReached}` 事件(审计/渲染不变),随后走 `decision_gate(loop_id, "loop 跑满 N 轮仍未收敛,选择 重试/跳过/中止\n<末轮 findings>")`:

- **Retry(ApproveGate)** → 再给一整份 `max` 轮预算重新进循环;iteration 编号**继续累加**(带 offset,UI 不出现第二个 "round 1")。与 verify 门 `OnUnmet::Gate` 的 Retry 语义同形(人工再批一次 = 新预算)。
- 门的 suggestion 附带**末轮 findings**:取锚点 step(body 最后一个 codex step,与 §3.5 同一计算)在 ctx 里的 findings,让人在门上直接看到"还剩什么没修"再决策。
- **Skip(SkipStep)** → `emit_skipped(loop_id)` 留审计痕(StepFinished{Skipped}),run 继续后续 step。「未收敛但人工放行」从此是显式决策,不是默认行为。
- **Abort** → `request_abort()` + Err,run 落 `RunStatus::Aborted`。

**为什么不加 manifest 开关**:fail-open 是 bug 而非可选行为;README 与模板注释一直承诺 fail-closed,代码向承诺对齐,不需要向 bug 兼容。

**headless 语义**:CLI 现有 stdin-EOF 策略是 fail-closed Skip(main.rs prompt_gate),MaxReached 门在无人值守时会被 Skip 并留下 `LoopMaxReached + StepFinished{Skipped}` 两条审计事件 —— 不阻塞、不静默,与 step 失败门的既有 EOF 语义一致,不另立规则。

**不动的**:`LoopEndReason::Aborted / SubStepFailed` 两条路径行为不变(已经 Err 透传)。

### 3.2 P2:severity 阈值收敛(`allow_residual`)

**schema**:REVIEW_SCHEMA 的 `severity` 从自由 string 收紧为 enum `["critical","major","minor","nit"]`(strict structured output 保证模型只能输出这四个;stub / 旧 codex 输出未知串时按 fail-closed 映射为 critical,见下)。

**结构化 findings 内部化**:`ReviewResult` 加 `items: Vec<FindingItem>`(severity/file/line/summary/suggestion),渲染串 `findings: String` 保留(插值 / 审计 / UI 展示继续用它)。`StepOutput` 同步加 `items`。这是**内部类型**,不进 NDJSON 事件协议,无兼容负担。

**manifest**:`Loop` 加可选字段:

```yaml
- id: review-fix
  kind: loop
  until: codex-clean
  max: 10
  allow_residual: minor   # 可选:severity ≤ minor 的 residual findings 不阻塞收敛
  body: [...]
```

**收敛判定**(eval_until 扩展):

```
converged =
  verdict == clean
  || (allow_residual = Some(t)
      && !items.is_empty()
      && items 全部 severity ≤ t)
```

- `items.is_empty() && verdict != clean` **必须不收敛**:解析 fallback 路径(codex 输出不可解析)正是「items 空 + changes_requested 占位文案」的形状,放行等于把解析失败判过,违反 fail-closed 底线。
- severity 排序:critical > major > minor > nit;未知字符串 → 按 critical 处理(阻塞,fail-closed)。
- `allow_residual` 取值域 validate 限 `nit|minor|major`(residual critical 无意义,拒绝)。
- 缺省(None)= 现行为,只认 verdict clean。

**事件**:`LoopConverged` 加 `#[serde(default)] residual: u32`(带残留收敛时 >0),CLI/GUI 渲染 "converged with N residual finding(s)";老审计日志 default 0,回放语义不变。

**为什么不做轮间 findings-diff 收敛**("连续两轮无新增 finding 即收敛"):需要跨轮 finding 身份对齐(fix 后 file:line 漂移,匹配启发式易错判),复杂度高、误收敛风险大;severity 阈值用一个 optional 字段拿到主要收益。diff 收敛留 V2。

### 3.3 P3:codex 自反驳核验(`vet`)

**manifest**:codex step 加可选 `vet: true`(default false;`action: ask` 配 vet 被 validate 拒绝,fail-loud):

```yaml
- id: review
  kind: codex
  action: review-mr
  base: "{{base.artifact}}"
  vet: true    # 可选:findings 先自反驳核验再输出
```

**行为**(收在 CodexRunner 内部,executor 只传 flag):review 首轮结果 `verdict == changes_requested && !items.is_empty()` 时,追加一次 `codex exec`(read-only,同 REVIEW_SCHEMA):prompt 附上首轮 findings 渲染串,要求「逐条重新到代码里核实,尝试用具体代码证据反驳;删除证据不足/误读/幻觉的条目,保留确认成立的,重新输出 verdict + findings」。核验结果**替换**首轮结果:

- 全部条目被驳回 → verdict 翻 clean,正常走收敛(这正是 vet 的价值:全误报轮不再空烧一轮 fix)。
- vet 调用 Err / 输出不可解析 → **保留首轮结果** + progress 告警(fail-closed 方向:核验失败不丢 review 信号,宁可多修不可漏修)。
- clean 首轮结果不触发 vet(没东西可核)。

**为什么是 codex 自反驳而不是 claude 复核**:claude 是作者方,复核「对自己代码的批评」有实证的自偏好(倾向驳回成立的批评),会把对抗验证的跨厂商前提拆掉;codex 自反驳无作者利益冲突,强制 re-ground 到代码证据对幻觉类误报的杀伤最直接。代价是同厂商自查对「系统性盲区」无效 —— 但那类问题本来就该由 command 门 / 人工兜,不在本机制目标内。

**成本**:每个非 clean review 轮 +1 次 codex 调用(read-only,与 review 同价量级);opt-in,模板默认不开,用户按仓库误报率自行权衡。P5 背景下这笔钱同样不进 budget(见 §4 非目标)。

### 3.4 P4:轮间记忆(`{{<id>.history}}` 插值)

**引擎**:`RunContext` 加 `histories: HashMap<String, Vec<String>>`。executor 的 **codex step 分支**在 record 新 findings 前,把该 step 既有的非空 findings push 进 histories(只有 loop 里同 id 反复执行才会累积;直线 step 永远空)。`interpolate` 支持字段名 `history`:按 `── 第 N 轮 ──` 分隔拼接**此前**轮次(不含当前轮),无历史 → 空串。

**范围**:只 codex step 写 history;claude verify 门的 findings 不进(振荡问题只发生在 loop 的 review→fix 循环,verify-retry 已有 feedback 注入机制,不重复建设)。

**模板**:`mr-review-loop.yaml` / `full-pipeline.yaml` 的 fix prompt 追加尾部段落:

```
此前轮次 Codex 已反馈过的问题(参考,避免来回改 / 重复修):
{{review.history}}
```

第 1 轮该段为空串(prompt 尾部一个孤立标题,LLM 可无害忽略;引擎不做模板条件渲染,保持插值语义简单)。

**上限**:history 不截断(max=10 轮 × 典型 findings 几 KB,在 claude 上下文预算内)。注意 P1 的 Retry 会以 max 为步长延长轮数(history 随之变长),但每次延长都过人工决策门,总量仍受人控;若未来出现超长 findings 再加尾部截断,不预建。

### 3.5 P6:收敛短路(锚点后立即 eval)

**行为**:`run_loop` 计算**锚点** = body 中最后一个 codex step 的下标(与 eval_until 的取值来源一致,一次计算)。每轮跑完锚点 step 后立即 eval_until:

- 已收敛 → 对锚点之后的剩余 body step 逐个发 `StepFinished{Skipped, summary: "loop 已收敛,跳过"}`,emit `LoopConverged`,return Ok。
- 未收敛 → 继续跑剩余 body step,轮末不再重复 eval(锚点后没有 codex step,verdict 不会再变)。

**语义变更说明**:对канonical body `[review, fix]`,review 判 clean 那轮 fix 不再执行 —— 这正是修复目标。若用户把「必须每轮都跑」的 step 放在锚点之后(如 loop 内 deploy),行为会变;这种编排本身与 `until: codex-clean` 语义冲突(收敛 = 不需要再动),文档注明即可,不加开关。

**多 codex body 的坑**:锚点固定取「最后一个 codex step」,锚点之前的 codex step 完成后**不**做收敛检查 —— 那时 ctx 里锚点的 verdict 是上一轮的残留,检查会读到脏值(上一轮若 clean 早已收敛,所以残留只可能是 changes_requested,检查结果恒 false,白做且易误导后人)。

## 4. 非目标

- ❌ P5(codex 计费):上游 codex CLI 不输出 usage,引擎侧无数据可记;保留现有一次性 WARN。上游升级后按 2026-06-26 spec 预留的 `ReviewResult.metrics` 直接填。**不做**时间/轮次 proxy 计费(伪造精度比无数据更误导)。
- ❌ 轮间 findings-diff 收敛(见 §3.2,V2)。
- ❌ 多 reviewer 投票 / 并行 fan-out / Verdict 四维重构(两厂商个人工具,YAGNI,维持 2026-06-26 裁决)。
- ❌ GUI composer 暴露 `allow_residual` / `vet` 新字段(手写 YAML 可用,composer 表单化留 follow-up;不阻塞本 spec)。
- ❌ CLI stdin-EOF 门策略调整(现行 fail-closed Skip 覆盖所有门类,单独为 loop 门改语义反而制造不一致)。

## 5. 范围摘要

| 文件 | 改动 |
|---|---|
| crates/engine/src/executor.rs | run_loop:MaxReached 决策门 + Retry offset;锚点短路;eval_until 加 allow_residual 判定;codex 分支写 history |
| crates/engine/src/manifest.rs | Loop.allow_residual(validate 取值域)+ Codex.vet(validate 拒 ask+vet) |
| crates/engine/src/context.rs | StepOutput.items;RunContext.histories + interpolate 支持 history 字段;Severity 枚举与排序 |
| crates/engine/src/protocol.rs | ReviewResult.items;LoopConverged.residual(serde default) |
| crates/engine/src/runner/codex.rs | REVIEW_SCHEMA severity enum;raw_to_result 产 items;vet 二次调用 |
| crates/cli/src/render.rs | LoopConverged residual 渲染 |
| ui/src(Tauri GUI) | LoopConverged residual 展示;loop 决策门确认走现有 Decision gate 渲染(预期零改动,验证项);验证 Skipped-without-Started 渲染:P1 Skip(loop_id)与 P6 短路跳过的 body step 都会产生无 StepStarted 的 StepFinished(Skipped),GUI 按 step_id 渲染必须容忍(不出幽灵/崩溃) |
| templates/*.yaml | fix prompt 加 history 段;README 行为描述核对(P1 修后 README 声明成真,无需改) |
| crates/engine/tests/* | 每个 P 至少 2 个测试,见 §7 |

## 6. 风险

| 风险 | 缓解 |
|---|---|
| P1 改变存量 headless 用户的 run 结局(原 Success → 现多两条事件后 Skip 继续 Success) | 终局 RunStatus 不变(EOF-Skip 路径),只是多了显式审计痕;交互用户多一次确认,是修 bug 的预期代价 |
| severity enum 收紧后旧 codex 二进制被 OpenAI strict mode 拒 | 与 2026-06-26 suggestion 字段同一策略:锁版本工具,升级 codex 同步;fallback 路径 ChangesRequested 不假成功 |
| vet pass 把成立的 finding 误驳(过度反驳) | vet prompt 要求「驳回必须给代码证据」;opt-in 默认关;审计 NDJSON 里首轮/vet 后两份 findings 都有 progress 痕可比对 |
| 锚点短路改变多 codex body 的执行次序预期 | validate 不拦(合法编排),文档写明锚点语义;canonical 模板不受影响 |
| history 无截断在极端长 findings 下膨胀 prompt | 观测到再治,max=10 轮上限天然封顶;不预建截断 |

## 7. 验收

- `cargo test --workspace` 全绿;`cargo clippy --workspace --all-targets -- -D warnings` 0 warning。
- 新增测试(engine):
  - P1:max 耗尽 → 收到 StepAwaitingGate{Decision};Approve → 再跑 max 轮且 iteration 续号;Skip → StepFinished{Skipped} + run Success;Abort → RunStatus::Aborted;stdin-EOF 语义由 CLI 层现有测试覆盖。
  - P2:allow_residual=minor + 全 minor findings → LoopConverged{residual>0};含 major → 不收敛;items 空 + changes_requested → 不收敛(解析失败 fail-closed);未知 severity → 阻塞。
  - P3:vet 开启 + stub 二次输出裁剪 findings → 结果被替换;vet 调用失败 → 保留首轮;clean 首轮 → 不触发 vet(stub 断言只调一次)。
  - P4:同 id 三轮 record → history 含前两轮、带轮次分隔;直线 step history 空;`{{x.history}}` 插值正确。
  - P6:review 首轮即 clean → fix step 收到 Skipped、LoopConverged 在 fix 之前;未收敛轮 fix 正常跑。
- `demo/demo-task.yaml` 用 stub 跑通(README Quickstart 流程回归;mr-review-loop 含需人工贴 MR 链接的 human step,不适合 headless 回归,validate 通过即可)。
- 本 spec 与执行文档入仓。
