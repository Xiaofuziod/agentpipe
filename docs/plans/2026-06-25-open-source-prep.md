# 开源发布准备 — 执行计划

日期 2026-06-25。目标:把 AgentPipe 从「个人仓库」收拾成「可公开的开源项目」,按价值评估给出的优先级落地。差异化叙事先行,诚实标注成熟度/平台。

## 交付物 / Phase

- P1 LICENSE(MIT)+ crate license 元数据
- P2 README.md(英文,主叙事 = cross-vendor adversarial review pipeline with deterministic command gates)+ 现有中文 README 移到 README.zh-CN.md 并互链
- P3 CONTRIBUTING.md(dev setup / verify 关卡 / 「个人工具、macOS 优先、欢迎 PR」框架 / Adapting to other agents 文档化注入点)
- P4 Demo:`demo/` 下 stub(claude/codex 计数收敛)+ demo-task.yaml(human 步用 value 预置 → 全程 headless)+ VHS `agentpipe.tape` → 渲染 `demo/agentpipe.gif`,README 顶部嵌入
- P5 CI:`.github/workflows/ci.yml` — ubuntu+macos 跑 `cargo test`(engine+cli)+ UI build/test;windows 跑 `cargo build`(engine+cli)。验证「最小跨平台路径」并入门禁
- P6 Secret 扫描:gitleaks 全 history detect,报告结果
- P7 发布清单:awesome-list 提交草稿(awesome-cli-coding-agents / awesome-agent-orchestrators,待仓库转公开后手动提)+ pre-public checklist,写入本文件附录

## 关键事实(实施基线,已核)

- `libc` 已 `[target.'cfg(unix)'.dependencies]` 门控 → Windows 可编译;`control.rs`/`runner/mod.rs` 有 `cfg(not(unix))` 回退(killpg → child.kill)。引擎本就基本跨平台,缺的是 Windows 验证 + 诚实文档。
- 二进制已可经 `AGENTPIPE_CLAUDE_BIN` / `AGENTPIPE_CODEX_BIN` 覆盖(stub 测试在用)。「去硬编」取「文档化注入点」档位:env 覆盖 + runner 模块结构 + 加新 agent 的步骤,写进 CONTRIBUTING;不做 StepKind 大重构(YAGNI)。
- human 步骤新增的 `value` 预置位让 demo 可 headless 跑(无 gate 阻塞)。
- 无已跟踪的 secret 文件;`.gitignore` 覆盖 target/node_modules/dist/gen,但未显式列 `.env`(补)。

## Non-goal(本轮不做)

- 不做多 agent 抽象层 / 插件系统大重构。
- 不真去外部 awesome repo 提 PR(对外、需仓库先公开,留作手动步骤)。
- 不改产品功能;纯发布工程 + 文档。

## 附录:Pre-public checklist(手动)

1. `gitleaks detect`(P6 已自动跑一遍)— 转公开前再确认无遗漏。
2. 确认仓库 Settings → 改 public;补 repo description + topics(`claude-code` `codex` `ai-agents` `code-review` `rust` `tauri`)。
3. awesome 提交(仓库公开后):
   - bradAGI/awesome-cli-coding-agents:在 orchestrators/harness 段加一行。
   - andyrewlee/awesome-agent-orchestrators:加一行。
   - 文案:`AgentPipe — Cross-vendor adversarial review pipeline: Claude writes, Codex reviews, loop until clean, with deterministic command gates. Rust engine + Tauri desktop GUI. (macOS-first)`
4. LICENSE 版权人按需改成真实姓名/组织。
