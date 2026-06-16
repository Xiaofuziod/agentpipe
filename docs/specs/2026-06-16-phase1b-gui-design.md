# AgentPipe Phase 1b 设计:Tauri 壳 + UI + 运行时控制

工作名 AgentPipe。日期 2026-06-16。状态:待评审。前置:Phase 1a(引擎 + CLI,已落地并真机验证)。

## 1. 目标与范围

把 Phase 1a 的 headless 引擎装进一个轻量 Tauri 桌面应用,提供两块界面:

- 编排页(Composer):可视化增删步骤、产出 task.yaml。
- 运行控制台(Run Console):看引擎执行、在 gate 处批准/跳过、终止运行。

并把 Phase 1a 留作占位的运行时控制协议真正接通:gate 批准/跳过走 UI、Abort 终止运行(杀掉在跑的子进程)、step 失败/中断走统一的决策 gate(重试/跳过/中止)。

### 非目标(沿用 Phase 1a YAGNI)

- 不做多 Run 并发,同一时刻只有一个活跃 Run。
- 不做分发/签名/自动更新,dev 模式跑。
- StepProgress 实时流式列为 stretch(末位任务),核心控制台先用步骤树 + 阶段摘要。
- 不做条件分支/并行步骤。

## 2. 技术栈与目录

- 壳:Tauri 2(Rust 后端 + 系统 webview)。
- 前端:React 18 + Vite + TypeScript,`@tauri-apps/api` v2。
- 引擎:复用 `crates/engine`,基本不动,仅加 serde 派生与一个 Control 句柄。

```
agentpipe/
├─ crates/engine/         # 复用,小改:protocol serde + manifest Serialize + Control
├─ crates/cli/            # 保留(headless 驱动 / 测试用)
├─ src-tauri/             # 新增:Tauri 后端薄壳(命令 + 事件转发 + AppState)
│  ├─ Cargo.toml
│  ├─ tauri.conf.json
│  └─ src/
│     ├─ main.rs          # tauri::Builder,注册 commands + manage(AppState)
│     ├─ state.rs         # AppState:活跃 Run 的 command Sender + Control
│     └─ commands.rs      # start_run / send_command / save_manifest / load_template / list_templates
└─ ui/                    # 新增:React 前端
   ├─ package.json, vite.config.ts, index.html
   └─ src/
      ├─ main.tsx, App.tsx          # 路由:'composer' | 'console'
      ├─ ipc.ts                     # invoke 封装 + listen('engine://event')
      ├─ types.ts                   # 镜像 engine 协议(手写,与 Rust 对齐)
      ├─ composer/                  # StepList / StepCard / TargetPicker / ModeToggle
      ├─ console/                   # RunTree / StepRow / GatePrompt / RunControls
      └─ state/runReducer.ts        # 引擎事件 → 控制台状态
```

## 3. 引擎 ↔ Tauri 集成(核心)

Phase 1a 引擎已是 channel 化:`Executor::new(manifest, bins, events: Sender<Event>, commands: Receiver<Command>)` + 阻塞式 `run()`,在 gate 处 `commands.recv()`。Tauri 层只需做桥接,引擎逻辑不重写。

```
webview ──invoke("start_run", path)──▶ commands.rs
                                          │ 解析+校验 manifest
                                          │ 建 (event_tx,event_rx) (cmd_tx,cmd_rx)
                                          │ 存 cmd_tx + Control 到 AppState
                                          │ thread::spawn(engine.run())   ← 引擎线程
                                          │ thread::spawn(forward event_rx) ← 转发线程
                                          ▼
engine 线程 ──Event──▶ event_rx ──forward──▶ app.emit("engine://event", evt)
                                                              │
webview  listen("engine://event") ◀───────────────────────────┘  渲染步骤树

webview ──invoke("send_command", cmd)──▶ commands.rs ──▶ cmd_tx.send(cmd)  到引擎 gate
```

要点:

