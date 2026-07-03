# parallel reviewers 设计（Phase C，rebase 2026-06-20 旧设计）

日期：2026-07-03
状态：待评审
取代：[2026-06-20-multi-agent-orchestration-design.md](2026-06-20-multi-agent-orchestration-design.md)（写于 src-tauri 单文件引擎时代，架构前提已变；产品决策沿用，实现方案按本 spec）
路线图定位：[2026-07-03-evolution-roadmap.md](2026-07-03-evolution-roadmap.md) Phase C

## 1. 背景

旧设计的目标拓扑不变：fan-out（同一输入分发给 N 个评审 agent 独立执行）+ fan-in（收集 N 份结果裁决）。这是"跨厂商对抗评审"的自然延伸——不同厂商 / 不同角色视角的盲区不重叠，多数决比单一 reviewer 更稳，恰好给 Phase B 无人值守下"单 reviewer 误判"兜底。

旧设计过时之处：

- 面向 `src-tauri/src/engine.rs`（tokio async、String-kind 大杂烩 Step、HashMap 产物）；现引擎在 `crates/engine`，纯同步（std::process + std mpsc），Step 是强类型 `StepKind` enum，产物经 context 管理，事件走 `Event` 协议 + NDJSON 审计。
- 旧 D4（until 通用化）/ D5（model/system 角色）当时標 Phase 2，本 spec 维持后置。

## 2. 目标（MVP = 旧设计的"命门"）

- 新增 `StepKind::Parallel`（fan-out）与 `StepKind::Aggregate`（fan-in），在同步引擎上实现。
- 端到端场景：3 个评审（如 claude 安全视角 / claude 性能视角 / codex review-mr）并发跑同一 MR，vote 多数决出裁决。

## 3. 非目标

- ❌ DAG 引擎重写、async 化（scoped threads + 并发度 4 足够，见路线图 §4）
- ❌ parallel 内嵌 human / loop / parallel（校验期拒绝，首版只允许 agent 类子步骤：claude / codex / acp）
- ❌ parallel 子步骤携带任何会打开交互门的配置：`verify`（unmet 走决策门）与 acp 的 `on_permission: ask`（权限走决策门）都在校验期拒绝——并发中的交互暂停语义 MVP 不定义，fail-closed 拒绝而非未定义行为。质量门放在 parallel 之后的 aggregate / verify 步骤上表达
- ❌ judge 策略与 until 通用化（Phase 2，见 §7）
- ❌ 结构化产物通道全面升级（vote 用轻量标记协议过渡，同旧设计 D3 判断）

## 4. 设计决策

### D1. StepKind::Parallel

```yaml
- id: fanout
  kind: parallel
  max_concurrency: 3        # optional,默认 4
  body:
    - id: rev_sec
      kind: claude
      prompt: "以安全视角评审 {{mr.artifact}},末行输出 verdict: pass 或 verdict: fail"
    - id: rev_perf
      kind: claude
      prompt: "以性能视角评审 {{mr.artifact}},末行输出 verdict: pass 或 verdict: fail"
    - id: rev_codex
      kind: codex
      action: review-mr
      base: main
```

执行语义（同步引擎实现）：

- `std::thread::scope` 内为每个子步骤起线程，简单信号量限并发（`max_concurrency` 默认 4，防子进程打满机器）。
- 产物合并：子线程不写共享状态，各自返回 `(step_id, artifact, metrics, result)`，scope join 后由主线程按 body 顺序串行合并进 context（并发默认不安全 → join 后串行写，沿旧设计 D6）。
- 事件：`Event` 的 sender 可 clone 到子线程；`StepStarted` / `StepProgress` / `StepFinished` 天然带 step_id，NDJSON 审计消费端是单 receiver 串行写，交错安全。CLI 渲染给并发期间的 progress 行加 step_id 前缀；GUI 并排多列（旧设计 §5 的 RunPanel 决策沿用）。
- 取消：`Control` 是 Send + Sync（AtomicBool + Mutex），引用传进每个子线程；abort 时各 runner 沿用自己的 SIGTERM → killpg 套路，scope 等全部子线程退出后统一收尾。
- 失败语义（fail-closed）：任一子步骤失败不立即杀兄弟（评审各自独立，杀了浪费已花成本），等全部结束后：若有失败子步骤，parallel step 整体走失败路径进决策门（重试 / 跳过 / 中止），产物中已成功的子步骤保留可引用。
- budget：子步骤 metrics 在 join 后统一 `charge_and_check`；并发期间不做轮间预算检查（成本粒度是 step 级，与现状一致）。检查时机后置意味着并发批次可能整体超一次预算，文档注明——预算是兜底不是精确闸门。
- gated 模式（RunMode 逐步门控）：门控发生在 parallel step 整体进入前（一次 GateKind::Step），body 子步骤不再逐个门控——并发中的逐步暂停语义与交互门排斥规则同因（见 §3 非目标末条）。

