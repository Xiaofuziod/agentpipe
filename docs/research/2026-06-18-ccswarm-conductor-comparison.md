# AgentPipe vs ccswarm vs Conductor 逐项对照

日期 2026-06-18。目的:把 ccswarm(最贴 AgentPipe 实现)和 Microsoft Conductor(最贴 AgentPipe 设计哲学)的 YAML schema、闸门/loop 收敛、审计可观测三块扒下来,跟当前 `templates/full-pipeline.yaml` + engine 实现逐项对照,沉淀可借鉴项。

数据来源:ccswarm README、Conductor 官方博客(2026-05-14)、本仓 `crates/engine/src/{protocol,executor,runner}`、`docs/specs/2026-06-16-design.md`、`docs/specs/2026-06-17-verify-gate-design.md`。

## 0. 三者定位

| | AgentPipe | ccswarm | Conductor |
|---|---|---|---|
| 语言 | Rust(+ Tauri 壳) | Rust | (CLI,MIT,Microsoft) |
| 形态 | 外部引擎,把 CLI 当黑盒串行编排 | 外部引擎,驱动 provider CLI 走固定 flow | 外部引擎,声明式 DAG + 确定性路由 |
| 拓扑 | 线性 steps 数组(顺序即拓扑)+ loop | stages 序列 + sangha 共识 | agent 图 + routes/when 条件路由 |
| provider | claude + codex 固定两家 | claude / codex / copilot 可选 | claude / copilot,per-agent 覆盖 |

一句话:AgentPipe 和 ccswarm 是同一物种(Rust + 黑盒编排 + 闸门 + 审计),Conductor 是同一哲学的更成熟形态(声明式 + 确定性 + 事件总线 + Web 看板)。

## 1. YAML Schema 对照

### AgentPipe(现状)

```yaml
version: 1
name: "<任务名>"
target: <目标仓库绝对路径>   # 单仓库
mode: auto                    # auto | step
steps:
  - id: implement
    kind: claude              # claude | codex | human | loop(固定枚举)
    skill: brainstorming      # 引用 Claude skill
    prompt: "..."
    verify: { by: codex, action: review-mr, max_retries: 2, on_unmet: gate }  # 设计中(A1)
  - id: codex-review
    kind: codex
    action: review-mr         # review-doc | review-mr | ask
    base: dev
  - id: codex-loop
    kind: loop
    until: codex-clean        # Phase1 仅此一个谓词
    max: 5
    body: [ ... ]
```

- 上下文传递:显式插值 `{{id.artifact}}` / `{{id.findings}}` / `{{id.verdict}}`。
- claude 一律 bypassPermissions 跑;无 per-step model / provider 覆盖。

### ccswarm

```yaml
stages:
  - id: sangha
    instruction: "Review the plan before implementation"
    permission: readonly        # 权限级别是一等字段
    provider: claude            # claude | codex | copilot
    model: sonnet
    sangha:                     # 多 agent 共识块
      quorum: 2
      members:
        - { id: planner,  persona: planner }
        - { id: reviewer, persona: reviewer }
        - { id: qa,       persona: qa }
    promotion: { provider: ... }      # 条件升级 provider
    on_rate_limit: { provider: ... }  # 限流时切 provider
```

- 内置 flow:`default` / `team` / `quick` / `research`;自定义放 `.ccswarm/flows/`。
- provider 优先级链:stage YAML → `--provider` flag → env → default。

### Conductor

```yaml
workflow:
  name: design-review
  entry_point: architect
  agents:
    - name: architect
      model: claude-opus-4.6-1m   # per-agent 模型/provider/温度
      prompt: "Create a design doc for: {{ workflow.input.purpose }}"
      output: { file_path: { type: string } }   # 结构化输出 schema
      routes:
        - to: reviewer
        - to: $end
          when: "{{ output.approved }}"          # Jinja2 条件路由
```

- 上下文模式三档:`accumulate`(全部前序输出) / `last_only` / `explicit`(仅命名依赖)。

### 差异结论

| 维度 | AgentPipe | ccswarm | Conductor |
|---|---|---|---|
| 拓扑表达 | 线性数组,顺序即拓扑 | stages 序列 | agent + routes 图 |
| 条件分支 | 无(明确 non-goal) | promotion/on_rate_limit 条件切 provider | when 路由,first match wins |
| provider/model 覆盖 | 无(固定 claude+codex) | 有,带优先级链 | 有,per-agent |
| 多 agent 角色 | step 类型枚举 | persona + quorum 成员 | 命名 agent + entry_point |
| 上下文传递 | 显式 `{{id.field}}` 插值 | (未明示) | accumulate/last_only/explicit |
| 权限 | claude 恒 bypass | permission 一等字段 | (未明示) |

