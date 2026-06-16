# AgentPipe

个人用的本地编排客户端:把一个研发任务从设计到合入的流程(brainstorm → 双重 review → 实现 → 代码闸门循环)编排成声明式配置,引擎解析后串行执行,把 Claude / Codex 当本地 CLI 黑盒调用。

当前为 Phase 1a:headless 引擎 + 命令行驱动。GUI(Tauri 壳)留作 Phase 1b。

## 构建

```bash
cargo build
cargo test
```

## 用法

```bash
agentpipe run <task.yaml>
```

`task.yaml` 见 `templates/`。可用环境变量覆盖 CLI 二进制(便于用 stub 测试):

- `AGENTPIPE_CLAUDE_BIN` — 默认 `claude`
- `AGENTPIPE_CODEX_BIN` — 默认 `codex`

stub 端到端示例:

```bash
mkdir -p /tmp/demo-repo
AGENTPIPE_CLAUDE_BIN=$PWD/tests/fixtures/stub-claude.sh \
AGENTPIPE_CODEX_BIN=$PWD/tests/fixtures/stub-codex.sh \
STUB_VERDICT=clean \
./target/debug/agentpipe run tests/fixtures/sample-task.yaml
```

## Step 类型

| kind | 说明 |
|---|---|
| claude | 让 Claude 一次性完成一个指令(可引用 skill);allow_writes 开写权限 |
| codex | Codex 审文档(review-doc)/ 审仓库改动(review-mr)/ 问一句(ask) |
| human | 人去做(通常在自己的 Claude Code 会话),引擎等批准与产物 |
| loop | 包一段子步骤,until: codex-clean 收敛或到 max 上限退出 |

## 模板

- `templates/full-pipeline.yaml` — 完整 8 步流程
- `templates/codex-gate-only.yaml` — 仅代码闸门循环(已有改动二次审)

## 设计文档

- `docs/specs/2026-06-16-design.md` — 架构与协议
- `docs/plans/2026-06-16-agentpipe-engine.md` — 实施计划