### D2. StepKind::Aggregate

```yaml
- id: verdict
  kind: aggregate
  inputs: [rev_sec, rev_perf, rev_codex]
  strategy: vote            # concat | vote,缺省 concat(fail-closed 最保守)
```

- concat：按 id 标注拼接各 input 产物为一段文本。零 agent 成本，是缺省。
- vote：从每份 input 提取一票：
  - codex review 子步骤：直接用结构化 `ReviewResult.verdict`（clean = pass），不走文本解析——这是相对旧设计的升级，2026-06-26 落的 verdict 结构化直接受益。
  - claude / acp 子步骤：解析产物末行 `verdict: pass|fail` 标记；解析失败按 fail（fail-closed，沿旧设计）。ACP 参与投票的脆弱性由多数决 + fail-closed 解析兜底（路线图 §4 的例外条款）。
  - 裁决：pass 票严格过半 → pass；平票 / 不足 → fail。产物 = 裁决结果 + 逐票明细（含"哪票是解析失败充 fail"，可解释）。
- 校验：`inputs` 非空、引用的 id 必须存在于此前步骤（含 parallel body 内的子 id）；strategy 未知值拒绝。
- aggregate 自身是纯计算步骤，无 agent 成本、无 metrics。

### D3. 与 loop 的组合（MVP 边界）

MVP 中 parallel / aggregate 可放进 loop body，但 `until` 收敛仍只认 codex-clean（现状 check 不变）——即"多评审 vote"在 MVP 里是单发裁决，不是循环收敛条件。`until: vote-pass`（看 body 内最后一个 aggregate 的结构化裁决）放 Phase 2，避免本片同时动收敛判定这块刚 harden 过的核心。

## 5. 错误路径盘点

- parallel body 含 human / loop / parallel / 重复 id / 带 verify 的子步骤 / on_permission: ask 的 acp 子步骤 → 校验期拒绝，可解释报错。
- 子线程 panic → scope join 时捕获，按该子步骤失败处理，不拖垮进程。
- 全部子步骤失败 → parallel 整体失败进决策门。
- vote 全票解析失败 → 全 fail → 裁决 fail（不放行），明细里逐票标注原因。
- abort 竞态：abort 到达时部分子步骤已完成 → 已完成产物照常合并落审计，未完成的走各 runner abort 路径，parallel 以 Aborted 收尾。

## 6. 测试

- 校验单测：body 白名单、id 引用、strategy 枚举、max_concurrency 边界。
- 并发单测：stub 二进制（demo/ 模式）3 子步骤并发，断言产物合并顺序确定、并发度上限生效（stub 里 sleep + 计数）。
- vote 单测：结构化票 / 标记票 / 解析失败票的混合矩阵，平票、过半、全失败。
- abort 传播：并发中途 abort，断言无僵尸子进程、事件终态正确。
- smoke：3-agent 评审 vote 全链路（stub），进 demo 场景。

## 7. Phase 2（本 spec 记录、不承诺）

- judge 策略（把 N 份产物 + 裁决指令喂给一个 agent 出结论）。
- `until: vote-pass` 循环收敛 + 修复步骤回路（多评审版 review-fix loop）。
- model / system 角色配置字段（需先核 claude runner 的参数注入点现状）。
