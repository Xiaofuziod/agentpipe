# ACP 集成设计 (agentpipe)

日期：2026-06-25
作者：claude (主) + 调研双 agent (Explore + Web 调研)
状态：草案,待 plan 拆解

## 0. 背景

agentpipe 当前通过两个具体 runner 驱动外部 AI CLI:
- `ClaudeRunner` ([crates/engine/src/runner/claude.rs](../../crates/engine/src/runner/claude.rs)):spawn `claude -p ... --output-format stream-json`,逐行解析 NDJSON 拿 answer/metrics。
- `CodexRunner` ([crates/engine/src/runner/codex.rs](../../crates/engine/src/runner/codex.rs)):spawn `codex` 拿结构化 JSON,解出 `Verdict::{Clean,ChangesRequested}` + findings。

引擎层是**纯同步** Rust(`std::process::Command` + `std::sync::mpsc`,见 [runner/mod.rs:22](../../crates/engine/src/runner/mod.rs)),无 tokio。

Zed 推动的 **Agent Client Protocol (ACP)** 已于 2026-06-24 发到 Rust SDK `1.0.0`(API 稳定),Claude Code / Codex CLI / Gemini CLI / 25+ agent 通过官方/适配器实现 server 侧,JSON-RPC over stdio 是当前唯一稳定 transport。([spec](https://github.com/agentclientprotocol/agent-client-protocol) / [Rust SDK](https://github.com/agentclientprotocol/rust-sdk) / [crate](https://crates.io/crates/agent-client-protocol))

## 1. 问题

1. 接入新厂商 agent(Gemini / Auggie / Factory Droid 等)目前要每家写一个 runner,重复劳动 + 输出协议各异。
2. 现有 runner 的进度信息有限:Claude 只能拆轮次 + 末段文本,Codex 只有终态 JSON;无法表达 ACP 已结构化的 ToolCall / ToolCallUpdate / Plan / Thought 流。
3. agentpipe 长期方向是 "跨厂商对抗式 pipeline",ACP 是事实标准的 client↔agent 协议,不接入会越来越落后。

## 2. 目标 (MVP)

- 让 agentpipe 能把任何**已实现 ACP server** 的 agent 作为 step 跑(spawn 子进程 + stdio JSON-RPC)。
- 不破坏现有 `Claude`/`Codex`/`Human`/`Loop` 四种 StepKind 的语义与 YAML 兼容性。
- 流式 `session/update` 通知映射到现有 `Event::StepProgress`,用户在 CLI / GUI 看到的体验跟 Claude step 一致。
- 接入 `Control::request_abort()`:先发 `session/cancel`,2s grace 不退则 killpg(沿用 [Control](../../crates/engine/src/control.rs) 的现有套路)。
- 单元测试不依赖任何真实外部 agent;CI 全绿。

## 3. 非目标 (MVP 不做,留 V2)

- ❌ **反向 capabilities**:`fs/read_text_file` / `fs/write_text_file` / `terminal/*` 由 client 提供给 agent。MVP **不声明**这些能力,让 agent 走自己的本机 fs(claude-agent-acp / codex-acp 默认行为)。
- ❌ **permission/request 反向通道**:MVP 在 initialize 阶段告诉 agent 走自己的 default 权限模式;若 agent 仍发请求,统一 reject(fail-closed)。完整 UI 等 V2(可复用现有 `GateKind::Decision`)。
- ❌ **session/load 续接 / session/fork**。每个 step 一次性 new session,跑完 close。
- ❌ **HTTP/WebSocket transport**。spec 主页写明 "Full support for remote agents is a work in progress",MVP 锁 stdio。
- ❌ **多 session 并发**。一个 step 一个 session,串行。
- ❌ **ACP step 作为 `verify` 门的 verifier**。verify 门现有 codex/claude/command 三种已够用,ACP step 想做 verifier 需在 prompt 里要求固定 verdict 标记,机制非 trivial,V2 再开。
- ❌ **替换 Claude/Codex runner**。两类 runner 各有 stream-json / 结构化 JSON 的成熟解析,ACP adapter 多一层(claude-agent-acp 还要套 npx → node),性能与可观测都没好处。并行存在,用户按 step 选。

## 4. 候选方案

### 方案 A:新增 `StepKind::Acp`,与 Claude/Codex 并列【推荐】

- 在 [manifest.rs:40](../../crates/engine/src/manifest.rs) 加变体 `Acp { agent, prompt, skill?, env?, args?, verify? }`。
- 新建 [crates/engine/src/runner/acp/](../../crates/engine/src/runner/) 子模块:对外 `AcpRunner::run(prompt, ..., on_progress, cwd) -> AcpOutcome { answer, metrics, full_transcript }`,签名跟 `ClaudeRunner::run` 同形。
- 内部用 `tokio::runtime::Runtime::new()?.block_on(...)` 把 SDK 的 async API 包成 sync(engine 主线程 zero 改动)。
- Executor 加一条 `StepKind::Acp` 分支(对照 [executor.rs](../../crates/engine/src/executor.rs) 现有 Claude/Codex 分支)。
- 优势:
  - 增量改动,既有 YAML / 既有 runner 不受影响;失败可单独回滚 step 不影响整 pipeline。
  - tokio 隔离在 ACP 子模块内,engine 其他模块继续 sync。
  - 用户可以**同时**用 Claude(原生 stream-json,跑得最稳)+ ACP-via-Gemini(尝鲜 + 跨厂商对抗)。
- 劣势:
  - 用户配置时多学一类 StepKind(但语义简单:换字段名 + agent 二进制路径)。
  - ACP runner 的 outcome 形状跟 Claude/Codex 不完全相同(无 turn 概念,Plan/ToolCall 是结构化事件);需新建 `AcpOutcome` 而非复用 ClaudeOutcome。

### 方案 B:统一抽 `trait Runner`,Claude/Codex/ACP 都实现【拒绝】

- 抽 `trait Runner { type Outcome; fn run(...) -> Result<Self::Outcome>; }`,三 runner 都实现。
- 优势:架构干净,Executor 走多态。
- 劣势:
  - 三 runner 的方法签名/参数差异大(Claude 有 `skill` + `read_only`,Codex 有 `action: CodexAction` + `base/path`,ACP 有 `agent_id` + 反向 capabilities)。强行抽象会出现一堆 `Option<T>` 凑参数,反而劣化可读性。
  - Outcome 类型异构(`ClaudeOutcome` / `ReviewResult` / `AcpOutcome`),GAT 或枚举包装都增复杂度。
  - 现有两 runner 没痛点,trait 化是为 ACP 而抽,**未来真要加第四个 runner 时再抽不迟**(YAGNI)。
- 拒绝理由:为美感付实际工程税。

### 方案 C:用 ACP 重写 Claude/Codex runner【拒绝】

- 删 stream-json 解析,Claude 走 [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp)(TS npm 包),Codex 走 [codex-acp](https://github.com/agentclientprotocol/codex-acp)(Rust)。
- 优势:统一架构 + ToolCall 流式可见。
- 劣势:
  - claude-agent-acp 是 TypeScript,spawn 链 `rust → npx → node → claude binary`,启动慢 + 多一层故障点。
  - codex-acp 仓库刚迁移(`zed-industries/*` → `agentclientprotocol/*`),稳定性需观察。
  - 现有 verify 门 "VERDICT: pass|fail" 模式跟流式 ACP 对话不一致,要重设计。
  - **破坏向后兼容**:存量 manifest 全部要改;CLAUDE.md「一功能=一分支=一 MR」原则下这会变成大刀阔斧的迁移。
- 拒绝理由:风险/收益严重失衡,且方案 A 已能拿到 ACP 全部好处。

## 5. 推荐方案:A

理由:与 agentpipe 现有"runner = 厂商适配器"哲学一致;增量、可灰度、可回滚;tokio 污染仅限 acp 子模块。

## 6. MVP 范围划线

| 能力 | MVP | 备注 |
|---|---|---|
| Spawn external ACP agent (stdio) | ✅ | 经 `Cargo.toml` 依赖 `agent-client-protocol = "1"` + `agent-client-protocol-tokio = "*"` |
| initialize 握手 + protocolVersion 协商 | ✅ | 协商失败 fail-loud(对齐 review-mr base-ref fail-loud 教训) |
| session/new + session/prompt | ✅ | 一 step 一 session,跑完 close |
| 流式 session/update → progress_sink | ✅ | AgentMessageChunk / AgentThoughtChunk → 行文本;ToolCall / ToolCallUpdate / Plan → label |
| 终态 answer 提取 | ✅ | 聚合所有 AgentMessageChunk + 末段总结 |
| 终态 metrics | ⚠️ | spec 有 `session/update` 中的 token usage(unstable feature flag `unstable_end_turn_token_usage`),MVP 不开;`StepMetrics` 字段全 0 或 None |
| Control 中止 | ✅ | session/cancel → 2s grace → killpg |
| timeout 兜底 | ✅ | 复用 `AGENTPIPE_CLAUDE_TIMEOUT_SECS` 同名约定:`AGENTPIPE_ACP_TIMEOUT_SECS` |
| 单元测试 (mock agent) | ✅ | fixture binary,见 §8 |
| 集成测试 (真实 agent) | ⚠️ 可选 | 仅 `AGENTPIPE_ACP_INTEGRATION=1` 时跑 |
| 反向 fs / terminal | ❌ | V2 |
| 反向 permission/request | ❌ MVP reject | V2 接 GateKind::Decision |
| session/load / fork | ❌ | V2 |
| HTTP / WebSocket transport | ❌ | 等 spec 稳定 |
| ACP step 当 verify 门 | ❌ | 现有 codex/claude/command 三种够用 |

## 7. 关键设计决策

### 7.1 同步外壳包 async SDK

```rust
// crates/engine/src/runner/acp/mod.rs (示意,不是最终代码)
pub struct AcpRunner { config: AcpConfig, timeout_secs: u64 }

impl AcpRunner {
    pub fn run(&self, prompt: &str, ..., on_progress: &mut dyn FnMut(&str, Option<u32>),
               cwd: &Path) -> Result<AcpOutcome, EngineError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all().build()
            .map_err(|e| EngineError::Cli(format!("acp: tokio rt: {e}")))?;
        rt.block_on(async { self.run_inner(prompt, ..., on_progress, cwd).await })
    }
}
```

`new_current_thread` 而非 multi_thread:单 step 单 session,不需要 worker pool;且与 engine 主循环线程模型对齐(简化进程组继承)。

### 7.2 ACP unstable_* features 全禁

`agent-client-protocol` crate 暴露一堆 `unstable_*` feature flag(unstable_protocol_v2 / unstable_mcp_over_acp / unstable_session_fork 等)。MVP **一个不开**,锁 v1 stable wire。Cargo.toml 显式不声明任何 features。

### 7.3 协议版本协商 fail-loud

initialize 返回的 `protocolVersion` 若不在 SDK 1.x 支持的范围,**立即** `Err(EngineError::Cli(...))`,不静默继续(对照 review-mr base-ref 教训:[a800c7f](https://github.com/Xiaofuziod/agentpipe/commit/a800c7f) 把 base 不存在从 changes_requested 静默循环改成 fail-loud)。

### 7.4 反向请求 MVP 策略

- `fs/read_text_file` / `fs/write_text_file` / `fs/list_directory` / `terminal/*`:**不声明** capability,SDK 自动 unsupported 响应。agent 自己用本机 fs(已是 claude-agent-acp / codex-acp 默认行为)。
- `permission/request`:声明 capability 但实现里 **统一返回 Cancelled**。effectively fail-closed,保险起见日志打 warn。完整 UI 集成走 V2。

### 7.5 答案提取

聚合策略(优先级递减):
1. 末次 `session/update` 中类型为 AgentMessageChunk 的 `content.text` 全量拼接;
2. 若步骤过程出过 ToolCall 且无 AgentMessageChunk,取末次 ToolCallUpdate 的 `result` 字段;
3. 都空 → `EngineError::Cli("acp: agent 未返回任何文本")` fail-loud,不返回空串(对齐 Claude runner 的 answer 回退链 fail-closed 哲学)。

### 7.6 安全 / 范围隔离

- ACP agent **仍在 cwd 内跑**(传 cwd 给 `session/new` 的 metadata)。
- worktree 模式([crates/engine/src/worktree.rs](../../crates/engine/src/worktree.rs))自动生效:executor 跑 ACP step 时 cwd 已被换成 worktree 路径,与 Claude/Codex step 同。
- bypassPermissions / 危险操作的边界**完全由 ACP agent 本身负责**(claude-agent-acp 内部有自己的权限模型),MVP 不再加一层。

## 8. 测试策略

### 8.1 Fixture mock ACP agent

新建 `crates/engine/tests/fixtures/mock_acp_agent/`,一个最小 Rust binary(可选 Cargo example,见 §10 验收):
- 接 stdin/stdout 走 JSON-RPC;
- 行为按 env 变量切换(`MOCK_ACP_SCENARIO=happy|empty|long_stream|cancel_test|version_mismatch`);
- 不依赖 agent-client-protocol crate(手写最小 JSON-RPC 即可,避免循环依赖),保证测试稳定。

### 8.2 必覆盖场景

| 场景 | 期望 |
|---|---|
| Happy path | initialize 成功 → prompt → 3 个 chunk → final answer 拼接正确 |
| 协议版本不匹配 | EngineError::Cli, 不静默继续 |
| Agent spawn 失败 | EngineError::Cli, 带 binary 路径 |
| 中途 cancel | Control::request_abort 后 ≤ 2s 内退出, full_transcript 截断 |
| 超时 killpg | 设小 timeout,过点后整组被杀 |
| Agent 返回空内容 | EngineError::Cli, 不返回空串 |
| permission/request 被拒 | mock agent 收到 Cancelled,不卡死 |

### 8.3 集成测试 (opt-in)

`tests/acp_integration.rs`,gated by `AGENTPIPE_ACP_INTEGRATION=1`,跑真实 `npx @agentclientprotocol/claude-agent-acp`(开发本机用)。CI 不跑。

## 9. 风险与缓解

| 风险 | 缓解 |
|---|---|
| ACP SDK 1.x patch 频繁(过去 30 天 5 个版本) | `Cargo.toml` 锁 `agent-client-protocol = "1"` 收 patch,禁 unstable_* features;每月 review |
| tokio 拉进 engine 增加体积 | 仅 `acp` 子模块用,主路径 sync 不变;tokio = "1" 用 default-features 子集(rt + macros + io-util + process,关 net) |
| Windows 进程组语义不同 | agentpipe 现状本就主要 macOS/Linux;ACP runner 沿用 [run_command](../../crates/engine/src/runner/mod.rs) 的 `#[cfg(unix)]` 进程组路径,Windows 走降级 |
| spawn `claude-agent-acp`(npm) 启动慢 | 用户责任(自己装 + 配 binary 路径),MVP 不做 npx 自动管理 |
| 反向请求被拒导致 agent 卡死 | MVP 不实现 fs/terminal,但 SDK 会发 unsupported 响应,不会卡;permission reject 同理 |
| 答案空 → fail-loud 可能漏抓真实"agent 无话可说"场景 | 接受:对齐 fail-closed 哲学,宁可显式失败也不静默成功 |

## 10. 验收 (MVP 完工标准)

- `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test --workspace` 全绿。
- `crates/engine/src/runner/acp/` 子模块编译并被 Executor 调用。
- `crates/engine/tests/acp_runner.rs` 覆盖 §8.2 全部 7 个场景,无 ignored 测试。
- `templates/` 加一个 ACP step 示例 YAML(挑 Gemini 或 Codex-via-ACP 一个)。
- 至少 1 个 manifest 示例能在真实 agent 上跑通(开发本机验证,CLAUDE.md 不强制 CI 跑)。
- 文档:`README.md` 加一节"使用 ACP agent"指向本 spec。

## 11. 引用

- spec: <https://github.com/agentclientprotocol/agent-client-protocol>
- Rust SDK: <https://github.com/agentclientprotocol/rust-sdk>
- yolo_one_shot_client.rs 示例: <https://github.com/agentclientprotocol/rust-sdk/blob/main/src/agent-client-protocol/examples/yolo_one_shot_client.rs>
- claude-agent-acp: <https://github.com/agentclientprotocol/claude-agent-acp>
- codex-acp: <https://github.com/agentclientprotocol/codex-acp>
- review-mr base-ref fail-loud 教训:commit a800c7f
