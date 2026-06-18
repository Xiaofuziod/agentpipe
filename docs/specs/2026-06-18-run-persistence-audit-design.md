# 运行持久化与审计:NDJSON 落盘 + 子命令化 CLI(设计)

日期 2026-06-18。借鉴 ccswarm(NDJSON 审计 / replay / diff / cost / run-id allowlist)+ Microsoft Conductor(pub/sub 事件总线 / validate / dry-run / stdout-stderr 分离)。配套调研见 `docs/research/2026-06-18-ccswarm-conductor-comparison.md` §3。

## 1. 问题

当前可观测性只活在内存与终端:

- `executor.rs` 通过 `events: Sender<Event>` 发事件,`cli/main.rs` 唯一消费者 `for event in erx` 打印到 stdout,**run 结束即蒸发**。没有持久记录。
- 出问题后无法事后排查("上次那个 run 第几步开始跑偏的?")、无法对比两次 run、无法看历史成本。
- `StepMetrics.cost_usd` 已采集,但**仅单步打印,无聚合**。
- CLI 只有 `run` 一条路径(手搓 argv `(Some("run"), Some(p))`),`validate` 埋在 run 内部(`m.validate()`),无独立校验、无 dry-run、无机器可读输出模式。

目标:给每次 run 落一份可重读、可对比的结构化审计;把 CLI 提升成子命令结构,补 `view / diff / cost / runs / validate` 与 `--dry-run / --json`。**引擎保持纯净**(只发事件),持久化是消费者侧的事,符合 Conductor pub/sub:同一事件流,终端渲染、NDJSON 落盘、未来 Tauri GUI 各自独立订阅。

## 2. 现有可复用资产

- `protocol.rs::Event` 已 `#[derive(Serialize, Deserialize)]`(L30)—— NDJSON 序列化零成本。
- `Event` 已覆盖全生命周期:`RunStarted/StepStarted/StepProgress/StepAwaitingGate/StepFinished/StepFailed/LoopIteration/LoopConverged/LoopMaxReached/RunFinished`。
- `StepMetrics { num_turns, duration_ms, cost_usd }` 随 `StepFinished` 带出 —— cost/耗时聚合的数据源已在流里。
- `serde_json` 已是 engine 依赖。

## 3. 架构:消费者侧 RunRecorder

引擎不改:executor 仍只 `events.send(...)`。在 engine crate 新增 `audit` 模块,提供一个**事件流包装器**,任何消费者把事件先喂给它再自己处理:

```
executor ──Sender<Event>──▶ erx ──┬──▶ RunRecorder.record(&event)   // 落 NDJSON
                                   └──▶ 终端渲染 / Tauri 推送        // 现状逻辑不变
```

`audit::RunRecorder`:

```rust
pub struct RunRecorder { file: BufWriter<File>, run_id: String }

impl RunRecorder {
    /// 在收到 RunStarted 时创建:派生 run_id,建文件。
    pub fn open(run_dir: &Path, name: &str, started_at: DateTime<Utc>) -> Result<Self>;
    /// 每个事件追加一行 NDJSON;内部错误不 panic、不影响主流程(审计失败不能拖垮 run)。
    pub fn record(&mut self, event: &Event);
    pub fn run_id(&self) -> &str;
}
```

CLI 的 `for event in erx` 循环里,`RunStarted` 时 `RunRecorder::open(...)`,之后每个事件 `recorder.record(&event)` —— 在现有 match 之前 tee 一行,**不改任何现有渲染分支**。

> 设计信条:审计是旁路。`record` 内部 I/O 错误只 `eprintln!` 告警,绝不冒泡打断 run(对齐全局基线"日志/审计失败 fail-safe,不破坏主路径")。

## 4. run-id 与存储

- 存储根:`~/.agentpipe/runs/`(`$AGENTPIPE_HOME` 可覆盖,便于测试与隔离;默认走 home)。
- 文件名:`<run-id>.ndjson`。
- run-id 格式:`<UTC RFC3339 紧凑>-<slug(name)>`,例 `20260618T142233Z-add-verify-gate`。
  - slug:name 转小写、非 `[a-z0-9]` 折成 `-`、压缩连续 `-`、截断 ≤ 40 字符。
  - **最终 run-id 强制匹配 `^[A-Za-z0-9_-]+$`**(ccswarm 同款 allowlist);任何外部传入 run-id 的子命令(view/diff/cost)先过此校验,**防路径穿越**(`../`)。不匹配直接拒绝。

## 5. NDJSON 行 schema

每行一个对象,稳定可解析:

```json
{"ts":"2026-06-18T14:22:34.512Z","event":{"type":"StepFinished","step_id":"implement","status":"Done","summary":"…","metrics":{"num_turns":7,"duration_ms":41200,"cost_usd":0.83}}}
```