- 引擎在后台线程跑,绝不阻塞 Tauri 主线程 / webview。
- 事件转发线程把 `Event` 经 `app.emit` 推给 webview;`event_rx` 收到末帧 `RunFinished` 后转发线程退出。
- `send_command` 把 webview 的指令塞进 `cmd_tx`,引擎在 gate 处 `recv` 到。
- AppState 用 `Mutex<Option<ActiveRun>>` 持有当前 Run 的 `cmd_tx` 与 `Control`;已有活跃 Run 时再 `start_run` 直接拒绝(单 Run 不变式)。

## 4. 协议 serde 化

`crates/engine/src/protocol.rs` 的 `Event` / `Command` / `StepStatus` / `RunStatus`,以及 `context::Verdict`,加 `#[derive(Serialize, Deserialize)]`,并用 `#[serde(tag = "type")]` 内部标签,webview 收到形如:

```json
{ "type": "StepStarted", "step_id": "review", "kind": "codex" }
{ "type": "StepAwaitingGate", "step_id": "fix", "suggestion": "...", "expects_artifact": false }
```

指令同理:`{ "type": "ApproveGate", "step_id": "fix", "artifact": null }`。

`manifest.rs` 的 `Manifest` / `Step` / `StepKind` / `CodexAction` / `RunMode` 追加 `Serialize`(Composer 保存路径:webview 送 JSON → Rust 反序列化校验 → serde_yml 写 YAML,复用 Phase 1a 的 validate)。

新增字段不破坏 Phase 1a:CLI 与测试不依赖序列化形态;加派生是纯增量。

## 5. 运行时控制:Abort / Interrupt / 失败决策

Phase 1a 的失败处理是"emit RunFinished{Failed} 直接结束";Interrupt/Resume/Abort 是占位。Phase 1b 统一成一个**决策 gate**模型,并让 Abort 能真正杀掉在跑的子进程。

### 5.1 Control 句柄(可中断的关键)

引擎当前在 `run_command` 里阻塞,gate 之外不轮询指令,所以"运行中的 step"无法被 channel 指令打断。引入一个 Arc 共享的 Control:

```rust
pub struct Control {
    pub abort: AtomicBool,                 // 置位 = 请求中止
    pub current_pid: Mutex<Option<u32>>,   // 当前在跑子进程的 pgid(进程组)
}
```

- `run_command` spawn 子进程时用独立进程组(Unix `Command::process_group(0)`),把 pgid 写入 `current_pid`,退出时清空。
- Tauri 的 Abort 指令处理器:读 `current_pid`,`killpg(pgid, SIGKILL)`(经 `libc`,连带杀掉 codex/claude 在 shell 下起的孙辈,解决 Phase 1a spike 暴露的"杀 bash 杀不掉 sleep"问题),并置 `abort=true`。
- 引擎每个 step 边界检查 `abort`,置位则停止剩余 step,emit `RunFinished{Aborted}`。

Control 由 `start_run` 创建,一份给引擎、一份(Arc clone)存 AppState 供指令处理器用。

### 5.2 决策 gate 统一失败/中断

step 失败(CLI 非零/超时/被 Abort 杀)时,不再直接结束 Run,而是 emit 一个决策 gate,webview 弹 重试 / 跳过 / 中止:

```
StepFailed{step_id, error}  ──▶  StepAwaitingGate{step_id, suggestion:"step 失败,如何处理", kind:"decision"}
webview ──▶ ApproveGate(=重试该 step) | SkipStep | Abort
```

复用既有 `StepAwaitingGate` + `commands.recv()` 通道,只在失败分支加一次 gate 等待。Abort 指令在此处或运行中均可生效(5.1)。Resume 语义即"在决策 gate 选重试",不单列指令。

### 5.3 协议增量

`Command` 已有 `Interrupt/Resume/Abort`,Phase 1b 真正实现 `Abort`(杀子进程 + 停 Run);`Interrupt` 等价于"Abort 当前 step → 进决策 gate"(杀子进程但不停 Run);`Resume` 在决策 gate 内用 `ApproveGate` 表达,标记 `#[deprecated]` 但保留枚举不破坏序列化。

## 6. Composer(编排页)

表单式,不做复杂拖拽(v1 用上移/下移按钮):

