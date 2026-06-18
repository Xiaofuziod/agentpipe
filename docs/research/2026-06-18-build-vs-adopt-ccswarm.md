# 自建 AgentPipe vs 采用 ccswarm:取舍调研

日期 2026-06-18。回答一个一直没正式记下来的问题:既然 ccswarm 是"最贴 AgentPipe 实现"的开源项目,为什么不直接用它、而是自建?本文把两边都查实,给出可复盘的决策依据。

配套:`docs/research/2026-06-18-ccswarm-conductor-comparison.md`(能力逐项对照,本文是其上的"采用 vs 自建"决策层)。

## 0. 可信度声明(先说边界)

- AgentPipe 一侧是**自有代码事实**(本仓库,可直接核)。
- ccswarm 一侧来自其 **GitHub repo + 自述文档**(README / CHANGELOG / ARCHITECTURE.md / COUPLING_REPORT.md / APPLICATION_SPEC.md / 源码抽样)+ web 检索,2026-06-18 采集。下文事实尽量标注 `[已验证]`(primary source 看到)/ `[声明]`(仅文档宣称,未独立交叉验证)。ccswarm 演进很快(pre-1.0),数据有时效性。

## 1. 问题

"多 agent / 多 CLI 编排"这件事,ccswarm 与 AgentPipe 同物种(Rust + 把 provider CLI 当黑盒、声明式 flow、闸门、NDJSON 审计)。既然这么像,理性的问题是:**直接采用 ccswarm 能不能省掉自建?** 标准:功能契合、可控性、扩展性、成熟度/风险、采用与迁移成本、长期维护。

## 2. 两个候选的事实画像

### 2.1 ccswarm(nwiizo/ccswarm)

| 维度 | 事实 | 标注 |
|---|---|---|
| 成熟度 | v0.9.1,pre-1.0,minor 版本间已有破坏性删除(v0.7.0 删模块/collapse 抽象) | 已验证(CHANGELOG) |
| 维护 | ~141 stars;**有效贡献者 1 人**(nwiizo + AI 助手 commit);0 open issues(低使用或不用 issue 跟踪) | 已验证(commit/issues) |
| 平台 | Linux / macOS;**Windows 明确不支持**(Unix 依赖) | 已验证(APPLICATION_SPEC) |
| 宿主 | Claude Code 全支持(硬编 `--dangerously-skip-permissions`);Codex 仅非交互 exec;Copilot 表里列出但标"Unsupported" | 已验证(providers) |
| 能力 | flow YAML(sequential/parallel/sangha 共识/team_leader/workflow-call)、HITL 门(plan/risky-edit/deploy/merge/commit)、NDJSON 审计 + run diff + cost + replay + advisory undo、git worktree 隔离、faceted prompt(persona/policy/knowledge 分层)、repertoire 包、`--max-budget-usd` 预算、`flow eject` 改默认流 | 已验证(混合) |
| 扩展性 | **自评耦合 D 级(0.44/1.0)**;COUPLING_REPORT 原话:"lack of trait-based abstractions means extensions require modifying core modules" + "Circular dependencies and high coupling make selective feature adoption challenging(fork 难)" | 已验证(其自评文档) |
| GUI | **无,明确 out of scope**;竞品对比里自承落后于"GUI diff review" | 已验证 |
| 自定义校验门 | 限文本匹配 / LLM judge;**无 shell 命令型 verifier / 插件接口**(跑你自己的测试当门做不到) | 已验证/推断 |
| 上下文传递 | prompt 文本注入,非 session 状态;长流水多轮连贯性依赖 CLI 版本,ARCHITECTURE 自承"variability" | 已验证 |
| 输出解析 | ai-session 用正则启发式,"handles complex multi-line outputs inconsistently" | 已验证(原话) |
| 测试 | 声明 559 测试,但 HARNESS doc 自承 file/command/metric oracle"planned rather than implemented",集成测 CI flaky | 声明 + 已验证(自承不全) |
| 许可 | MIT | 已验证 |

### 2.2 AgentPipe(本仓库,2026-06-18)

