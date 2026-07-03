# watch 外循环设计（Phase B）

日期：2026-07-03
状态：待评审
前置：Phase A 的 D1（ACP budget 硬拦）必须先落地；决策门 stdin-EOF → Abort 已落地（commit 652e089）
路线图定位：[2026-07-03-evolution-roadmap.md](2026-07-03-evolution-roadmap.md) Phase B

## 1. 背景与目标

AgentPipe 目前是单发管线：人贴 MR 链接 → loop 收敛 → run 结束。Loop Engineering 的分水岭在"谁来触发循环"。本设计让 pipeline 自己订阅工作源：

- `agentpipe watch watch.yaml`：常驻进程轮询 GitHub PR，命中过滤条件的新 item 自动用既有模板实例化一次 headless run，结果回写 PR 评论，需要人时发通知。
- 定位是"长驻的评审代理"，不是任务面板：串行、fail-closed、全程 NDJSON 审计。

成功标准：给一个 repo 挂上 watcher 后，贴了指定 label 的 PR 在无人操作下得到"Codex 审 → Claude 修 → 循环收敛"的完整处理与评论回执；任何不收敛 / 失败 / 需授权的情况都以非成功状态显式暴露，绝不静默放行。

## 2. 非目标

- ❌ daemon 管理（开机自启 / 进程守护交给用户的 launchd / systemd；watch 是普通前台进程）
- ❌ run 暂停恢复（决策门在 headless 下 Abort，V2 再考虑 parked-run 复活）
- ❌ 多 run 并发（MVP 串行队列，一次一个 run）
- ❌ GitLab / 其他 forge（source 设计成 enum 留扩展位，MVP 只做 github-pr）
- ❌ 服务端 / webhook 接入（轮询足够，不引入网络监听面）

## 3. 候选方案

- A) `agentpipe watch` 子命令：常驻循环 = `loop { scan_once(); sleep(interval) }`，同时暴露 `--once` 只跑一轮扫描后退出。【推荐】
- B) 纯 cron 方案：只做无状态的 `scan-once` 子命令，调度完全交给 cron。实现最小，但单日预算累计、串行队列、通知去重都要在无常驻状态下重造，反而复杂。
- C) GUI（Tauri）托管 watcher。体验好但把无人值守能力绑死在开着窗口的桌面上，与"长驻代理"定位矛盾。

决策：A，且把 scan_once 实现为无状态纯核心（状态全在磁盘 state 文件），`--once` 免费送给偏好 cron 的用户——A 兼容 B 的使用方式。

## 4. 设计

### 4.1 watch.yaml（新清单类型，不复用 task manifest）

```yaml
version: 1
name: "loom-vscode MR guard"
source:
  kind: github-pr
  repo: owner/name          # gh 的 -R 参数
  label: agentpipe-review   # 只认带此 label 的 PR
  authors: [alice, bob]     # 作者白名单;必填且非空(fail-closed)
interval_secs: 300
template: /abs/path/mr-review-loop.yaml
bindings:
  mr: "{{item.url}}"        # 模板内 human step id -> 预置 value(支持 {{item.*}} 插值)
budget:
  per_run_usd: 5.0          # 覆盖模板的 budget_usd
  daily_usd: 30.0           # 当日累计上限,超过即停摆
notify: "osascript -e 'display notification \"{{message}}\"'"   # 可选,置空则仅评论
```

校验规则（fail-closed）：

- `authors` 缺失或空数组 → 校验错误。空白名单 = 谁都不信，必须显式列人。这是 prompt 注入的第一道防线：PR 内容会进入 bypassPermissions 的 agent 上下文，等于把执行权交给 PR 作者，watch 只能对可信作者开。
- `template` 必须存在且 `Manifest::validate` 通过；若 watch 设了 `budget.per_run_usd` 而模板含 ACP step，Phase A D1 的 allow_unmetered 规则同样适用（在 watch 启动期即拦下，不等 run 时）。
- 模板声明了 `allow_unmetered: true` → watch 启动校验**默认拒绝**：无人值守下双层预算对 ACP step 记 0（audit 只统计带 metrics 的终态），"兜底"对它是空转；确要挂载须在 watch.yaml 顶层再显式声明 `allow_unmetered_template: true`（双重签字——无人值守的确认门槛应高于交互场景）。
- `bindings` 的 key 必须命中模板中 human step 的 id，插值只支持 `item.*` 命名空间；且模板内**每个** human step 都必须被 bindings 覆盖、插值后非空——headless 下未预置的 human 门会被 Skip，下游 `{{id.artifact}}` 插值成空串静默喂给 bypassPermissions 的 agent（EOF-Skip + context 空串回退的组合坑），必须在启动期拦。
- 模板 `mode` 必须为 auto：step 门控模板在 headless 下会被逐步 Skip 甚至以 Success 收尾，校验期拒绝。

### 4.2 item 身份与状态