- 顶部:target 目录选择(Tauri dialog 插件)、name 输入、mode 切换(step/auto)。
- 步骤列表:每个 step 一张卡片 = kind 下拉(claude/codex/human/loop)+ 该 kind 的字段表单;增/删/上移/下移。
  - claude:prompt(多行)、skill(可选)、allow_writes(开关)、timeout(可选)。
  - codex:action 下拉(review-doc/review-mr/ask)+ 条件字段(path / base / prompt)。
  - human:instruction、expects(开关:是否需产物)。
  - loop:until(固定 codex-clean)、max、body(嵌套步骤列表,复用 StepCard)。
- "从模板新建":`list_templates` + `load_template` 把模板 YAML 读成 JSON 灌进编辑器。
- 保存:webview 把当前编辑对象(JSON)`invoke("save_manifest", {manifest, path})` → Rust 反序列化为 `Manifest` → `validate()`(复用 Phase 1a 校验,错误回显字段级 message)→ `serde_yml` 写 YAML。
- 保存成功 → 可一键"运行"(切控制台 + `start_run`)。

## 7. Run Console(运行控制台)

事件驱动:

- `listen("engine://event")` → `runReducer` 把事件归约成步骤树状态(每 step 的 status / summary、loop 轮次、当前 gate)。
- 步骤树:status 用 Phase 1a 的 `StepStatus`(Phase 1b 让引擎补发 Running/AwaitingGate 中间态,使树能显示"运行中/待批准")。
- Gate 区:收到 `StepAwaitingGate` 时高亮该 step + 显示 suggestion + 按钮(批准 / 跳过;human 步骤额外一个 artifact 输入框;decision gate 显示 重试/跳过/中止)→ `send_command`。
- 运行控制条:Abort 按钮(任意时刻);Run 结束显示终态(Success/Failed/Aborted)。
- StepProgress(stretch):若实现流式,则在选中 step 下展示 CLI stdout 增量。

引擎补发中间态:Phase 1a 只发 StepStarted/Finished;Phase 1b 在进入 step 时该 step 视为 Running、进 gate 时 AwaitingGate,失败时 Failed。这是对 `StepStatus` 既有枚举的接通(Phase 1a code-review 指出其大半 variant 未用,此处正好补齐),不新增协议形状。

## 8. 错误与边界

- 单 Run 不变式:活跃 Run 存在时 `start_run` 返回错误,UI 提示先结束当前 Run。
- manifest 校验失败:`save_manifest` / `start_run` 返回字段级错误,UI 红字回显,不进执行。
- Abort 杀进程经进程组,避免遗留孙辈(Phase 1a spike 教训)。
- 转发线程与引擎线程在 Run 结束/Abort 后干净退出(event_tx drop → forward 循环结束)。
- allow_writes step 在 auto 模式 = 完全放权(claude bypassPermissions);UI 在运行前对含 allow_writes 的 auto Run 给一次显式提示(沿用 fail-closed 谨慎基调)。
- webview/Rust 协议形状靠"手写 TS 类型镜像 + 一处契约测试"对齐(见计划),不引入 codegen。

## 9. 里程碑(详见实施计划)

1. 协议 serde 化 + manifest Serialize(引擎小改,不破坏 Phase 1a 测试)。
2. src-tauri 骨架 + AppState + start_run/send_command + 事件转发(headless 可测)。
3. Composer UI。
4. Run Console UI(步骤树 + gate)。
5. 运行时控制:Control + Abort(进程组杀)+ 失败决策 gate + 中间态补发。
6. (stretch)StepProgress 流式。

## 10. 未决问题

- Tauri dialog 选目录用 `tauri-plugin-dialog`,需确认 v2 插件接入方式(计划 Task 含)。
- 进程组杀用 `libc::killpg`;Windows 不在当前目标(macOS 优先),跨平台 kill 留待需要时(Windows 用 `taskkill /T`)。
- 中间态补发是否影响 Phase 1a 的 executor 测试断言(现有测试只数 StepStarted/RunFinished,补 Running 不破坏计数,但需复核)。
