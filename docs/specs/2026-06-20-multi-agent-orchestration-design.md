# 多 Agent 编排：并行 + 汇总（方案 A 增量）设计

状态：设计中（待实施）
日期：2026-06-20
范围：src-tauri 执行引擎 + 数据模型 + 前端运行/编辑 UI

## 1. 背景与目标

agentpipe 当前是一个「带循环的线性管道」：`src-tauri/src/engine.rs` 单线程顺序遍历 `task.steps`，step 类型有 human / claude / codex / loop，产物是 `HashMap<String, String>` 纯文本，靠 `{{id.artifact}}` 模板传递。运行历史按 run 落盘（亮点），human step 暂停/恢复闭环完整。

产品目标是「本地、可编排不同 agent 协作 / 评审 / 交叉验证」。这三种场景的共同拓扑是：

- fan-out：同一输入分发给 N 个 agent 各自独立执行
- fan-in：收集 N 个结果，汇总成一个决策（拼接 / 投票 / 裁决）

当前引擎严格串行、产物纯文本、无汇总原语，这个拓扑无法表达，是目标的命门缺口。

本设计采用「方案 A 增量」：不重写为 DAG 引擎，而是在现有 steps 框架上新增两个 step 类型（parallel / aggregate），最小成本让 fan-out + fan-in 跑通，验证产品价值后再视情况演进到方案 B（结构化 DAG）。

## 2. 现状事实（实施基线）

数据模型 src-tauri/src/models.rs：

- Step 是 String-kind 大杂烩，已有字段：id、kind、instruction、prompt、skill、verify、expects、until、max、body（Vec Step）、action、base、path、timeout。关键：body 字段已存在（loop 在用），parallel 可直接复用。
- RunRecord.artifacts 类型为 HashMap String String（纯文本）。
- StepResult：step_id、status、output、artifact、iterations。

引擎 src-tauri/src/engine.rs：

- run_task 单个 for 循环顺序 await，match step.kind 分派到 run_human_step / run_agent_step / run_loop_step。
- 产物写入单个 artifacts map。interpolate 用正则替换 双花括号 id.artifact。
- check_until 硬编码只认 codex-clean，关键词 substring 匹配。
- 无 tokio::spawn 并发，无 join。

## 3. 设计决策

### D1. 新增 step 类型 parallel（fan-out）

复用已有 body 字段，但子 step 并发执行而非顺序。

执行语义：

- 引擎遇到 kind = parallel 时，对 body 中每个子 step 用 tokio::spawn 并发执行，join_all 等全部完成。
- 每个子 step 把产物写到自己的 step.id 下（与串行时一致，后续 aggregate / 模板可按 id 引用）。
- 并发度上限由新增可选字段 max_concurrency 控制（缺省给安全默认值，如 4），超出排队，避免一次拉起过多 agent 子进程打满机器。

新增字段（Step，全部 optional 向后兼容）：

- max_concurrency：u32，仅 parallel 用，缺省 4。

### D2. 新增 step 类型 aggregate（fan-in / 裁决）

把指定若干 step 的产物收集起来，产出一个汇总结论。

新增字段（Step，optional）：

- inputs：Vec String，要汇总的 step id 列表（如 parallel 内子 step 的 id）。
- strategy：String，汇总策略，取值见下。

strategy 三档（fail-closed 默认走最保守的 concat，不静默放行）：

- concat：把各 input 产物按 id 标注拼接成一段文本，作为本 step 产物。最简单，零 agent 成本。
- judge：把各 input 产物拼成上下文，连同本 step 的 prompt（裁决指令）喂给一个 agent（claude / codex，由 agent 字段指定），由 agent 输出裁决结论。这是交叉验证的核心模式。
- vote：从每个 input 产物里按约定标记（如 verdict: pass / fail）解析出票，多数决。解析不出标记的票按 fail 计（fail-closed）。产出投票结果 + 明细。

### D3. 结构化产物（最小化，Phase 1 不强求）

Phase 1 仍以纯文本 artifacts 为主，保证向后兼容与最小改动。vote 策略需要从文本里解析标记，约定一个轻量协议：agent 评审产物末尾输出一行 verdict: pass 或 verdict: fail。解析失败按 fail。

结构化产物（artifacts 升级为带类型的 JSON 通道）留到 Phase 3，届时 vote / judge 可消费结构化 verdict，不再靠文本解析。

### D4. until 通用化

把 check_until 从硬编码 codex-clean 改为可配：