可借鉴(按价值排序):
1. provider/model per-step 覆盖 + 优先级链(ccswarm/Conductor 都有)——AgentPipe 现在 claude 恒 bypass、codex 固定,想"某步用便宜模型分类、某步用 opus 推理"做不到。
2. 上下文可见性模式(Conductor 的 accumulate/last_only/explicit)——AgentPipe 现在全靠手写插值,等于永远 explicit;大流程下"自动带上前序"会更省事。
3. permission 作为 step 字段(ccswarm)——AgentPipe 把"恒 bypass"写死,与全局设计基线"读写默认会写、最保守分支"略有张力,可考虑 readonly/write 显式声明。

暂不借鉴:条件路由(when)。AgentPipe 已在 design.md §non-goal 明确"先只做线性 + loop + gate",符合个人用、可控优先的定位;等真出现"codex 没意见就跳过 simplify"这类需求再上。

## 2. 闸门 / loop 收敛机制对照

### AgentPipe

- loop:`until: codex-clean`(Phase1 唯一谓词)+ `max` + `body`;到 max 未收敛 → `LoopMaxReached` 暂停交人,不静默通过。
- 收敛信号:codex review 走 `--output-schema` 强制结构化 `verdict ∈ {clean, changes_requested}`。**fail-closed**:解析失败 / 文件缺失 / verdict 缺失一律按 `changes_requested` 处理,绝不静默判干净(design.md §8)。
- Gate:human 介入点,可 批准 / 跳过 / 改参 / 打断(`GateKind` + `ApproveGate`)。
- verify gate(设计中 A1):任意 claude/codex 步骤挂 `verify: { by, action, max_retries, on_unmet: gate|fail|continue, feedback }`,寄生现有失败重试环;洞察是"codex-loop 本质就是手搓的 verify-retry,verify gate 才是底层原语"。

### ccswarm

- sangha 共识:N 个 member 各自独立判 `SANGHA_DECISION=APPROVE|REVISE`,达 `quorum` 才推进——多 agent 投票门。
- `review-fix` flow:循环 review → fix until 通过。

### Conductor

- `when` 条件路由回跳实现 loop(not approved → route back to architect)。
- 安全限额:max iteration **+ wall-clock timeout** 双保险防 runaway。
- script step:跑 `pytest` 之类,用 exit code 分支——把"确定性校验"交给真命令而非 LLM。

### 差异结论

| 维度 | AgentPipe | ccswarm | Conductor |
|---|---|---|---|
| 收敛信号 | 单 codex 结构化 verdict | 多成员 quorum 投票 | 任意 Jinja2 表达式 / exit code |
| fail-closed | ✅ 解析失败判 changes_requested | (未强调) | (未强调) |
| runaway 防护 | max → 暂停交人 | review-fix 循环 | max iteration + 墙钟 timeout |
| 多元校验 | 单 verifier(未来 by 可扩) | quorum 多 persona 投票 | 多 route 条件 |
| 确定性校验门 | 无(只有 codex LLM 审) | 无明示 | script step(pytest 等) |

AgentPipe 的强项:fail-closed 收敛(符合全局设计基线"判定函数抛错走最保守分支")+ max 到顶不静默通过。这两点 ccswarm/Conductor 文档都没强调,是 AgentPipe 设计上更稳的地方。

可借鉴(按价值排序):
1. **script/命令型校验门**(Conductor)——当前 verify gate 只有 codex 一种 LLM 审;但很多"目标达成"判据其实是确定性的(测试绿、`cargo build` 过、lint 干净)。给 verify 加一个 `by: command` / `kind: script` 变体,exit code 即 verdict,比让 codex 读 diff 判定又快又准,且天然 fail-closed。**这是最高价值借鉴项**,直接补强正在做的 verify gate。
2. **墙钟 timeout**(Conductor)——AgentPipe 现在只有 loop max 轮数兜底,没有单步/整 run 的 wall-clock 上限;慢任务或 CLI 卡死时缺这道防线。
3. **多 verifier 投票**(ccswarm quorum)——verify gate 的 `by` 未来可从单 codex 扩成"codex + claude 双判,都 clean 才过",对应 sangha。优先级低于前两项(个人用场景单 verifier 够)。

## 3. 审计 / 可观测对照

### AgentPipe(现状,已核对源码)

- 事件枚举 `Event`(protocol.rs):`RunStarted / StepStarted / StepProgress / StepAwaitingGate / StepFinished / StepFailed / LoopIteration / LoopConverged / RunFinished`。
- StepProgress 实时流式已落地(Task7);最新 commit 加了运行可观测性(轮次/耗时)。
- `StepMetrics.cost_usd` 已采集(从 claude `result.total_cost_usd` 解析),但**仅单步打印,无跨 run 聚合**。
- CLI 只有 `run` 一个子命令;`validate` 是 run 内部自动调(`m.validate()`),无独立 `validate` / `dry-run` 子命令。
- **事件只打到 stdout,不落 NDJSON,无 run-id,无 replay/diff/undo**。

