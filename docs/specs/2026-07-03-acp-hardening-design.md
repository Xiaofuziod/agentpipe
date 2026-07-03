# ACP 二等公民收编设计（Phase A）

日期：2026-07-03
状态：待评审
前置：ACP MVP 已落地（PR #10，spec [2026-06-25-acp-integration-design.md](2026-06-25-acp-integration-design.md)）
路线图定位：[2026-07-03-evolution-roadmap.md](2026-07-03-evolution-roadmap.md) Phase A

## 1. 背景与问题

ACP MVP 有意留下五个缺口，代码内均有自认注释，现在按长期路线收编：

1. **budget 洞**：`AcpOutcome.metrics` 恒为 `None`（runner/acp.rs:87，MVP 未开 SDK 的 `unstable_end_turn_token_usage` feature），ACP step 完全不计入 `budget_usd`。现状只有一条一次性 eprintln 警告（runner/acp.rs:134）。无人值守（Phase B watch）场景下预算是第一道安全阀，这是阀上的洞。
2. **verify 门缺失**：manifest 注释明写"MVP 不带 skill / verify"（manifest.rs:89 附近）。ACP step 干的活没有质量门，与"deterministic gates"的产品主张矛盾。
3. **permission 一律拒**：反向 `permission/request` 统一回 `Cancelled`（runner/acp.rs:267-274）。fail-closed 正确，但期待交互授权的 agent 在 headless 语境外也干不了活，长尾兼容性打折。原 spec 自留了 V2 走 `GateKind::Decision` 的口子，评审修复后升级为专属 `GateKind::Permission`，避免 UI / CLI 按钮语义错位。
4. **agent 命令内联**：`command` 是 YAML 里逐 step 写死的完整命令。模板跨机器 / 分享即断（对方的 gemini 装在哪、什么 wrapper 都不同）。
5. **transcript 质量**：`format_update_for_log` 对 ToolCall / ToolCallUpdate / Plan 直接 `{:?}` Debug 输出（runner/acp.rs:410-412），replay / 审计可读性差。

## 2. 目标

- ACP step 与 claude / codex step 在 budget、verify、审计可读性上同等公民。
- 设了 `budget_usd` 的 manifest 不再可能"以为有预算兜底、实则有洞"。
- 模板中的 ACP step 可跨机器复用。

## 3. 非目标

- ❌ 接 SDK 的 `unstable_end_turn_token_usage`（仍 unstable；稳定后另起小改动填 `metrics`，本设计只留好位置）。
- ❌ ACP-as-verifier（verify 门的 `by` 不增加 acp 选项，理由见路线图 §4）。
- ❌ session/load 续接、HTTP transport 等 ACP V2 其余项。

## 4. 设计决策

### D1. budget 洞：校验期 fail-closed 硬拦 + 显式确认

候选：

- a) 仅在 validate 时打 warning（现状运行期 eprintln 的前移版）。不打断用户，但"warning 会被忽略"正是这类洞的成因。
- b) validate 硬拒 + manifest 顶层显式确认字段。【推荐】
- c) 给 ACP step 估算一个合成成本计入 budget。需要维护各 agent 定价表，估算错了比没有更糟，否决。

决策（b）：`Manifest::validate` 增加规则——`budget_usd` 为 `Some` 且 steps（含 loop body 递归）中存在 `Acp` 步骤时，返回 `EngineError::Validation`，错误信息列出未计费的 step id 并给出两条出路：去掉 budget_usd，或在 manifest 顶层显式声明：

```yaml
budget_usd: 5.0
allow_unmetered: true   # 我知道 acp step 不计入 budget,仍要跑
```

- 字段：`#[serde(default, skip_serializing_if = "is_false")] pub allow_unmetered: bool`，与 `worktree` 同模式，旧 YAML 无字段解析为 false，向后兼容。
- 运行期 eprintln 警告保留（runner/acp.rs:134 的 Once 逻辑不动）——validate 可被 SDK 嵌入方绕过，双保险。
- 将来 metrics 接通后，该字段仍保留：无法保证所有 ACP agent 都上报 usage，语义从"ACP 一律不计费"弱化为"存在可能不计费的 step"。

### D2. Acp 变体支持 verify 门

