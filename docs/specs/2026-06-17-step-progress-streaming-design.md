# Step 进度可见性:模型请求轮次 + 耗时(设计)

日期 2026-06-17。环境同 [cli-behavior-findings.md](cli-behavior-findings.md):claude 2.1.x,codex 0.139.x,macOS。

## 1. 问题

控制台对一个 `Running` 的 claude step 只显示一行 `▷ {id}`,执行期间(可能数分钟)无任何进度,用户**无法区分"挂死"与"正在跑"**。这正是要补的能力:让人看到「模型请求轮次」和「耗时」。

### 根因(两层 gap)

链路本身是通的:`run_command`(`runner/mod.rs`)独立线程实时逐行读 stdout → `on_line` 回调 → executor `progress_sink` emit `Event::StepProgress` → bridge 转发 webview → runReducer。但:

1. **上游无流式**:`runner/claude.rs:35` 用 `claude -p "<prompt>"`(文本 print 模式),claude 全程不吐中间行,只在结束时一次性打印最终答案。于是整段执行期 `on_line` 几乎不触发 → 无 `StepProgress`。
2. **下游不渲染**:即便有 `StepProgress`,runReducer 只把它塞进 `RunState.log[]`(封顶 1000),而 `Console.tsx` 根本没渲染 `log` —— `Running` step 只显示 `summary`(运行中恒为空)。

两层都得治,否则单治一层仍然空白。

## 2. 实测:claude stream-json 输出形状(本次 spike,grounded)

`claude -p "<prompt>" --output-format stream-json --verbose` 输出 NDJSON,每行一个对象,`type` 字段区分:

| type / subtype | 含义 | 关键字段 |
|---|---|---|
| `system` / `hook_started`,`hook_response` | hook 生命周期 | 忽略 |
| `system` / `init` | 会话初始化 | `model`,`tools`,`cwd`,`permissionMode` |
| `assistant` | 一轮模型响应(= 一个请求轮次) | `message.content[]`(块:`thinking`/`text`/`tool_use`)、`message.usage`、`message.model` |
| `rate_limit_event` | 限流信息 | 忽略(或可选提示) |
| `result` / `success`\|`error` | 终态 | `num_turns`、`duration_ms`、`duration_api_ms`、`ttft_ms`、`result`(最终答案文本)、`total_cost_usd`、`usage`、`is_error` |

- 轮次 = `assistant` 行计数,终值 = `result.num_turns`。
- 耗时 = wall-clock(UI 端从 StepStarted 计)+ 终态 `result.duration_ms`(总)/`duration_api_ms`(纯 API)。
- `tool_use` 块的 `name` 字段 = 正在调用的工具(Bash/Read/Edit…),是「它在干什么」的最佳信号。
- **答案文本在 `result.result`**,不是 stdout 最后一行。

## 3. 候选方案与取舍

### 方案 A:运行层解析、协议保持文本(小改面)

`claude.rs` 切 stream-json,内部解析每行 → 拼成人读字符串喂 `on_line`("第 2 轮 · 调用 Bash"),`result.result` 取作答案。协议(`protocol.rs`)与事件结构完全不动,UI 只需渲染已有的 `log`/进度行。

- 优点:blast radius 最小,不动协议四件套(protocol/types/reducer/Console 仅 Console 加渲染)。
- 缺点:轮次/耗时是文本里的字,UI 无法做独立的「轮次计数 chip」「实时计时器」「成本」结构化展示;想要就得 UI 反解字符串,丑且脆。

### 方案 B:结构化事件(推荐)

`claude.rs` 解析 stream-json,产出结构化信号:
- 每轮 → `StepProgress` 扩一个 `round: Option<u32>` + 文本 `line`(工具/文本摘要)。
- 终态 → `StepFinished` 扩一个 `metrics: Option<StepMetrics>`(`num_turns` / `duration_ms` / `cost_usd`)。
- 答案 = `result.result`。

UI 据此渲染:运行中 step 的「实时计时器(UI 时钟)+ 当前轮次 chip + 最近一行在做什么」,终态「✓ done · 3 轮 · 3.3s · $0.49」。

- 优点:精确命中需求(轮次 + 耗时都是一等公民、可独立排版、可扩展),符合「补全这部分」的产品意图。
- 缺点:动协议 → 四处同步(`protocol.rs` / `ui/types.ts` / `runReducer.ts` / `Console.tsx`);bridge 自动转发(serde),无需改。工作量略大。

### 取舍结论:选 B,但严格控制扩面

`StepProgress` 只加一个 optional `round`;`StepFinished` 只加一个 optional `metrics`。两者都是后向兼容的 Option 追加(`runReducer` default 分支已对未知字段安全)。不新增独立事件类型、不改 codex 路径语义。codex 的 stdout 行继续作为无 round 的纯进度行流动(顺带受益于 Console 开始渲染进度行)。

### 决定性收益:实时计时器是纯 UI、零引擎依赖

「是否挂死」最关键的信号是运行中 step 上的**实时秒表**——从 `StepStarted` 时刻起 UI 每秒 tick。它不依赖任何引擎/CLI 改动,甚至在第一轮 assistant 到达之前(等首个模型响应,实测 ttft 数秒起)就能显示 `运行中 0:42`,立刻消除"冻住"错觉。故拆为独立 Phase 先落,代价最低、收益最高。

## 4. 数据契约变更