- `ts`:事件落盘时刻(RFC3339,毫秒)。
- `event`:`Event` 的 serde 序列化(沿用其现有 tag 形式)。
- 首行约定为 `RunStarted`,末行为 `RunFinished` —— 读取侧据此判断 run 是否完整(无末行 = 中断/崩溃,view 标记"未完成")。

## 6. CLI 子命令化(clap)

从手搓 argv 迁到 `clap`(derive)。子命令:

| 命令 | 作用 |
|---|---|
| `agentpipe run <task.yaml> [--dry-run] [--json]` | 执行(现状)。`--dry-run`:解析+validate+打印执行计划(步骤树/loop/verify 门),不起任何 CLI 子进程。`--json`:事件以 NDJSON 写 stdout、人读日志写 stderr。 |
| `agentpipe validate <task.yaml>` | 仅解析 + `m.validate()`,打印 OK 或带 message 的错误。退出码 0/非 0。 |
| `agentpipe runs` | 列 `~/.agentpipe/runs/` 下所有 run(run-id、name、时间、状态、总成本),按时间倒序。 |
| `agentpipe view <run-id>` | 重读该 run 的 NDJSON,按现有终端渲染格式重放打印(只读,不重执行)。未完成的 run 标注。 |
| `agentpipe diff <run-a> <run-b>` | 对比两次 run 的事件时间线:步骤集合差异、各步 verdict/状态差异、成本/耗时差异。 |
| `agentpipe cost <run-id>` | 该 run 的成本拆解:per-step `cost_usd`/`num_turns`/`duration_ms` + 总计。 |

- **stdout/stderr 分离**:`--json` 下,机器数据(事件 NDJSON)走 stdout,所有人读提示/进度走 stderr(ccswarm 同款,便于 `agentpipe run x.yaml --json | jq`)。非 `--json` 维持现状(人读走 stdout)。
- 退出码:run 成功 0 / 失败非 0;validate 同理 —— 便于脚本与未来 CI 串接。

## 7. cost 聚合

`view` 与 `cost` 共用一个纯函数:遍历 NDJSON 的 `StepFinished.metrics`,累加 `cost_usd / num_turns / duration_ms`,按 step_id 归并(loop 内多轮按出现次数累加)。`runs` 列表里的"总成本"列复用它。无新数据源,纯计算。

## 8. 依赖变更

- engine:加 `chrono`(run-id 时间戳 + ts;Cargo.lock 已传递引入,提为直接依赖)。
- cli:加 `clap`(derive feature)+ `chrono`(展示时间)+ `serde_json`(读 NDJSON)+ `agentpipe-engine`(现状)。
- `~` 解析:用 `std::env::var("HOME")`(unix,项目已 cfg(unix));`$AGENTPIPE_HOME` 优先。

## 9. 测试

- `audit::RunRecorder`:喂一串事件 → 读回 NDJSON,行数/内容/首末行正确;I/O 失败(只读目录)不 panic、run 照常。
- run-id slug + allowlist:各种 name → 合法 run-id;`view ../etc/passwd` 类输入被拒。
- cost 聚合纯函数:构造含 loop 多轮的事件序列,断言累加正确。
- `diff`:两份构造 NDJSON,断言差异输出。
- CLI smoke(真实进程,符合全局基线):stub CLI 跑一个 run → `~/.agentpipe/runs/` 落文件 → `view` 能重读 → `cost` 数值匹配 → `validate` 对坏 yaml 报错。用 `$AGENTPIPE_HOME=临时目录` 隔离。

## 10. 非目标

- **replay = 重执行**:LLM 非确定性,重跑等于再 `run` 一次 task.yaml,价值低、成本高。明确不做;`view`(重读)已满足"看上次发生了什么"。
- undo / 改写 git 历史:破坏性,且 AgentPipe 不持有 git 操作主权(claude/codex 自己提交);如需,仅做 ccswarm 式 advisory 列 commit(不自动执行),留待独立 backlog。
- 远程/云端聚合、多机:仅本地 `~/.agentpipe/runs/`。
- 审计内容脱敏:NDJSON 仅本地、含 prompt/findings 明文;与现有终端输出同级敏感度,不额外脱敏(若未来上报再议,对齐 loom 系"日志仅本地"红线)。
- 日志轮转/清理:Phase 1 不做;`runs` 列表 + 手动删足够,容量策略留观察。

## 11. 落地顺序建议(供 plan)

1. engine `audit` 模块 + `RunRecorder` + run-id/slug/allowlist + 单测。
2. cli 迁 clap,接 `RunRecorder` 进事件循环(run 落盘),`--json` 分流。
3. `validate` / `--dry-run`(纯前置,不依赖 NDJSON,可与 1 并行)。
4. `view` / `cost` / `runs`(读 NDJSON)。
5. `diff`(依赖 4 的读取层)。
