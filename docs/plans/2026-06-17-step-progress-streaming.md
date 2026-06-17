# 实施计划:Step 进度可见性(轮次 + 耗时)

配套 spec:[../specs/2026-06-17-step-progress-streaming-design.md](../specs/2026-06-17-step-progress-streaming-design.md)。分 3 Phase,每片单独可 review / 可回滚。

## Phase 1 — 纯 UI:实时秒表 + 渲染进度行(零引擎依赖,先消除"冻住"错觉)

无需任何 Rust / 协议改动。最高性价比,先落。

1. `runReducer.ts`:`StepView` 加 `startedAt?: number`、`lastLine?: string`;`StepStarted` 写 `startedAt = Date.now()`;`StepProgress` 写 `lastLine = e.line`(保留 pushLog)。
2. `Console.tsx`:
   - 新增 `useElapsed(startedAt)` hook(`Running` 时每 1s setState tick,返回 `mm:ss`)。
   - 运行中 step 行右侧显示秒表;step 行下方暗色次行显示 `lastLine`(有才显示)。
3. 测试:`runReducer.test.ts` 加 `startedAt`/`lastLine` 断言;`npm run -C ui test`。
4. 验收:跑一个 claude quick-run,确认即使无 StepProgress 也能看到秒表走动(不再像挂死)。

> Phase 1 完即可交付「能看出在跑」。Phase 2 才补「在跑什么 / 跑了几轮」。

## Phase 2 — 引擎流式解析 + 结构化轮次/耗时

### 2a. 协议契约(改完先编译,catch 漂移)

5. `crates/engine/src/protocol.rs`:加 `StepMetrics` 结构;`StepProgress` 加 `round: Option<u32>`;`StepFinished` 加 `metrics: Option<StepMetrics>`。对新字段加 `#[serde(default)]` 保后向兼容。
6. `ui/src/types.ts`:镜像 —— `StepMetrics` 类型 + `StepProgress.round?` + `StepFinished.metrics?`。

### 2b. claude 流式解析

7. `crates/engine/src/runner/claude.rs`:
   - args 加 `--output-format stream-json --verbose`。
   - `ClaudeOutcome` 改为 `{ answer, metrics: Option<StepMetrics>, full_output }`。
   - 新增私有解析层(状态机:turn 计数 + label 派生 + result 提取),见 spec §5。answer fail-closed 回退链:`result.result` → 末个 assistant text 块 → 空串。
8. `crates/engine/src/executor.rs`:
   - `progress_sink` 回调签名加 `round: Option<u32>`(claude 传实际轮次,codex 传 `None`)。`Event::StepProgress` 带上 `round`。
   - `StepKind::Claude` 成功分支:`artifact: Some(out.answer)`(不再 `last_line`);`finish` 传 `out.metrics`。
   - `finish` / `StepFinished` 调用点带 `metrics`(claude 有,其余 `None`)。
9. 编译:`cargo build`(catch protocol/executor/runner 三处签名对齐)。

### 2c. UI 消费结构化字段

10. `runReducer.ts`:`StepView` 加 `round?`、`metrics?`;`StepProgress` 更新 `round`;`StepFinished` 存 `metrics`。
11. `Console.tsx`:运行中 step 显示轮次 chip(`round`);终态 step 显示 metrics chip(`{num_turns} 轮 · {(duration_ms/1000)}s · ${cost.toFixed(2)}`)。

## Phase 3 — 测试 + 真机 smoke + 收口

12. 固定样本:把 spike 抓的真实 NDJSON 存 `crates/engine/tests/fixtures/claude-stream-*.ndjson`(含 thinking / tool_use / text / result 各形态)。
13. 解析器单测:吃固定样本,断言 round 序列、label 文案、answer、metrics 数值。
14. `runReducer.test.ts`:round/metrics 分支断言。
15. 真机 smoke:本机真 claude 跑 `tests/fixtures/sample-task.yaml`,确认 spec §8 全部验收点(含下游 `{{...}}` 拿到答案文本而非 JSON)。
16. `/four-dimension-review` 自查:链路连贯(emit→bridge→reducer→render)、同构面(protocol↔types 两份手工镜像必须一致)、字面vs语义(`round` 0/1 起始、`duration_ms` 单位)、默认值最坏 case(解析失败 fail-closed answer、Option 缺省)。

## 风险与回滚

- 每 Phase 独立 commit(中文信息)。Phase 1 可单独发(纯增益)。Phase 2 若 stream-json 真机异常,回滚 2 即恢复文本模式,Phase 1 的秒表仍在。
- 最大风险点:`artifact` 从 `last_line` 改 `answer` 后下游 `{{step_id}}` 插值内容变化。Phase 3 smoke 必须显式验证多 step 串联(brainstorm→plan→implement)拿到的是答案文本。

## 待验(spike 未覆盖)

- 含真实 `tool_use` 的多轮 assistant 行结构(本次 spike 只跑了单轮纯文本)——Phase 3 固定样本需用一个真会调工具的 prompt 重抓。
- `result/error`(`is_error:true`)时 `result` 字段是否仍有文本 —— Phase 2 处理失败态时确认。