### 4.1 protocol.rs(Rust,SSOT)

```rust
pub struct StepMetrics {
    pub num_turns: u32,
    pub duration_ms: u64,
    pub cost_usd: f64,
}

pub enum Event {
    // ...
    StepProgress { step_id: String, line: String, round: Option<u32> }, // +round
    StepFinished { step_id: String, status: StepStatus, summary: String, metrics: Option<StepMetrics> }, // +metrics
    // ...
}
```

### 4.2 ui/types.ts(手工镜像,必须与 Rust 同步)

`EngineEvent` 的 `StepProgress` 加 `round?: number`;`StepFinished` 加 `metrics?: StepMetrics`;新增 `StepMetrics` 类型。

### 4.3 runReducer.ts

- `StepView` 加 `round?: number`、`startedAt?: number`、`lastLine?: string`、`metrics?: StepMetrics`。
- `StepStarted`:记 `startedAt = Date.now()`(UI 时钟,避免引擎/UI 跨进程时钟偏移)。
- `StepProgress`:更新该 step 的 `lastLine` 与 `round`(仍同时 pushLog 保留流水)。
- `StepFinished`:存 `metrics`。

### 4.4 Console.tsx

运行中 step 行追加:实时秒表(`useElapsed(startedAt)` 每秒 tick)+ 轮次 chip(`round`)+ `lastLine`(暗色次行)。终态行追加 metrics chip(`{num_turns} 轮 · {duration}s · ${cost}`)。

## 5. claude.rs 解析层设计

新增 `runner/claude_stream.rs`(或 `claude.rs` 内私有 mod),状态机式逐行喂:

- 入参:每行 `&str` + 一个回调(round/label)+ 累加器。
- `assistant` 行:turn 计数 ++;从 `content[]` 派生 label:
  - 有 `tool_use` 块 → `调用 {name}`(多个取首个 + `等`)。
  - 否则有 `text` 块 → 文本前 ~60 字(压扁换行)。
  - 否则 `thinking` 块 → `思考中`。
  - 回调 `(round=turn, line=label)`。
- `system/init` 行:回调 `(round=None, line="{model} 就绪")`(可选,低优先,先不发以免噪声)。
- `result` 行:解析 `result`(答案文本)+ `num_turns`/`duration_ms`/`total_cost_usd` → 存入 outcome,不发 progress。
- 其他 type:忽略。
- 非 JSON / 解析失败的行:display 侧 fail-open(当作纯文本 progress 行,无 round);**但答案/artifact 提取 fail-closed**(拿不到 `result.result` 时回退到「最后一个 assistant 的 text 块」,再不行回退空串 + 标注,绝不把 JSON 当答案塞下游)。

`ClaudeOutcome` 改为:

```rust
pub struct ClaudeOutcome {
    pub answer: String,          // result.result(回退见上)
    pub metrics: Option<StepMetrics>,
    pub full_output: String,     // 原始 NDJSON,留作调试
}
```

executor `StepKind::Claude` 分支:`artifact: Some(out.answer)`(不再是 `last_line`),`finish` 带 `out.metrics`。

> `on_line` 仍是 `FnMut(&str)` 不够用(要带 round)。两条路:(a) 把 progress sink 升级成带 round 的回调;(b) claude.rs 解析后把 round 编进约定前缀字符串、executor 侧不解析。选 (a):executor 的 `progress_sink` 增一个 round 形参;codex 调用处传 `None`。codex 的 `on_line` 签名不变(codex 不切流式),仅 executor 内部回调签名调整。具体在 plan 里定。

## 6. 边界与错误路径

- claude 非零退出:现有逻辑 `success==false → Err(Cli)` 保留;失败仍走决策 gate。但要先 drain 已解析的 metrics?——失败态不展示 metrics,保持简单。
- stream-json 中途被 Abort(killpg):reader 线程被 drop,已解析的 round 已发,answer 取不到 → step 进失败/中止路径,不污染下游。
- claude 输出超大(长 implement):NDJSON 行可能很长(单条 assistant 含大量 tool_use)。progress label 截断 60 字;`full_output` 仍全量进内存(与现状一致,暂不落盘,后续若 OOM 风险再预算化)。
- 旧引擎 / 新 UI 或反向:Option 字段后向兼容;UI runReducer default 分支吞未知事件;Rust serde 对缺失 Option 字段需 `#[serde(default)]`,确保老 manifest/事件不 panic。
- codex 路径:不变,继续无 round 流式;Console 渲染进度行对其同样生效(bonus)。

## 7. 不做(明确划界)

- 不做 token/成本的实时累加显示(终态 cost 足够);不做 per-turn 独立计时(wall-clock 总计时足够)。
- 不动 codex 为结构化轮次(codex exec 输出非 NDJSON 协议,另立项)。
- 不把 `full_output` NDJSON 落盘(暂留内存,与现状一致)。

## 8. 验证

- 单测:`claude_stream` 解析器吃 spike 抓到的真实 NDJSON 固定样本(`tests/fixtures/`),断言 round 序列、label、answer、metrics。
- 单测:`runReducer` 新字段(round/startedAt/metrics)分支。
- 真机 smoke:本机真 claude 跑 `tests/fixtures/sample-task.yaml`,肉眼确认计时器走动、轮次递增、终态 metrics、下游 `{{...}}` 拿到的是答案文本而非 JSON。