| 维度 | 事实 |
|---|---|
| 规模 | engine 2263 + cli 403 + tauri 433 + ui 2113 ≈ **5200 行**,小到一个人能完整 hold |
| 测试 | **80 cargo + 24 vitest = 104**,全绿;关键路径有序列化/契约/边界测试锁定 |
| 引擎 | manifest/executor/runner(claude/codex)/audit/control/context/worktree;step 类型 claude/codex/human/loop;verify 门(codex/claude/**command** exit-code)、codex-clean loop、fail-closed 收敛 |
| 审计 | NDJSON 落盘 + `runs/view/cost/diff` + run-id allowlist(防穿越)+ 同秒撞名退避 |
| 隔离 | 任务级 git worktree(`worktree: true`,fail-closed) |
| GUI | Tauri 三栏:历史(持久化/成本/只读回看/两两 diff)、控制台(实时逐 task 进度:状态/当前行/轮次/秒表/成本)、编排(可视化建 task.yaml + verify 门 codex/claude/command) |
| 契合 | 专为 Claude Code + Codex 双厂商互检、你的 skills、MR/记忆约定;串行可控、每步可审可回滚 |
| 文档 | 7 specs + 7 plans + 2 research,全入仓 |

## 3. 决策维度对照

| 维度 | ccswarm | AgentPipe(自建) | 谁更优(对"你的需求") |
|---|---|---|---|
| 桌面 GUI | 无(out of scope) | 有(Tauri 三栏,实时进度/历史/diff/编排) | **自建**(GUI 是硬需求) |
| 宿主契合 | Claude 中心,Codex 半,硬编 skip-permissions | Claude+Codex 对等,权限模式可控 | **自建** |
| 可控性 | 偏 sangha 共识/并行,自动化重 | 串行、每步可审、fail-closed、显式插值数据流 | **自建**(你要可控) |
| 扩展性 | 自评 D 级、"扩展须改核心、fork 难" | 5200 行全懂,加 step/verifier 是局部改 | **自建** |
| 校验门灵活度 | 文本/LLM judge,无命令型 | command verifier(exit code 即 verdict)+ codex/claude | **自建** |
| 成熟度风险 | pre-1.0 + 单人 + minor 破坏性 + bus-factor 1 | 自己掌控,无外部 abandonment 风险 | **自建** |
| 现成审计/编排原语 | 已有(NDJSON/replay/diff/cost/sangha/facets/repertoire) | 多数自己实现了(审计/diff/cost),sangha/facets/repertoire/replay 没有 | **ccswarm**(原语更全) |
| 多 agent 协作 | sangha 共识、team_leader、并行 | 仅串行 + loop,无投票/并行 | **ccswarm** |
| 采用/迁移成本 | 学 facet/路由/门体系 + 为 GUI 从零包壳 + 可能 fork | 已建成,边际成本 = 继续加功能 | **自建**(沉没成本已付且小) |
| 平台 | Linux/macOS | macOS 优先(cfg(unix)) | 平手(你在 mac) |

## 4. 取舍分析

### 4.1 "其实可以采用 ccswarm"的情形(诚实列出)
如果下面**全部**成立,采用 ccswarm 会比自建省时间:
- 不需要桌面 GUI(终端够用);
- 接受 Claude 中心 + 硬编 skip-permissions;
- 接受 pre-1.0 单人项目的 abandonment / 破坏性升级风险;
- 不需要命令型校验门、不需要深度自定义 step 类型(facet 级 prompt 定制够);
- 愿意吃 D 级耦合带来的"任何非平凡扩展都要改核心 / fork 难"。
这种情况下,ccswarm 的 NDJSON 审计 + flow 引擎 + sangha + facets 是数周的现成功劳,直接用更划算。

### 4.2 为什么对"你的需求"自建胜出
你的需求恰好踩在 ccswarm 的每个短板上:
1. **GUI 是硬需求** —— ccswarm 给 0,自建给了全套 Tauri(且实时进度/历史回看/diff/可视化编排)。光这一条就基本定调:用 ccswarm 也得从零包一个壳读它的 NDJSON,等于自建一半。
2. **可控 + fail-closed + 双厂商互检**是你的设计取向 —— ccswarm 偏 sangha 共识/并行/自动化,哲学不同;权限还硬编 skip-permissions,你想要的"可控"它给不了。
3. **扩展性** —— 你会持续按自己工作流加东西(command verifier、worktree、MR-loop 模板都是后加的)。AgentPipe 5200 行全懂、加功能是局部改;ccswarm 自评 D 级"扩展须改核心、fork 难",在它上面长你的东西成本更高、还要对抗它的破坏性升级。
4. **风险/掌控** —— 依赖一个 141 star、单人、pre-1.0、minor 间破坏的项目,等于把核心工具押在一个人的兴趣上;自建无此风险。
5. **沉没成本其实很低且已转化为资产** —— AgentPipe 才 ~5200 行、104 测试、文档齐全,不是"重造大轮子",而是"造了个正好合手的小轮子"。

### 4.3 一句话结论
**不是"评估后否决 ccswarm",而是"你的需求(GUI + 可控 + 可深度扩展 + 低风险)正好落在 ccswarm 的结构性短板上,自建的边际成本又很低,所以自建胜出;ccswarm 当 idea 库。"** 这与最初对话的实际走向一致(从来不是二选一,而是"已在自建 + 去 ccswarm 扒思路")。

## 5. 触发重新评估的条件(什么时候该回头看 ccswarm)
- 你决定**放弃 GUI**、回到纯终端;
- 你需要**多 agent 并行/共识投票**(sangha)且不想自己实现;
- ccswarm 走到 **1.0 + 多贡献者 + 重构掉 D 级耦合 + 暴露稳定扩展点(trait/插件/HTTP API)**;
- 你需要它已有而自建没有的成套生态(faceted prompt 分层、repertoire 包分发)且不想自造。
以上任一长期成立,再做一次本调研。

## 6. 仍值得从 ccswarm 借鉴的(backlog,非采用)
按价值排序,继续"扒思路"而非"装依赖":
1. **sangha 式多 verifier 共识** —— verify 门的 `by` 扩成"N 个 verifier 投票,达 quorum 才过"(对应已记的 backlog)。
2. **`--max-budget-usd` 预算上限** —— 给 run/step 设成本/轮次硬上限,超了停(配合已有 cost 聚合)。
3. **replay** —— ccswarm 有;AgentPipe 之前判为非目标(LLM 非确定性)。若只做"按记录重放给人看"(非重执行),GUI 只读回看已覆盖大半。
4. **faceted prompt 分层** —— persona/policy/knowledge 的 builtin<user<project 覆盖链,比当前直插 skill 更结构化(长期,YAGNI 警惕)。
保持不借:sangha 的强制共识默认、硬编 skip-permissions、prompt 注入式上下文、正则输出解析。

## 7. 复盘备注
本调研补上了当时缺的"为什么不用 ccswarm"正式记录。核心教训:**"最贴的开源项目"≠"该采用的开源项目"** —— 契合度要看你的硬需求(这里是 GUI + 可控 + 扩展 + 低风险)落在对方的强项还是短板上,以及自建的真实边际成本(这里很低)。
