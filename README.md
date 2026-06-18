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
| claude | 让 Claude 一次性完成一个指令(可引用 skill);一律以 CLI 最高权限(bypassPermissions)跑 |
| codex | Codex 审文档(review-doc)/ 审仓库改动(review-mr)/ 问一句(ask) |
| human | 人去做(通常在自己的 Claude Code 会话),引擎等批准与产物 |
| loop | 包一段子步骤,until: codex-clean 收敛或到 max 上限退出 |

## 模板

- `templates/full-pipeline.yaml` — 完整 9 步全自动流程(brainstorm → Claude 审 → Codex 审 → 出执行文档 → 写码建 MR → code-review + simplify → Codex 审 MR 循环到干净 → 存记忆)
- `templates/mr-review-loop.yaml` — 输入 MR 链接 → Codex 审 MR → Claude 按反馈修复并 push → 再审,循环直到 Codex 判定干净(`base` 改成该 MR 的目标分支)

## GUI(Tauri 桌面端)

```bash
cargo tauri dev            # 开发启动(自动起 ui vite + tauri 窗口)
# 或仅前端: cd ui && npm run dev
```

功能:左侧历史记录(持久化于 ~/.agentpipe/runs/,含成本,可两两对比),中间控制台(实时执行 / 历史只读回看 / 底部 prompt 快速运行),右侧编排(可视化建 task.yaml,含 verify 校验门 codex/claude/command)。

stub 演示同 CLI:设 `AGENTPIPE_CLAUDE_BIN` / `AGENTPIPE_CODEX_BIN` 指向 `tests/fixtures` 下的 stub 脚本。

## 设计文档

- `docs/specs/2026-06-16-design.md` — 架构与协议
- `docs/plans/2026-06-16-agentpipe-engine.md` — 实施计划