- item key = `PR 编号 + head SHA`。同一 PR 推了新 commit 就是新 item，自动复审；未变的 head 不重复处理。
- state 文件：`base_dir()/watch/<name>.json`（base_dir 语义同 runs 目录：$AGENTPIPE_HOME 或 $HOME 拼 /.agentpipe，公共 helper 抽法见 acp-hardening spec D4），内容：已处理 key 集合、每 key 的处置结果（run id + 终态）、按日期滚动的当日累计成本。原子写（tmp + rename），启动时读，坏文件 fail-loud 拒启（宁可人来看一眼，不冒重复烧钱 / 重复评论的险）。

### 4.3 scan_once 流程

1. `gh pr list -R <repo> --label <label> --json number,headRefOid,author,url,title` 拉候选。
2. 过滤：作者在白名单内、key 未处理过。
3. 逐个（串行）处理每个新 item：
   - 加载模板 manifest，注入 bindings（复用 human step 既有的 `value` 预置机制——引擎已支持 inline 注入跳过人工 gate），budget 用 `per_run_usd` 覆盖。
   - 以 headless 语义执行（进程内直接调 engine，与 CLI run 同路径；决策门无人应答即 Abort——沿用 stdin-EOF 语义在 watch 内的等价实现：watch 模式下 Decision 门直接回 Abort，不等输入）。
   - run 结束按终态处置：
     - Success + LoopConverged → `gh pr comment` 回写"收敛，N 轮，残留 M 条（如有）、run id、成本"
     - Aborted（决策门 / 权限门 / loop 耗尽 max）→ 评论"需要人工介入 + 原因 + run id" + 触发 notify
     - Failed → 评论失败摘要 + notify
   - 处置结果写入 state 文件（先落盘再进入下一 item）。
4. 每个 run 结束后把 audit 聚合成本累加进当日账；超过 `daily_usd` → notify + 进程以非零码退出（fail-closed：停摆让人来看，不静默降频）。

### 4.4 gh 依赖与测试注入

- forge 操作全部经 `gh` CLI（复用用户既有认证，不碰 token 存储）。二进制路径经 `AGENTPIPE_GH_BIN` 覆盖——与 `AGENTPIPE_CLAUDE_BIN` / `AGENTPIPE_CODEX_BIN` 同一套 stub 模式，测试与 demo 都靠它。
- `gh` 调用失败（网络 / 认证 / 限流）→ 本轮扫描跳过并记日志，进程不退（瞬态错误容忍）；连续 N 轮（默认 5）全失败 → notify + 退出（持续故障不静默空转）。

### 4.5 安全与威胁模型（文档必须随功能落 README）

- 信任边界：白名单作者的 PR 内容视为可信输入；白名单外一律不碰。这与全局设计基线"会执行外部代码的操作必须在用户信任确认后才跑"对齐——白名单就是那个信任确认。
- watch.yaml 与模板都只接受本机路径，watch 不提供任何远程拉取配置的机制。
- 评论回写内容仅含 verdict / findings 摘要 / run id / 成本，不含日志全文（避免把内部路径 / 环境细节外泄到 PR）。
- 预算盲区显式声明：`daily_usd` 基于 audit 聚合成本，只覆盖上报 metrics 的 step；ACP step 在 metrics 接通前对两层预算均不可见（这正是启动校验默认拒绝 allow_unmetered 模板的原因）。

## 5. 错误路径盘点

- state 文件损坏 → fail-loud 拒启。
- 模板校验失败 → watch 启动期拒启（不是发现 item 后才炸）。
- run 进行中 watch 进程被杀 → run 的 NDJSON 审计已落盘可查；重启后该 item 的 key 未写入 state（处置结果落盘在 run 结束后），会重跑一次——语义是 at-least-once，评论可能重复，可接受且显式写入文档。
- 同一轮扫描出现多个新 item → 串行逐个跑，处理期间新 push 的 head 下一轮才可见。
- notify 命令执行失败 → 记日志不中断（通知是 best-effort，评论回写才是主凭据）。

## 6. 测试

- watch.yaml 校验单测：白名单空 / bindings 不命中 / 模板非法各自报可解释错误。
- scan_once 单测：stub gh 二进制返回固定 JSON，覆盖过滤、去重、新 head 复审、白名单拒绝。
- state 文件 round-trip + 原子写 + 坏文件拒启。
- 日预算累计与停摆路径。
- smoke：stub gh + stub claude/codex 全链路 `watch --once`，断言评论回写调用与 state 变更（沿用 demo/ 的 stub 模式）。

## 7. 实施切片

1. watch.yaml 解析 + 校验 + state 文件（纯数据层，无副作用）
2. scan_once + `--once`（stub gh 可全测）：过滤、去重、实例化、headless run、处置回写
3. 常驻循环 + interval + 连续失败退出 + 日预算停摆
4. notify 接入 + README 威胁模型文档
5. （V2 候选，不在本 spec 承诺内）GitLab source、parked-run 恢复、多 watcher