### ccswarm(最成熟,最值得抄)

- NDJSON 全审计:每个动作落 newline-delimited JSON。
- `ccswarm replay <run-id>`:按记录重放;`undo <run-id>`:advisory 列 commits,**绝不自动改写历史**;`run diff <a> <b>`:比两次 run 事件时间线;`run view <run-id>`:全事件日志。
- `cost <run-id>`:per-stage + per-agent 成本拆解。
- `--json`:数据走 stdout,trace/log 走 stderr(分离)。
- run-id 严格 `[A-Za-z0-9_-]` allowlist 防路径穿越。

### Conductor

- pub/sub 事件总线:terminal renderer / web dashboard / 未来 consumer 各自独立订阅。
- Web dashboard:实时 DAG 可视化,per-node 看 prompt / model / token / cost / 活动流 / 输出。
- `conductor validate` + `--dry-run`:执行前抓错。

### 差异结论

| 能力 | AgentPipe | ccswarm | Conductor |
|---|---|---|---|
| 实时事件流 | ✅(StepProgress) | ✅ | ✅(pub/sub) |
| 持久化审计(NDJSON/run 落盘) | ❌ | ✅ | ✅ |
| replay / diff / undo | ❌ | ✅ | (replay 类未明示) |
| cost 追踪 | 单步采集,无聚合 | per-stage+per-agent | per-node |
| validate / dry-run 子命令 | ❌(仅 run 内部 validate) | (未明示) | ✅ |
| run-id + 安全 allowlist | ❌ | ✅ | (未明示) |
| Web 看板 | Phase1b 计划(Tauri) | (CLI) | ✅ |

可借鉴(按价值排序):
1. **NDJSON 落盘 + run-id**(ccswarm)——AgentPipe 现在事件只打 stdout,run 结束即蒸发。落 `~/.agentpipe/runs/<run-id>.ndjson` 后,replay/diff/事后排错全有了地基;run-id 走 allowlist。**这是审计维度最高价值项**,且 Conductor 的 pub/sub 总线思路天然兼容(订阅者之一就是 NDJSON writer,另一个是 Tauri GUI)。
2. **`validate` + `--dry-run` 子命令**(Conductor)——把已有的 `m.validate()` 暴露成独立命令 + 加 dry-run(只解析+校验+打印执行计划,不真跑 CLI),写 task.yaml 时先验。成本极低。
3. **cost 聚合 + `cost` 子命令**(ccswarm)——`cost_usd` 已经在采,只差按 run/step 聚合输出。
4. **stdout/stderr 分离 + `--json`**(ccswarm)——数据走 stdout、日志走 stderr,方便脚本消费。
5. undo(ccswarm,advisory 不改历史)——符合全局规范"破坏性 git 需显式确认";优先级低,但语义正确值得记。

## 4. 借鉴清单汇总(给后续 plan)

按"价值 / 成本"排序,候选 backlog:

| 项 | 来源 | 价值 | 成本 | 落点 |
|---|---|---|---|---|
| verify gate 加 `by: command`(exit code 即 verdict) | Conductor script step | 高 | 低 | 正在做的 verify gate(A1)直接扩 |
| NDJSON 落盘 + run-id allowlist | ccswarm | 高 | 中 | engine 事件总线加 writer 订阅者 |
| `validate` + `--dry-run` 子命令 | Conductor | 中 | 低 | cli crate |
| 整 run / 单步 wall-clock timeout | Conductor | 中 | 低 | executor |
| cost 聚合 + `cost` 子命令 | ccswarm | 中 | 低 | 复用现有 cost_usd |
| provider/model per-step 覆盖 + 优先级链 | ccswarm/Conductor | 中 | 中 | manifest schema + runner |
| replay / run diff 子命令 | ccswarm | 中 | 中 | 依赖 NDJSON 落盘先行 |
| 上下文可见性模式(accumulate/last_only/explicit) | Conductor | 低 | 中 | context.rs |
| 多 verifier 投票(quorum) | ccswarm sangha | 低 | 中 | verify gate `by` 扩展 |
| 条件路由(when) | Conductor | 低 | 高 | 与 design.md non-goal 冲突,暂缓 |

AgentPipe 现有且不输对手的设计(保持):fail-closed 收敛、max 到顶交人不静默通过、显式插值的可控数据流、Rust 引擎 + 计划中的 Tauri pub/sub 看板(与 Conductor 同构)。