- `StepKind::Acp` 增加 `#[serde(default)] verify: Option<Verify>` 字段。verify 的**裁判**（`verify_once`）确实与 step 类型解耦可直接复用；但 verify-retry **循环本体**当前是内联在 Claude match arm 里的 ~120 行（executor.rs:220-340，与 claude.run / skill / read_only 缠绕），不存在现成共享路径——实现方式定为：先抽出以"跑一次 attempt"为闭包参数的公共 verify-retry helper，Claude 与 Acp 分支共同调用，而非复制或"接入"。
- **消解在案决策 executor.rs:346-354（review §A finding #7）**：该决策因 ACP metrics 恒 None、budget 无法兜底重试烧钱，把 ACP step 排除在自动 retry / 决策门 Retry 之外（runner 失败直接 fail + request_abort）。本设计与其边界如下：
  - runner-Err 路径**维持现状不变**：ACP step 自身失败仍不进决策门重试。
  - verify-retry 仅覆盖"step 成功但校验未达标"场景，重试次数受 verify 的 max_retries（默认 2）与全局 MAX_VERIFY_RETRIES 硬顶双重约束。设置 `budget_usd` 时，该场景已被 D1 的 `allow_unmetered` 显式签字覆盖；未设置 `budget_usd` 时没有签字环节，但此时本就没有预算兜底预期，不计费重跑仍是有界行为。
- metrics 累积走既有 `StepMetrics::sum` SSOT（protocol.rs:35）：ACP 侧为 None、verifier 侧有值时自然求和，无需特判。
- validate 规则与 claude 的 verify 校验对齐（既有规则适用什么就沿用什么）。
- `skill` 字段不加：skill 注入是 claude CLI 专属机制，ACP 协议无对应物。

### D3. permission 反向请求：可选权限门

候选：

- a) 维持一律 Cancelled。零成本，但长尾 agent 兼容性问题永远在。
- b) per-step 策略字段 `on_permission: reject | ask`，默认 reject。【推荐】
- c) 全局 config 开关。粒度太粗——同一 pipeline 里可信 agent 和陌生 agent 应可不同策略。

决策（b）：

- manifest：`Acp` 变体增加 `#[serde(default)] on_permission: PermissionPolicy`（enum：`Reject`（default）/ `Ask`），旧 YAML 缺字段 = Reject = 现行为，向后兼容且 fail-closed。
- runner API：`AcpRunner::run` 增加一个权限回调参数（与 `on_progress` 同风格，主线程调用）。worker 线程的 `on_receive_request` handler 不再直接回 Cancelled，而是把请求（工具名、agent 提供的 options 列表）经 std mpsc 发给主线程，并在 `tokio::sync::oneshot` 上 `.await` 答复——**必须 await 而非阻塞式 recv**：worker 是 current-thread runtime，handler 里阻塞会连带冻结同一 `select!` 的 timeout / abort 分支，spec 承诺的墙钟兜底恰在权限等待路径失效。主线程 drain loop 里 try_recv 权限请求、调回调拿决策后经 oneshot 回填。`Reject` 策略下回调恒返回拒绝，行为与现状完全一致。
- executor：`Ask` 策略时回调映射到专属 `GateKind::Permission`（机制沿用决策门的阻塞等待与命令通道，按钮 / CLI 文案改为"批准 / 拒绝 / 中止"）：
  - Approve → 在 agent 提供的 options 里选 allow 类选项（优先一次性 allow；agent 未提供 allow 选项则回 Cancelled 并在 transcript 记一行）
  - Skip → `RequestPermissionOutcome::Cancelled`（拒绝该次请求，session 继续）
  - Abort → 翻 abort 标志，走既有 abort 收尾（notify → session 取消）
- headless 语义自动正确：CLI 的 stdin-EOF 对 Decision / Permission 门都是 Abort，watch / headless 下 `Ask` 策略的权限请求会中止 run 而非挂死或静默放行，fail-closed 不破。
- 等待期间无超时，与 human 门一致；但 ACP step 的整体墙钟超时（`AGENTPIPE_ACP_TIMEOUT_SECS`）仍然生效，挂死有兜底。**注意**：门等待的时间会被算进 step 墙钟，文档需注明交互场景下必要时调大该 env。

### D4. agent registry

- 新增 `base_dir()/agents.toml`。`base_dir()` = `$AGENTPIPE_HOME 或 $HOME` 拼 `/.agentpipe`——注意既有语义是 AGENTPIPE_HOME **替代 HOME**、`.agentpipe` 仍拼接（cli/main.rs:44-50 的 runs 目录同构），不是替代整个 `~/.agentpipe`。`runs_dir()` 目前是 cli crate 的 pub(crate)，需抽成 engine 公共 helper，供 runs / agents.toml / watch state 三处共用：