- until 取值优先按「子 step id + 期望标记」解释：如 until = review:verdict=pass 表示看 review step 产物的 verdict 标记。
- 兼容旧 codex-clean（保留为内置别名）。
- 进一步可让 until 指向一个 judge step（由 agent 判定是否退出），留到 Phase 2。

### D5. agent 角色 / 模型配置

交叉验证的价值在「不同视角互验」。新增字段（Step，optional）：

- model：String，传给 agent 的模型参数（claude --model / codex 对应参数）。
- system：String，角色 system prompt（如「你是安全视角评审者」）。

claude 调用补 --model 与角色 system prompt；codex 同理。缺省不传，行为与现在一致。此项可与 Phase 1 同步做（改动小），也可拆到 Phase 2。

### D6. 并发下的取消 / 暂停 / 产物写入

- 取消：现有 cancel AtomicBool 需传入每个并发子任务，子任务在边界检查并能 kill 自己的子进程。
- 暂停 / human：parallel 内不允许出现 human step（首版约束，校验期拒绝），避免并发暂停语义复杂化。
- 产物写入：并发子任务不直接写共享 artifacts map（数据竞争）。各子任务返回 (step_id, artifact)，join 后由主任务统一合并进 artifacts，串行写，避免锁与竞态（并发默认不安全，按最保守处理）。

## 4. 执行流程示例（交叉验证）

任务：3 个 agent 从不同视角评审同一 MR，多数通过则放行。

steps：

1. human（贴 MR 链接）→ 产物 mr
2. parallel，body：
   - claude，system=安全视角，prompt 含 mr → 产物 rev_sec
   - claude，system=性能视角，prompt 含 mr → 产物 rev_perf
   - codex，action=review-mr → 产物 rev_codex
3. aggregate，strategy=vote，inputs=[rev_sec, rev_perf, rev_codex] → 产物 verdict
4. （可选）loop / 条件：verdict=fail 则回到修复步

第 2 步三个评审并发执行；第 3 步收集三份产物投票。这正是当前引擎做不到的拓扑。

## 5. 前端最小改动

- 编辑：TaskEditor 支持 kind 选 parallel / aggregate；parallel 复用 loop 的嵌套 body 卡片渲染；aggregate 配 inputs（多选已有 step id）+ strategy 下拉。
- 运行：RunPanel 对 parallel step 用并排多列展示各子 step 的实时输出（交叉验证要并排看），其余沿用单步可展开。
- markdown 渲染长文本产物留到 Phase 3。

## 6. 分 Phase 实施

Phase 1（命门：fan-out + fan-in）

- models.rs：Step 加 max_concurrency / inputs / strategy（均 optional）。
- engine.rs：match 加 parallel（tokio::spawn + join_all + 并发度上限 + 产物 join 后合并）、aggregate（concat / vote / judge 三策略）。
- 校验：parallel body 内禁 human；inputs 引用的 step id 必须存在。
- 前端：TaskEditor 支持新 kind；RunPanel 并排展示 parallel 子步。
- 真实 smoke：构造 3-agent 评审 vote 流水线，端到端跑通。

Phase 2（灵活性）

- until 通用化（标记 / judge step）。
- agent model / system 角色配置（若 Phase 1 未带）。
- 条件分支 step（branch：按产物标记路由）。

Phase 3（体验 / 质量）

- 结构化产物（artifacts 升级 JSON 通道，vote / judge 消费结构化 verdict）。
- 流程可视化、markdown 渲染、运行历史回看 UI。
- 容错编排（step 重试 / 失败继续）。

## 7. 风险与边界

- 子进程爆炸：parallel 不限并发会一次拉起过多 agent 打满机器 → max_concurrency 默认 4 兜底。
- 数据竞争：并发写共享 artifacts → 改为 join 后串行合并。
- 取消语义：并发子任务需各自响应 cancel 并 kill 子进程，避免取消后僵尸进程。
- 向后兼容：所有新字段 optional，旧 task / 旧导出 YAML 不受影响；新 kind 旧引擎遇到会落 未知步骤类型 错误（可接受，属新版能力）。
- fail-closed：strategy 缺省 concat；vote 解析不出标记按 fail；这些保守默认不静默放行。

## 8. 不在本设计内

- 方案 B 完整 DAG 引擎重写。
- agent 插件注册扩展点（Phase 1 仍硬编码 claude / codex，新 agent 后续设计）。
- 跨 run 产物复用、全局 settings / 密钥管理。
