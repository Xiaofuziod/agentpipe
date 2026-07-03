# AgentPipe 演进路线图（2026-07）

日期：2026-07-03
状态：已定稿（2026-07-03 与用户对话收敛，按推荐决策执行）
配套 spec：
- Phase A：[2026-07-03-acp-hardening-design.md](2026-07-03-acp-hardening-design.md)
- Phase B：[2026-07-03-watch-outer-loop-design.md](2026-07-03-watch-outer-loop-design.md)
- Phase C：[2026-07-03-parallel-reviewers-design.md](2026-07-03-parallel-reviewers-design.md)

## 1. 定位判断

AgentPipe 的内循环（单次任务内的质量收敛回路）已经闭环并打磨三轮：review-fix 循环、fail-closed 决策门、budget 上限、vet 自反驳、轮间记忆、锚点短路（见 2026-06-26 / 2026-07-02 两份 spec）。边际收益在递减。

下一阶段沿 Loop Engineering 的"外循环"方向演进：从「人手动触发的单次质量回路」变成「长驻的、自己找活的评审代理」。差异化定位不变——串行、fail-closed、可审计的对抗式评审管线；不做并行交互式 swarm（那是 Vibe Kanban / Conductor 的地盘）。fail-closed 与 NDJSON 审计在无人值守场景下从"设计洁癖"升级为"必要条件"，是这条路线的护城河。

## 2. 三个阶段

### Phase A：ACP 二等公民收编（最小，先行）

补齐 ACP 集成 MVP 有意留下的缺口，让 ACP step 与 claude / codex step 同等公民：

- A1 budget 洞：ACP step 不上报 cost，`budget_usd` 对其形同虚设 —— 校验期 fail-closed 硬拦 + 显式确认字段
- A2 verify 门：`Acp` 变体支持 `verify`
- A3 permission 反向请求：从一律拒绝升级为可选决策门
- A4 agent registry：`agents.toml` 把启动命令从模板里解耦，模板可跨机器分享
- A5 transcript 渲染质量：ToolCall / Plan 不再用 Debug 格式输出

### Phase B：watch 外循环（旗舰）

`agentpipe watch`：订阅工作源（GitHub PR，label + 作者白名单过滤），每来一个新 item 就用既有模板实例化一次 headless run（human step 用 `value` 预置机制注入），结果回写 PR 评论；决策门在 headless 下沿用 stdin-EOF → Abort 的既有语义并追加通知。预算双层兜底（单 run + 单日累计）。

### Phase C：parallel reviewers（评审广度）

把 2026-06-20 的 parallel / aggregate 设计 rebase 到 crates/engine 的同步引擎上：fan-out 多评审（不同厂商 / 不同角色视角）+ vote 多数决裁决，fail-closed 解析。旧设计写于 src-tauri 单文件引擎时代，架构前提已变，须按新 spec 收窄后实施。

## 3. 依赖与排序

- A1 是 B 的硬前置：无人值守下预算是第一道安全阀，ACP step 的计费洞必须先堵。
- A3 与 B 的"决策门降级为通知"共享 gate 机制，先 A 后 B 可少走弯路。
- C 不依赖 B，理论上可并行；排在 B 后是产品判断——先把无人值守跑通（差异化最大），再扩评审广度。
- Phase 内部切片见各自 spec 的"实施切片"节。

## 4. 明确不做（本轮决策，含理由）

- 同步引擎改 async：parallel 的并发度上限本就设计为 4，scoped threads 足够；async 重写是为假想需求付大代价。ACP runner 已验证"专用线程内建 current-thread runtime"的隔离模式可行（crates/engine/src/runner/acp.rs 头部注释）。
- ACP-as-verifier（verify 门的 verifier 用 ACP agent）：verdict 语义只能靠 prompt 约定硬约，单点脆弱。例外：Phase C 的 vote 里 ACP 可参与投票——多数决 + 解析失败按 fail 的 fail-closed 兜底了单票脆弱性。
- swarm / kanban 方向：与竞品正面相撞，放弃差异化，不做。
- 通用 agent 插件抽象：维持 2026-06-25 ACP spec 的判断，ACP 已覆盖长尾接入，不再叠一层自有插件协议。

## 5. 验收基线

每 phase 独立走 spec → plan → 分片实施；完工基线沿用仓库现行标准：`cargo test --workspace` 全绿 + `cargo clippy --workspace --all-targets -- -D warnings` + stub 二进制真实 smoke（demo/ 模式）。
