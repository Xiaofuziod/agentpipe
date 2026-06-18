# 左列项目化(Codex 式 Project = Target)设计

## 背景与目标

当前 GUI 三列:左 `RecordsPanel`(所有 run 扁平列表,最新在前)、中控制台、右编排。
所有 run 共用一个全局 `target`(工作目录),快跑栏每次要手选目录,历史 run 之间没有"归属"关系。

参考 Codex 的 project 概念,本次把左列改成 **项目化**:

- **一个项目 = 一个 target**(run 的工作目录 / 仓库绝对路径)。
- 同一 target 下产生的多轮 run("多轮回合")归类到该项目下。
- 左列展示为可折叠的项目分组树:项目头(目录名 + run 数 + 累计成本) → 其下嵌套该项目的 run 列表。
- 底部快跑栏的 target 选择器 **默认选最近用过的项目**,并可在已知项目间切换 / 另选新目录。

三列布局保持不变,只重构左列与快跑栏 target 选择。

## 关键约束:target 当前未落盘

run 审计 ndjson 只记录 `Event::RunStarted { name }`,**不含 target**。要按 target 归类历史 run,
必须先把 target 持久化。落点选在 `RunStarted` 事件本身(executor 发事件时 `manifest.target` 可得,
且 live / 历史回放共用同一事件流,单一来源)。

### 向后兼容(schema 演进)

- `Event::RunStarted` 新增 `target` 字段,Rust 侧 `#[serde(default)]`:旧 ndjson 无此字段 → 反序列化默认空串,
  容损读取不破。属"只加 optional 字段"的向后兼容演进。
- 空 target(旧记录 / 解析失败)在前端归入"(未指定项目)"分组,不丢记录。

## 数据流

```
executor.run() 发 RunStarted { name, target }
  ├─ live:  runReducer 把 e.target 存进 RunState.target → liveRun.target
  └─ 落盘:  audit run_summary 从 RunStarted 提取 target → RunSummary.target
                ↓
          前端 groupByProject(summaries, liveRun) → Project[]
                ↓
          左列 ProjectsPanel(可折叠分组) + 快跑栏 target 下拉(默认 groups[0])
```

## 改动清单

### 后端(Rust)

1. `protocol.rs`:`RunStarted { name, #[serde(default)] target: String }`。
2. `executor.rs`:发 `RunStarted` 带 `self.manifest.target`(显示串)。
3. `audit.rs`:`RunSummaryCore` 加 `target: String`;`run_summary` 从首个 `RunStarted` 提取;更新构造点 / 测试。
4. `commands.rs`:`RunSummary` 加 `target` 透传。
5. `bridge.rs` / `render.rs` / `cli/main.rs`:`RunStarted` 模式匹配补 `target`(或 `..`)。

### 前端(TS)

6. `types.ts`:`RunStarted` 加 `target`;`RunSummary` 加 `target`。
7. `runReducer.ts`:`RunState` 加 `target`,`RunStarted` case 写入 `e.target`。
8. 新 `state/projects.ts`:`groupByProject` 纯函数 + 单测(分组、排序、空 target 兜底、live 合并)。
9. 新 `records/ProjectsPanel.tsx`:替换 `RecordsPanel` 渲染,可折叠分组;run 行复用既有 `record-item` 样式 + 对比 checkbox;项目头点击设为活跃 target 并切换展开;活跃项目高亮。
10. `console/Console.tsx` 快跑栏:target 选择器从"仅选目录"改为 **项目下拉**(列已知项目 + "另选目录…"),默认最近项目。
11. `App.tsx`:首次 summaries 刷新后若 `target` 为空,默认设为最近项目 target;项目头选择联动 `target`。
12. `styles.css`:项目分组头 / 嵌套缩进 / 活跃态样式(走既有 token)。

## 候选方案与取舍

- **A(选用)target 进 RunStarted 事件**:单一来源,live 与回放共用;向后兼容靠 serde default。改动集中、回放零特判。
- **B 单独 projects.json 索引文件**:需维护 run↔project 映射、并发写、与 ndjson 同步,复杂度高;Phase 1 个人本地工具不值当。
- **C 用 run_id 前缀或目录名推断**:run_id 不含 target,推断不出,排除。

选 A。

## 边界与失败路径

- 空 target / 旧记录:归"(未指定项目)",不隐藏。
- 无任何历史 + 无 live:左列空态文案保留。
- 活跃 target 为空(全新启动无历史):快跑栏回退到"选择目录",与现状一致。
- 项目下拉选目录后未跑 run:仅更新活跃 target,不立即建空项目(项目随首个 run 落盘出现)。
