# GUI 补全:审计接通 + 校验门编排(设计)

日期 2026-06-18。目标:把 Tauri GUI 补到与 CLI 能力对齐——持久化审计接进 GUI(运行落盘 + 历史/成本/回看/对比),并让 Composer 能编排 verify 校验门(含新增 `by: command`)。带测试、高质量。

现状基线(已核对源码):
- 后端 `src-tauri/`:`bridge`(引擎跑后台线程 + 事件转发到 webview)、`commands`(start_run / start_run_inline / send_command / save_manifest / list_templates / load_template)、`state`(单活跃 Run 不变式)。**bridge 不写 NDJSON**;**无任何审计读命令**。
- 前端 `ui/`:三栏(RecordsPanel 记录 | Console 控制台+GatePrompt | Composer 编排),`useRuns`/`runReducer`(事件→状态,带测试 14 个),`styles.css` 919 行。**记录仅内存**(重启即丢、无成本列、无历史、无 diff)。**Composer 的 claude 编辑器无 verify 块编辑 UI**;`types.ts` Verify 缺 `by:"command"`。
- 基线绿:`ui` build+test 过、`agentpipe-tauri` cargo 编译过。

## 1. 问题与范围

GUI 与 CLI 的能力差:CLI 有 `run`(落 NDJSON)+ `runs/view/cost/diff`(读审计)+ verify 门(codex/claude/command)。GUI 缺这两块。本 spec 补齐,分两个独立部分:

- **A. 审计接通**:GUI 发起的 run 也落 `~/.agentpipe/runs/<run-id>.ndjson`;新增 Tauri 读命令;前端历史浏览(列表/成本/只读回看/两次对比)。
- **B. 校验门编排**:Composer 的 claude 步骤可视化编排 verify 门(by: codex|claude|command),`types.ts` 与引擎 schema 对齐。

两部分互相独立,可各自实现、各自测试。

非目标(本 spec 不做):真实 `cargo tauri dev` 自动化端到端(需图形环境,改由 build+单测+组件测试 + 手动启动文档覆盖);GUI 主题/视觉重构;replay=重执行(只读回看即可);跨机/云端审计。

## 2. 部分 A:审计接通

### A1. GUI 运行落盘(bridge 接 RunRecorder)

`bridge::start` 的转发线程在收到事件时,镜像 CLI `cmd_run` 的做法:`RunStarted` 时 `RunRecorder::open(&runs_dir(), name)`,之后每个事件 `record(&evt)`。失败降级为不落盘(审计旁路,`eprintln` 告警,绝不打断 run)。

- 新增 `src-tauri` 的 `runs_dir()`(镜像 CLI:`$AGENTPIPE_HOME` 否则 `$HOME`,接 `.agentpipe/runs`)。放 `src-tauri/src/paths.rs`,供 bridge 与 commands 共用。
- run-id 由 `RunRecorder` 生成(已有 allowlist + 同秒撞名退避,见上一个 spec)。bridge 把 run-id 经一个新 Tauri 事件 `engine://run-started-id`(payload `{ run_id }`)发给 webview,让内存记录与落盘文件关联(用于结束后在历史里定位同一条)。失败不发(webview 自行靠刷新历史兜底)。

> 引擎仍纯净:落盘是 bridge(消费者侧)的事,executor 不动。与 CLI 同构。

### A2. 审计读命令(Tauri)

`src-tauri/src/commands.rs` 新增,全部复用 `agentpipe_engine::audit`:

```rust
#[tauri::command] fn list_runs() -> Result<Vec<RunSummary>, String>
#[tauri::command] fn view_run(run_id: String) -> Result<Vec<EngineEventDto>, String>
#[tauri::command] fn diff_runs(a: String, b: String) -> Result<Vec<DiffRow>, String>
```

- `RunSummary { run_id, name, started_ts, status: Option<String>, total_cost_usd, total_turns, step_count, complete: bool }`:由一次 `read_run` + 聚合得出(name 取首个 RunStarted.name;status 取末 RunFinished.status;complete=末行是否 RunFinished;cost 用 `aggregate_cost`)。`list_runs` 扫 `runs_dir` 下 `*.ndjson`,逐个 `read_run` 聚合,按 run-id(时间戳前缀)倒序。
- `view_run(run_id)`:`is_valid_run_id` 校验(防穿越)→ `read_run` → 返回事件序列(给前端 `runReducer` 重建 RunState,复用同一 reducer,零重复)。
- `diff_runs(a,b)`:两边 `is_valid_run_id` → `read_run` → 按 step 终态(status+cost)对比,返回 `DiffRow { step_id, kind: "only_a"|"only_b"|"changed", a?: StepFinal, b?: StepFinal }`。对比逻辑下沉到 engine `audit::step_finals`(目前在 CLI `commands.rs`,提到 engine 复用,与 `aggregate_cost` 并列——消除 CLI/GUI 两份)。
- 引擎侧改动:`RunEntry`/`CostSummary` 加 `Serialize`(tauri 返回需要);新增 `audit::step_finals(entries) -> BTreeMap<String,(String,f64)>` + `audit::run_summary(entries) -> RunSummaryCore`(name/status/cost/complete/step_count 的纯计算,tauri 与未来 CLI 共用)。
- 全部命令注册进 `main.rs` 的 `invoke_handler`。
- 安全:`view_run`/`diff_runs` 的 run-id 必过 `is_valid_run_id`,非法即 `Err`(与 CLI 一致,防路径穿越)。

### A3. 前端历史浏览

