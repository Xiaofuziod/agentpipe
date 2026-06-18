# AgentPipe 设计:任务级 git worktree 隔离

工作名 AgentPipe。日期 2026-06-18。状态:待评审。前置:Phase 1a(引擎)+ Phase 1b(GUI),均已落地。

## 1. 背景与目标

当前一个 Run 的所有 step 直接在 `target` 仓库的工作区原地跑(`RunContext.cwd = manifest.target`)。claude 干活步骤以 bypassPermissions 自主 edit + commit,会直接改动用户当前签出的分支/工作区。

目标:给"发布任务"加一个开关 —— 让整个 Run 在一个**隔离的 git worktree**(新分支)里跑,target 主工作区不受影响。跑完保留 worktree,用户可自行 review / 合并 / 丢弃。

非目标:
- 不做自动合并 / 自动删除 worktree(产出即工作,删了就丢)。
- 不做 worktree 复用 / 池化,每个 Run 一个全新 worktree。
- 不在引擎内做 git 鉴权 / 远端推送。

## 2. 设计

### 2.1 配置(单一来源:Manifest)

`Manifest` 增加字段:

```rust
#[serde(default, skip_serializing_if = "is_false")]
pub worktree: bool,
```

- `#[serde(default)]` → 所有现存 YAML / 模板无此字段时解析为 false,向后兼容。
- `skip_serializing_if` → 关闭时保存的 YAML 不出现该字段,模板 diff 干净。
- flag 落在 Manifest 而非 GUI 一次性参数:同时流过 GUI 保存/加载、`start_run`(文件)、`start_run_inline`(对象)、CLI `run`,无需各入口单独传。

### 2.2 引擎:新模块 `crates/engine/src/worktree.rs`

```rust
pub struct Worktree { pub path: PathBuf, pub branch: String }
pub fn create(target: &Path, name: &str) -> Result<Worktree, EngineError>
```

流程:
1. `git -C <target> rev-parse --show-toplevel` 拿仓库根。失败 → Err(非 git 仓库 / git 未装)。
2. 生成唯一后缀 `<epoch_secs>-<pid>`,分支名 `agentpipe/<slug(name)>-<suffix>`。
3. worktree 落 `<repo_root>/../.agentpipe-worktrees/<repo_name>-<suffix>`(同盘、隐藏、与 target 主工作区分离、可发现)。
4. `git -C <repo_root> worktree add -b <branch> <path>`。失败 → Err(透传 git stderr)。

直接用 `std::process::Command` 调 git(一次性捕获,不需要 runner 的流式/进程组)。

### 2.3 执行器接线(executor.rs `run()`)

```
emit RunStarted
if manifest.worktree:
    match worktree::create(target, name):
        Ok(wt)  => ctx.cwd = wt.path; emit WorktreeReady{path,branch}
        Err(e)  => emit WorktreeFailed{error}; emit RunFinished{Failed}; return Failed   // fail-closed
... 原有 step 循环(全部在 ctx.cwd = worktree 里跑)
```

fail-closed 是关键不变式:隔离请求失败绝不退回 target 原地跑。

### 2.4 协议两个新事件(protocol.rs)

```rust
WorktreeReady { path: String, branch: String },
WorktreeFailed { error: String },
```

镜像同步 4 处(改协议必须同动):
- `crates/engine/src/protocol.rs`(Event 枚举,SSOT)
- `crates/cli/src/render.rs`(render_event 加两行)
- `ui/src/types.ts`(EngineEvent union)
- `ui/src/state/runReducer.ts`(归约进 RunState)

审计旁路无需改:RunRecorder 落全部事件,serde tagged 新变体自然序列化,view 回放走 render_event。

### 2.5 GUI

- `Composer.tsx` 任务级设置卡加一个 checkbox:"在隔离 git worktree 中运行(不改动 target 工作区)",绑 `m.worktree`。persist 已 `{...m, target}` 落盘,worktree 自动带上。
- `runReducer` 把 WorktreeReady/Failed 收进 `RunState.worktree` / `worktreeError`。
- `Console.tsx` 顶部渲染一条 worktree 横幅(分支 + 路径,失败显错)。
- 快速运行栏(quickRun)默认 false,不在本次范围。

## 3. 边界与错误路径

| 场景 | 行为 |
|---|---|
| target 非 git 仓库 | WorktreeFailed + Run Failed |
| git 未安装 | WorktreeFailed + Run Failed |
| 分支/路径碰撞 | suffix 含 epoch+pid,近乎不可能;真撞了透传 git 错误 → Failed |
| Run 中途 Abort | worktree 保留(里面可能已有改动) |
| worktree=false(默认) | 行为与现状完全一致,零回归 |

## 4. 测试

- manifest:缺字段默认 false;`worktree: true` 解析为 true。
- worktree::create:临时 git 仓库 → 返回的 path 存在且 `git worktree list` 含之;非 git 目录 → Err。
- render:WorktreeReady / WorktreeFailed 渲染预期行。
- runReducer:WorktreeReady 写入 state.worktree。
