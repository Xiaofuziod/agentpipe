# 控制台单步输出可展开/折叠设计

## 背景与目标

控制台(中列)此前每步只渲染一行:运行中显示最近一条进度行 `lastLine`,终态显示
`summary`/`error`。流式过程行没有逐条留存,用户看不到完整 / 实时输出。

目标:让每步的实时输出可见、可折叠、想看时随时展开。

- 运行中的步默认展开,新进度行到达自动滚到底 → 看实时输出。
- 紧凑为先:非焦点步默认折叠,点行头按需展开。
- run 结束后保持最后一步展开,不在完工瞬间把正在看的输出收走。

## 数据流

```
引擎 StepProgress{line} → runReducer 把每行 append 进 StepView.lines(封顶 500)
  ├─ live:  逐行累积,展开态滚动查看
  └─ 回放:  view_run 重放 StepProgress 事件 → 重建 lines(ndjson 已落 StepProgress)
```

## 设计要点

### 每步留存输出行(runReducer)

- `StepView` 新增 `lines?: string[]`,`StepProgress` 时 `appendLine` 追加;
  `STEP_LINE_CAP = 500` 封顶防长 run 撑爆内存(保留最近 500 行)。
- `StepStarted` **重置本步视图**(`lines`/`lastLine`/`summary`/`error`/`metrics`/`round`
  清空):`loop` 内同 `step_id` 重跑是一次全新尝试,不重置会让展开输出跨迭代累积、而
  summary/metrics 只反映最后一次,二者串味不一致(治本于 reducer,非 UI 兜底)。

### 焦点步展开模型(Console)

- 默认展开的是"焦点步":正在运行的步;无运行步时是 `order` 的最后一步。
- `isExpanded(id) = override[id] ?? id === focusId`;用户点行头写 `override` 覆盖默认。
- 效果:执行推进时焦点随之移动(同时只开一个 feed,跟随活跃步),run 结束后最后一步
  保持展开;不会在某步完工瞬间默认值翻转把输出折叠掉。

### 渲染(StepLine)

- 折叠态:`cline-head` 单行(caret / mark / id / round / main / metrics / timer)。
- 展开态:`cline-feed` 滚动块(等宽、max-height 280px)渲染全部 `lines`。
- 无进度行的步(纯 command/human/codex 等)`hasLines=false`,不显示 caret,渲染同旧样式。
- 运行中 + 展开时,`lines.length` 变化触发把 feed 滚到底(实时跟随)。

## 边界

- 历史回放:ndjson 含 StepProgress,`replayToState` 折叠重建 `lines`,可展开回看。
- `lastLine` 与 `lines` 总是同处(StepProgress)写入,不存在"有 caret 但展开为空"或
  "有内容但无 caret"的不一致。
- 内存:每步 ≤500 行 × 步数,Phase 1 个人本地工具可接受。
