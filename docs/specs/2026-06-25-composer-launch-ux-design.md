# 编排面板产品优化：模式简化 / 保存与启动分离 / 任务列表删除

状态：设计中 → 实施
日期：2026-06-25
范围：crates/engine（manifest + executor）+ src-tauri commands + ui（types / Composer / ProjectsPanel / App / ipc）

## 1. 背景

来自产品三点优化（针对 GUI「编排任务」面板与左侧「项目」列表）：

1. 模式不用切换，只保留 auto（自动跑）。step（逐步批准）选择器移除。
2. 底部「保存到 task.yaml」把「新建模版（保存）」和「启动任务（运行）」混在一张卡片里，要拆开。启动任务时直接在这里填入必要的条件即可，不必先存盘。
3. 左侧任务（运行记录）列表要支持删除。

## 2. 现状事实（实施基线）

- 模式：`crates/engine/src/manifest.rs` `RunMode { Step, #[default] Auto }`；executor `gated = matches!(mode, Step)`。两个模板都是 `mode: auto`。
- 「必要的条件」= human 步骤的输入。`StepKind::Human { instruction, expects }`，运行到该步发 `StepAwaitingGate{gate_kind:Human, expects_artifact:expects.is_some()}`，用户在控制台 gate 的「产物」框粘贴值，引擎 `record(step.id, artifact)`，后续步骤用 `{{mr.artifact}}` 插值。当前只能运行中逐个填。
- Composer 底部一张卡：保存路径 input + 「保存」（saveManifest 落盘）+「▷运行」（先落盘再 start_run 跑该文件）。run 强制要 savePath。
- 左侧 ProjectsPanel 渲染 `RunSummary[]`（历史 .ndjson），无删除入口。run 落盘在 `~/.agentpipe/runs/<run_id>.ndjson`，命令在 `commands.rs`。

## 3. 设计

### 3.1 模式简化（#1）

UI 不再暴露模式选择，一律 auto。`emptyManifest().mode = "auto"`；loadTemplate 结果强制 `mode: "auto"`；保存/启动构造 manifest 时强制 auto。warn banner 去掉 `mode==="auto"` 前置条件（恒 auto），仅按 `hasClaude(steps)` 显示。引擎 `RunMode::Step` 保留（向后兼容已有 step 模板 / CLI），仅 GUI 不再产出。

### 3.2 human 步骤预置值（#2 引擎支撑）

`StepKind::Human` 增可选字段 `value: Option<String>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`，向后兼容）。executor `run_human`：

- `value` 为 `Some` 且插值后非空 → 直接 `record(step.id, { artifact: 插值后的 value })` + `finish`，不发 gate、不阻塞 recv。
- 否则维持现状（发 Human gate 等用户）。

语义：value 是「启动时预置的人工输入」。模板本身不存 value（保持通用）；启动时把表单值注入 manifest 副本再 inline 跑。无 `expects` 的 human 步骤（如收尾确认 done）不预置 → 仍正常 gate。

TS 镜像：`types.ts` Human kind 增 `value?: string`。

### 3.3 底部拆分为「保存模版」+「启动任务」（#2 UI）

Composer 底部由一张卡拆成两张：

- 保存模版：保存路径 input +「保存」。落盘的是当前编辑的 manifest（注入 target、mode=auto），不含启动值。纯模版产出。
- 启动任务：
  - 列出每个「必要条件」= `steps` 中 `kind==="human" && expects` 的步骤，每条渲染一个带 label（instruction）的输入框，绑定 `launchValues[stepId]`。
  - 「▷ 运行」：构造 inline manifest = `{ ...m, mode:"auto", target, steps: human(expects) 步骤注入 value=launchValues[id] }`，调 `onLaunch(manifest)` → `runs.startInline`。不需要 savePath。
  - 无条件时该区只有运行按钮，直接 inline 跑。

Composer props 由 `{ target, onRun:(path) }` 改为 `{ target, onLaunch:(manifest) }`；保存仍走 `ipc.saveManifest` 内部直调。App 把 onLaunch 接到 `runs.startInline` + 选中 live。

### 3.4 删除运行记录（#3）

- `commands.rs` 增 `delete_run(run_id)`：`run_path_checked`（含 is_valid_run_id 防穿越）→ `remove_file`，NotFound 当成功（幂等）。`main.rs` 注册。
- `ipc.ts` 增 `deleteRun(runId)`。
- ProjectsPanel record-item 增删除按钮（✕，stopPropagation），点了走 `window.confirm` 二次确认再回调 `onDeleteRun(runId)`。
- App `onDeleteRun`：await deleteRun → `hist.refresh()`；若删的是当前 selection 则清空；从 compareIds 移除。

## 4. 候选与取舍（#2）

- 方案 A（采用）：引擎加 `value` 预置位，启动时注入 inline manifest。值随 manifest 自包含、可审计，gate 逻辑不变，收尾确认仍 gate。引擎改动 ~15 行 + TS 镜像。
- 方案 B（弃）：纯 UI 在控制台 Human gate 到达时用预存值自动 ApproveGate。零引擎改动，但值散在 UI、需把预填表跨 App→Console 串联并模拟人，分层不干净，且依赖 UI 时序匹配 gate。

选 A：与本仓「manifest 为单一来源、运行自包含」的取向一致，总改动量反而更小。

## 5. Phase

- P1 引擎：manifest `value` 字段 + executor 预置分支 + 测试（`cargo test -p agentpipe-engine`）。
- P2 UI #1：types 镜像 + 移除模式选择器 + 恒 auto。
- P3 UI #2：底部拆「保存模版 / 启动任务」+ inline 启动 + props 调整 + App 接线。
- P4 #3：delete_run 命令 + 注册 + ipc + ProjectsPanel 删除按钮 + App 接线。
- 收口：`npm run build`(tsc) + `cargo test` + vitest。
