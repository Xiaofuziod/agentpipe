# codex review 排除低价值路径（pathspec exclude）

日期：2026-07-10
状态：设计
影响：crates/engine/src/manifest.rs、runner/codex.rs、executor.rs、
templates/mr-review-loop.yaml、ui/src/types.ts、ui/src/composer/StepDrawer.tsx

## 1. 动机

实测 `loom-agent-client` 的 `git diff main...HEAD`：287 文件 / 36495 行 / 约 2MB。
codex 逐文件读，实测节奏每次工具调用约 6.7 秒（绝大部分是模型决策时间），
按每文件读一次估算需要约 32 分钟，超过 `DEFAULT_CODEX_TIMEOUT_SECS = 1200`（20 分钟），
review 步骤必然超时。

其中 66% 的行（96 文件 / 24226 行）对 code review 没有价值：

| 类别 | 例子 |
|---|---|
| vendored 依赖 | packages/loom-acp-codec/vendor/acp-v2/schema.json（5037 行） |
| 锁文件 | package-lock.json（1718 行） |
| 设计文档 | docs/plans/（数千行） |
| 生成代码 | generated/ 目录 |

按模板推荐清单排除后为 255 文件 / 20323 行（测试文件不排除 —— 测试是代码，该审）。

注意：排除主要削减行数与体积，不是文件数。耗时正比于文件数，详见第 6 节。

## 2. 关键代码事实

引擎**不自己计算 diff**。`runner/codex.rs` 的 review-mr 只是在 prompt 里叫 codex
去执行 `git diff {base}...HEAD`，由 codex 在自己的 read-only 沙箱里跑。
因此排除范围只能表达在那条命令里。

## 3. 方案

### 候选 A（采纳）：prompt 内嵌 pathspec

manifest 的 codex step 新增 `exclude: Vec<String>`，引擎把它拼成 git pathspec
附在 prompt 的 diff 命令后：

```
git diff {base}...HEAD -- ':(exclude)**/vendor/**' ':(exclude)**/package-lock.json'
```

优点：改动小；codex 仍按需分段读文件，不撑上下文。
缺点：软约束 —— codex 是 LLM，可以不照抄该命令。实践中它照抄；且即便不照抄，
最坏结果只是退回今天的行为（审全部），不会漏审之外的东西。

### 候选 B：引擎算出白名单文件列表注入 prompt

约束略强，但仍是软的，且要多跑一次 git。收益不抵复杂度。否决。

### 候选 C：引擎生成 diff 内容经 stdin 喂给 codex

唯一的硬约束（codex 只能看到给它的内容）。否决：12269 行约 500KB，撑爆上下文；
且剥夺 codex 按需追溯上下文（读被改函数的调用点）的能力，反而降低 review 质量。
review-doc 走 stdin 是因为文档是自包含的，diff 不是。

## 4. 默认值：不排除任何路径

`exclude` 默认为空，引擎默认审全部 —— 与今天行为逐字一致。

理由：漏审是安全风险，默认值里藏静默漏审违背本项目 fail-closed 基线。
推荐清单放进 `templates/mr-review-loop.yaml`，用户看得见、改得动、可删。

## 5. 注入防护

`exclude` 条目会被 codex 抄进 shell 命令执行，等同于把用户输入送进 shell。
校验参照现有 `base_ref_resolvable` 的字面 reject（codex.rs），fail-loud：

- 拒空白与控制字符（含换行）
- 拒引号 `'` `"`、反引号、`$`、`;`、`&`、`|`、`<`、`>`、`(` `)`
- 拒以 `-` 开头（避免被 git 当 option）
- 空条目拒

任一条目非法 → `EngineError::Cli` 明示哪一条、为什么，不静默丢弃。

## 6. 超时可解释

**exclude 救不了超时。** 用模板清单在 loom-agent-client 实测：

| 指标 | 排除前 | 排除后 | 变化 |
|---|---|---|---|
| 文件数 | 287 | 255 | -11% |
| 行数 | 36495 | 20323 | -44% |
| 体积 | 2.09 MB | 1.24 MB | -40% |

codex 的耗时正比于**文件数**（每个文件一次 `nl`/`sed` 读取 + 一次模型决策，实测约
6.7 秒/次），不是行数 —— 5037 行的 vendored schema.json 再大也只占一个文件。
排除主要省的是 token 与上下文压力：255 × 6.7 秒 ≈ 28.5 分钟，仍超过默认 20 分钟。

故本特性不承诺解决超时。真正要跑完大 MR，仍需调高 `AGENTPIPE_CODEX_TIMEOUT_SECS`。

不改 `DEFAULT_CODEX_TIMEOUT_SECS`（小 diff 场景 20 分钟合理），改为让超时报错
可解释：同时给出「调高超时」与「用 exclude 缩小范围」两条出路。符合"校验失败的
报错要可解释"基线。

## 7. GUI

不给 composer 加 exclude 编辑控件（模板已带推荐清单，手编 YAML 是当前的既定路径）。
但必须处理一个与 `vet` 逐字同构的坑（StepDrawer.tsx 注释 F6）：

`exclude` 只对 review-mr 合法（第 5 节的 validate 会拒别的 action）。若用户加载带
exclude 的模板 step，再在 GUI 里把 action 切成 ask/review-doc，隐形携带的 exclude
会在保存/运行时才报错，而 GUI 里没有任何控件能清掉它。故 action 切换时与 `vet`
一样显式剥离。types.ts 的 codex step 同步加上 `exclude?: string[]`（该文件注释
声明与 manifest.rs 手工镜像同步）。

## 8. 验收

- exclude 为空 → prompt 与今天逐字一致（回归）。
- exclude 非空 → prompt 含 `-- ':(exclude)<pat>'` 逐条。
- 非法条目（引号 / 分号 / `-` 开头 / 空白 / 空串）→ Err，错误信息点名该条目。
- 超时错误信息含 `AGENTPIPE_CODEX_TIMEOUT_SECS`。
- manifest 能解析 exclude；未写该字段的既有 manifest 照常解析。
- 全量 cargo test 绿。