- `ipc.ts` 加 `listRuns / viewRun / diffRuns / onRunStartedId`。
- `types.ts` 加 `RunSummary / DiffRow / StepFinal` 镜像类型 + `engine://run-started-id` 监听。
- 新 hook `state/useHistory.ts`:`list()`(拉 list_runs)、`open(run_id)`(view_run → 折叠 `runReducer` 重建 RunState 用于只读回看)、`refresh`(挂载时 + 每次 RunFinished 后)。
- `RecordsPanel` 升级为历史浏览:展示持久化 run 列表(名称/时间/状态点/步数/`$成本`);活跃 live run 置顶(来自 `useRuns`,带"运行中"点)。选中持久化条目 → Console 只读回看(喂 view_run 重建的 state,`isLive=false`);选中活跃条目 → 现有 live 路径。空态文案更新。
- diff:RecordsPanel 支持选两条(多选/对比按钮)→ 弹 `records/DiffView.tsx` 展示 `DiffRow` 列表(仅A/仅B/变化,含成本差)。
- 成本展示:记录行显示 `total_cost_usd`(>0 时);Console 头部对回看的 run 也显示总成本。
- 数据流:`App` 组合 `useRuns`(live)+ `useHistory`(persisted);RunFinished 后 `useHistory.refresh()` 让刚结束的 run 进历史。live 内存记录与持久化记录用 run-id 去重(`engine://run-started-id` 关联;拿不到 id 时以"活跃中只显示 live、结束后只显示持久化"兜底,不双显)。

## 3. 部分 B:校验门编排

### B1. types.ts 与引擎对齐

`Verify` 加 `by: "command"` 与 `command?: string`:

```ts
export type Verify = {
  by: "codex" | "claude" | "command";
  action?: CodexAction;  // codex
  base?: string; path?: string; prompt?: string;
  skill?: string;        // claude
  command?: string;      // command(新)
  max_retries?: number;
  on_unmet?: "gate" | "fail" | "continue";
  feedback?: boolean;
};
```

### B2. StepDrawer 加 verify 编辑(claude 步骤)

claude 的 `Fields` 增加一个可折叠 "校验门(verify)" 区:

- 复选「启用校验门」→ 无则 `verify` 不写(保持可选)。
- `by` 选择:codex / claude / command。
- 按 `by` 条件渲染:codex→action(+ base/path/prompt 随 action)、claude→prompt(+ skill 可选)、command→command(shell,占位 `cargo test`)。
- 通用:max_retries(number,默认 2)、on_unmet(gate/fail/continue,默认 gate)、feedback(checkbox,默认 true)。
- 写回走 `onChange({...step, verify})`;StepCard 的 `stepSummary` 给 claude 追加 verify 摘要(如 `+verify:command`)。
- 客户端轻校验:command 选了但空 → 行内提示(与引擎 validate 同义,提前反馈);真正 fail-closed 仍以引擎 `validate()` 为准(保存/运行时)。

> 引擎 schema 不变(已支持三种 verifier);本部分纯前端补 UI + 类型。

## 4. 测试策略(高质量门)

- **引擎(Rust)**:`step_finals` / `run_summary` 纯函数单测(含 only_a/only_b/changed、未完成 run、空文件);`RunEntry`/`CostSummary` 序列化 round-trip。
- **Tauri(Rust)**:`list_runs`/`view_run`/`diff_runs` 针对临时 `AGENTPIPE_HOME` 的集成测(写两个 ndjson → 断言摘要/事件/diff);`view_run` 拒绝非法 run-id(穿越);bridge 落盘——用 stub 引擎跑一次断言 ndjson 生成(可走现有 smoke 模式)。tauri command 逻辑尽量抽成可单测的纯函数(`#[tauri::command]` 只做薄包装)。
- **前端(vitest)**:`useHistory` 的 reducer 折叠(view_run 事件序列 → 期望 RunState);diff 渲染分桶;verify 编辑的 onChange 写回(启用/切 by/清空)正确产出 Verify 对象;`types` 镜像不漂移(已有 runReducer/useRuns 测试保留)。
- **构建门**:`ui` `npm run build`(tsc 严格)+ `npm test`;workspace `cargo build && cargo test`(含新 tauri/engine 测试)。全绿才算完工。
- **手动启动**:文档补 `cargo tauri dev` 启动说明 + 一条 stub 烟测路径(图形环境下人工验收:跑一次 → 历史出现该 run → 点开回看 → 成本正确 → 选两条 diff → 编排一个 by:command verify 保存)。

## 5. 落地顺序(供 plan)

部分 A 与 B 独立,可并行;建议:
1. 引擎 audit:`step_finals` + `run_summary` 下沉/新增 + `Serialize` 派生 + 单测;CLI `commands.rs` 的 `step_finals` 改调引擎版(去重)。
2. tauri `paths.rs`(runs_dir)+ bridge 接 RunRecorder + `engine://run-started-id`。
3. tauri 审计读命令(list_runs/view_run/diff_runs)+ 注册 + 集成测。
4. 前端类型 + ipc + `useHistory` + 测试。
5. 前端 RecordsPanel 历史化 + Console 只读回看接线 + DiffView。
6. 部分 B:types.ts Verify + StepDrawer verify 编辑 + stepSummary + 测试。
7. 文档(启动说明)+ 全量验证。

## 6. 关键不变式(防回归)

- 引擎纯净:落盘/审计永远在 bridge/commands(消费者侧),executor 不依赖文件系统。
- fail-closed 延续:审计写失败不打断 run;view/diff 的 run-id 必过 allowlist;非法即 Err。
- 单一来源:对比/聚合逻辑在 engine `audit`,CLI 与 GUI 都调它(不得各写一份);事件→状态用同一个前端 `runReducer`(live 与回看共用)。
- 类型镜像:`types.ts` 的 Verify/Event/StepMetrics 与 `manifest.rs`/`protocol.rs` 手工镜像,改一边须同步注释指明的另一边。