```toml
[agents.gemini]
command = "gemini --experimental-acp"

[agents.claude-acp]
command = "npx @agentclientprotocol/claude-agent-acp"
```

- manifest：`Acp.command` 从 `String` 改为 `Option<String>`。解析优先级：step 内联 `command` > registry 按 `agent` 名查找 > 校验错误（错误信息提示两条出路）。既有 YAML 都带 command，不受影响。
- registry 文件不存在 = 空表（仅使用内联 command 的用户无感知）；文件存在但 TOML 解析失败 = fail-loud 报错，不静默当空表（防"改了 registry 没生效"的静默漂移）。评审修复后补充约束：宿主先用 `needs_registry(manifest)` 判定是否存在 `command: None` 的 ACP step（递归含 loop body），只有真依赖 registry 时才读取 `agents.toml`；纯内联 command 或无 ACP 的 manifest 完全不触碰坏 registry 文件。
- 解析是运行边界的独立 pre-pass：CLI run / validate 与 Tauri launch 加载 manifest 后，按需读取 registry，再 `resolve_agents(&mut manifest, &registry)`，最后走运行态校验。`Manifest::validate()` 保持纯函数、无文件系统依赖，语义仍是运行态：校验"command 为 None → 可解释报错并提示两条出路"。跑到一半才发现 agent 缺失是不可接受的错误路径。
- 作者态与运行态校验分档：`validate_authoring()` 仅额外容忍 `Acp.command: None`，供 Tauri `load_template` / `save_manifest` 使用；它不读取 registry、不 resolve、不把本机 command 烤进模板。`Some("")` 空串仍拒绝，其余规则（budget × allow_unmetered、verify 等）与运行态一致。
- Phase 2（可选）：`agentpipe agents` 子命令列出 registry 内容，非必需。

### D5. transcript 渲染

- `format_update_for_log`：ToolCall / ToolCallUpdate 渲染为 `[tool] <title/kind> <status>` 一行式，Plan 渲染条目数与首条摘要；不再 `{:?}`。
- 截断统一走既有 `truncate()`；未知 update 类型保留兜底分支。
- 纯展示层改动，不碰协议与 answer 聚合。

## 5. 错误路径盘点

- budget_usd + Acp step 且无 allow_unmetered → validate 期 Err，带 step id 列表与修复建议（可解释报错）。
- 真依赖 registry 的 manifest 遇到 TOML 坏文件 → fail-loud Err；不依赖 registry 的 manifest 不读取坏文件；registry 缺条目且无内联 command → validate Err。
- permission Ask 下 agent 不提供任何 allow 选项 → Approve 退化为 Cancelled + transcript 记录（不 panic、不挂死）。
- permission 门等待期间用户 Abort / Control 中止 → 走既有 abort 通路，oneshot 发送端 drop，worker 侧收到关闭即回 Cancelled 后退出。
- 反方向：门等待期间 worker 侧墙钟超时先到 → oneshot 接收端 drop，人随后的应答回填失败——定义为忽略 + transcript 记一行，step 终态以 worker 侧超时 Err 为准（不静默吞、不 panic）。
- 已知边界（记录，非回归）：门等待期间主线程 drain loop 暂停，仅翻 Control 标志、不走 Command 通道的嵌入方在此期间无法中止 run——与既有 human / decision 门行为一致。
- verify-retry 中 ACP step 反复失败 → 与 claude 路径同形，走 OnUnmet 语义，无新分支。

## 6. 测试

- validate 单测：budget × Acp × allow_unmetered 的 2×2×2 组合；registry 缺失 / 坏文件依赖与非依赖 / 优先级覆盖。
- runner 单测：扩展 `examples/mock_acp_agent.rs` 支持发 permission/request，验证 Reject / Ask-Approve / Ask-Skip / Ask-Abort / 无 allow 选项五条路径。
- executor 单测：Acp + verify 门的 unmet-retry-met 链路（stub runner）。
- 渲染快照测试：ToolCall / Plan 的 format_update_for_log 输出稳定。
- smoke：demo stub 流程加一个带 verify 的 ACP step 场景。

## 7. 实施切片（每片独立可 review / 可回滚）

1. D1 budget 硬拦（validate + 字段 + 测试）——最小且是 Phase B 硬前置
2. D2 verify 门
3. D5 transcript 渲染（顺手小片）
4. D4 agent registry
5. D3 permission 决策门（最大的一片，涉及 runner API 变更，放最后单独消化）
