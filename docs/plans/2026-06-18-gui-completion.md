# GUI 补全 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tauri GUI 补到与 CLI 对齐——GUI 运行落 NDJSON、历史/成本/回看/对比接通,Composer 能编排 verify 校验门(含 by:command)。

**Architecture:** 引擎保持纯净;落盘与审计读在 tauri 消费者侧复用 `agentpipe_engine::audit`;前端 live 与回看共用同一个 `runReducer`。

**Tech Stack:** Rust(engine + tauri)、React 18 + TS(ui,vitest)。

设计依据:`docs/specs/2026-06-18-gui-completion-design.md`。

## Global Constraints

- 引擎纯净:executor 不碰文件系统;落盘/审计读在 `src-tauri`(消费者侧)。
- fail-closed:审计写失败只 eprintln、不打断 run;`view_run`/`diff_runs` 的 run-id 必过 `is_valid_run_id`,非法即 `Err`。
- 单一来源:对比/聚合在 engine `audit`(CLI 与 GUI 共用,不得各写);事件→状态用同一前端 `runReducer`(live 与回看共用)。
- 类型镜像:`ui/src/types.ts` 的 Verify/Event/StepMetrics 与 `manifest.rs`/`protocol.rs` 同步(改一边同步另一边的注释指明处)。
- run-id 格式/allowlist 沿用 `audit`(秒级时间戳-slug,`[A-Za-z0-9_-]`,同秒撞名退避)。
- 验证门:`cargo build && cargo test` 全绿 + `cd ui && npm run build && npm test` 全绿,才算完工。
- 提交信息用中文。

---

### Task 1: engine audit 下沉 step_finals/run_summary + Serialize

**Files:**
- Modify: `crates/engine/src/audit.rs`(加 `StepFinal`/`step_finals`/`RunSummaryCore`/`run_summary`;`RunEntry`/`CostSummary` 加 `Serialize`)
- Modify: `crates/cli/src/commands.rs`(`diff` 改调 `audit::step_finals`,删本地 `finals`)
- Test: `crates/engine/src/audit.rs` 内 `mod tests`

**Interfaces:**
- Produces:
  - `#[derive(Clone, Serialize)] pub struct StepFinal { pub status: String, pub cost_usd: f64 }`
  - `pub fn step_finals(entries: &[RunEntry]) -> std::collections::BTreeMap<String, StepFinal>`
  - `#[derive(Clone, Serialize, PartialEq, Debug)] pub struct RunSummaryCore { pub name: String, pub status: Option<String>, pub total_cost_usd: f64, pub total_turns: u32, pub step_count: usize, pub complete: bool }`
  - `pub fn run_summary(entries: &[RunEntry]) -> RunSummaryCore`
  - `RunEntry`、`CostSummary` 现 `Serialize`
- Consumes:`Event`、`StepMetrics`、`aggregate_cost`(已有)。

- [ ] **Step 1: 写失败测试**

在 `crates/engine/src/audit.rs` 的 `mod tests` 追加:

```rust
    use crate::protocol::{RunStatus, StepStatus};

    fn ev_finished(id: &str, cost: f64) -> Event {
        Event::StepFinished {
            step_id: id.into(), status: StepStatus::Done, summary: "".into(),
            metrics: Some(StepMetrics { num_turns: 1, duration_ms: 1000, cost_usd: cost }),
        }
    }

    #[test]
    fn step_finals_extracts_status_and_cost() {
        let entries = vec![
            RunEntry { ts: "".into(), event: Event::RunStarted { name: "x".into() } },
            RunEntry { ts: "".into(), event: ev_finished("a", 0.5) },
        ];
        let f = step_finals(&entries);
        assert_eq!(f.len(), 1);
        assert_eq!(f["a"].status, "Done");
        assert!((f["a"].cost_usd - 0.5).abs() < 1e-9);
    }

    #[test]
    fn run_summary_aggregates_name_status_cost_complete() {
        let entries = vec![
            RunEntry { ts: "".into(), event: Event::RunStarted { name: "demo".into() } },
            RunEntry { ts: "".into(), event: ev_finished("a", 0.3) },
            RunEntry { ts: "".into(), event: ev_finished("b", 0.7) },
            RunEntry { ts: "".into(), event: Event::RunFinished { status: RunStatus::Success } },
        ];
        let s = run_summary(&entries);
        assert_eq!(s.name, "demo");
        assert_eq!(s.status.as_deref(), Some("Success"));
        assert_eq!(s.step_count, 2);
        assert!(s.complete);
        assert!((s.total_cost_usd - 1.0).abs() < 1e-9);
    }

    #[test]
    fn run_summary_incomplete_when_no_runfinished() {
        let entries = vec![
            RunEntry { ts: "".into(), event: Event::RunStarted { name: "x".into() } },
            RunEntry { ts: "".into(), event: ev_finished("a", 0.1) },
        ];
        let s = run_summary(&entries);
        assert!(!s.complete);
        assert_eq!(s.status, None);
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p agentpipe-engine step_finals run_summary`
Expected: 编译失败(`step_finals`/`run_summary`/`StepFinal`/`RunSummaryCore` 未定义)。

- [ ] **Step 3: 实现**

`RunEntry` 派生改:`#[derive(Debug, Clone, serde::Serialize)]`。`CostSummary` 派生改:`#[derive(Debug, Default, PartialEq, serde::Serialize)]`。

在 `aggregate_cost` 之后加:

```rust
/// 一步的终态(status 文本 + cost),供 diff 对比。
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct StepFinal {
    pub status: String,
    pub cost_usd: f64,
}

/// 按 step_id 提取每步终态。供 CLI diff 与 GUI diff_runs 共用(单一来源)。
pub fn step_finals(entries: &[RunEntry]) -> std::collections::BTreeMap<String, StepFinal> {
    let mut m = std::collections::BTreeMap::new();
    for e in entries {
        if let Event::StepFinished { step_id, status, metrics, .. } = &e.event {
            let cost_usd = metrics.as_ref().map(|x| x.cost_usd).unwrap_or(0.0);
            m.insert(step_id.clone(), StepFinal { status: format!("{status:?}"), cost_usd });
        }
    }
    m
}

/// 一次 run 的摘要(列表/卡片用)。name 取首个 RunStarted;status/complete 取末 RunFinished。
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct RunSummaryCore {
    pub name: String,
    pub status: Option<String>,
    pub total_cost_usd: f64,
    pub total_turns: u32,
    pub step_count: usize,
    pub complete: bool,
}

pub fn run_summary(entries: &[RunEntry]) -> RunSummaryCore {
    let cost = aggregate_cost(entries);
    let name = entries
        .iter()
        .find_map(|e| match &e.event {
            Event::RunStarted { name } => Some(name.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let status = entries.iter().rev().find_map(|e| match &e.event {
        Event::RunFinished { status } => Some(format!("{status:?}")),
        _ => None,
    });
    let complete = status.is_some();
    RunSummaryCore {
        name,
        status,
        total_cost_usd: cost.total_cost_usd,
        total_turns: cost.total_turns,
        step_count: cost.steps.len(),
        complete,
    }
}
```

- [ ] **Step 4: CLI diff 改调引擎版(去重)**

`crates/cli/src/commands.rs`:删除本地 `fn step_finals(...)`,`diff` 改用 `agentpipe_engine::audit::step_finals`。注意返回类型从 `(String,f64)` 元组变成 `StepFinal{status,cost_usd}`,`diff` 里的比较与打印同步改:`x.status`/`x.cost_usd`(原 `x.0`/`x.1`),且 `StepFinal` 需要 `PartialEq` 已派生可直接 `x != y`。`use agentpipe_engine::audit::{... , step_finals, StepFinal};`(StepFinal 仅类型标注需要时引)。

改后 `diff` 的对比片段:

```rust
let (fa, fb) = (step_finals(&load_run(a)), step_finals(&load_run(b)));
// keys 合并去重不变
match (fa.get(k), fb.get(k)) {
    (Some(x), None) => println!("  - {k}: 仅 A ({})", x.status),
    (None, Some(y)) => println!("  + {k}: 仅 B ({})", y.status),
    (Some(x), Some(y)) if x != y => {
        println!("  ~ {k}: {} ${:.2} → {} ${:.2}", x.status, x.cost_usd, y.status, y.cost_usd);
    }
    _ => {}
}
```

- [ ] **Step 5: 测试 + 构建**

Run: `cargo test -p agentpipe-engine && cargo build -p agentpipe-cli`
Expected: engine 全绿(含 3 新测试),cli 编译过。CLI diff 行为不变(手动可跑上一个 spec 的 diff 烟测确认)。

- [ ] **Step 6: 提交**

```bash
git add crates/engine/src/audit.rs crates/cli/src/commands.rs
git commit -m "feat(engine): audit 加 step_finals/run_summary + Serialize;CLI diff 复用"
```

---

### Task 2: tauri paths + bridge 落盘 + run-id 事件

**Files:**
- Create: `src-tauri/src/paths.rs`(runs_dir)
- Modify: `src-tauri/src/main.rs`(`mod paths;`)
- Modify: `src-tauri/src/bridge.rs`(转发线程接 RunRecorder + emit run-id)

**Interfaces:**
- Produces:`pub fn paths::runs_dir() -> std::path::PathBuf`;新 Tauri 事件 `engine://run-started-id`(payload `{ run_id: String }`)。
- Consumes:`agentpipe_engine::audit::RunRecorder`。

- [ ] **Step 1: 写 runs_dir + 测试**

新建 `src-tauri/src/paths.rs`:

```rust
use std::path::PathBuf;

/// ~/.agentpipe/runs(AGENTPIPE_HOME 优先)。与 CLI runs_dir 同义。
pub fn runs_dir() -> PathBuf {
    let base = std::env::var("AGENTPIPE_HOME")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".agentpipe").join("runs")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runs_dir_uses_agentpipe_home() {
        // 仅验证拼接结构(不改全局 env,避免与其他测试竞争):末两段固定
        let d = runs_dir();
        assert!(d.ends_with("runs"));
        assert!(d.to_string_lossy().contains(".agentpipe"));
    }
}
```

`src-tauri/src/main.rs` 顶部加 `mod paths;`。

- [ ] **Step 2: 运行测试确认通过(纯新增)**

Run: `cargo test -p agentpipe-tauri paths`
Expected: PASS。

- [ ] **Step 3: bridge 转发线程接 RunRecorder + emit run-id**

`src-tauri/src/bridge.rs` 的转发线程改造(在 `for evt in event_rx` 前后):

```rust
use agentpipe_engine::audit::RunRecorder;

// 转发线程:引擎事件 → webview(+ 落 NDJSON,审计旁路)
let forward_app = app.clone();
thread::spawn(move || {
    let mut saw_finished = false;
    let mut recorder: Option<RunRecorder> = None;
    for evt in event_rx {
        if matches!(evt, Event::RunStarted { .. }) {
            recorder = RunRecorder::open(&crate::paths::runs_dir(), run_name(&evt))
                .map_err(|e| eprintln!("(GUI 审计未启用: {e})"))
                .ok();
            if let Some(r) = &recorder {
                let _ = forward_app.emit("engine://run-started-id", RunIdPayload { run_id: r.run_id().to_string() });
            }
        }
        if let Some(r) = &mut recorder {
            r.record(&evt);
        }
        if matches!(evt, Event::RunFinished { .. }) {
            saw_finished = true;
        }
        let _ = forward_app.emit(EVENT_CHANNEL, evt);
    }
    // ... 既有兜底(合成 RunFinished + 清 active)不变 ...
});
```

辅助(放 bridge.rs):

```rust
#[derive(Clone, serde::Serialize)]
struct RunIdPayload { run_id: String }

fn run_name(evt: &Event) -> &str {
    match evt {
        Event::RunStarted { name } => name,
        _ => "run",
    }
}
```

> 注意:`evt` 在 record 与 emit 都要用 → record 接 `&evt`,emit 用 `evt`(move)放最后;`RunStarted` 分支里用 `run_name(&evt)` 取 name,不要在 emit 前 move。

- [ ] **Step 4: 构建 + smoke(真实落盘)**

Run: `cargo build -p agentpipe-tauri`
Expected: 编译通过。bridge 落盘的端到端验收在 Task 7 手动烟测覆盖(需 webview);此处仅保证编译 + 现有 tauri 测试不破。

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/paths.rs src-tauri/src/main.rs src-tauri/src/bridge.rs
git commit -m "feat(tauri): GUI 运行落 NDJSON(bridge 接 RunRecorder)+ 广播 run-id"
```

---

### Task 3: tauri 审计读命令

**Files:**
- Modify: `src-tauri/src/commands.rs`(list_runs / view_run / diff_runs + DTO)
- Modify: `src-tauri/src/main.rs`(注册三命令)
- Test: `src-tauri/src/commands.rs` 内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes:`audit::{read_run, run_summary, step_finals, is_valid_run_id, RunEntry}`、`paths::runs_dir`。
- Produces:命令 `list_runs() -> Result<Vec<RunSummary>,String>`、`view_run(run_id) -> Result<Vec<Event>,String>`、`diff_runs(a,b) -> Result<Vec<DiffRow>,String>`;DTO `RunSummary`(含 run_id)、`DiffRow`。把可测逻辑抽成纯函数 `collect_runs(dir)`、`build_diff(a_entries,b_entries)`,command 只薄包装。

- [ ] **Step 1: 写失败测试**

在 `src-tauri/src/commands.rs` 末尾:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use agentpipe_engine::audit::RunRecorder;
    use agentpipe_engine::protocol::{Event, RunStatus, StepStatus, StepMetrics};
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aptauri-{}-{tag}", std::process::id()))
    }
    fn write_run(dir: &std::path::Path, name: &str, cost: f64, finish: bool) -> String {
        let mut r = RunRecorder::open(dir, name).unwrap();
        r.record(&Event::RunStarted { name: name.into() });
        r.record(&Event::StepFinished {
            step_id: "a".into(), status: StepStatus::Done, summary: "".into(),
            metrics: Some(StepMetrics { num_turns: 1, duration_ms: 1000, cost_usd: cost }),
        });
        if finish { r.record(&Event::RunFinished { status: RunStatus::Success }); }
        r.run_id().to_string()
    }

    #[test]
    fn collect_runs_summarizes_and_sorts_desc() {
        let dir = tmp("list");
        let _ = write_run(&dir, "one", 0.10, true);
        let _ = write_run(&dir, "two", 0.20, true);
        let runs = collect_runs(&dir).unwrap();
        assert_eq!(runs.len(), 2);
        // 倒序:run-id 时间戳前缀大的在前(两次同名不同 → 退避序号;断言总成本可读)
        assert!(runs.iter().all(|r| r.total_cost_usd > 0.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_diff_buckets_only_and_changed() {
        let dir = tmp("diff");
        let a = write_run(&dir, "a", 0.10, true);
        let b = write_run(&dir, "b", 0.99, true);
        let ea = agentpipe_engine::audit::read_run(&dir.join(format!("{a}.ndjson"))).unwrap();
        let eb = agentpipe_engine::audit::read_run(&dir.join(format!("{b}.ndjson"))).unwrap();
        let rows = build_diff(&ea, &eb);
        // 同一 step "a" 成本不同 → changed
        assert!(rows.iter().any(|r| r.step_id == "a" && r.kind == "changed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn view_run_rejects_traversal() {
        assert!(view_run_impl("../etc/passwd").is_err());
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p agentpipe-tauri commands`
Expected: 编译失败(`collect_runs`/`build_diff`/`view_run_impl`/DTO 未定义)。

- [ ] **Step 3: 实现 DTO + 纯函数 + 命令**

在 `commands.rs` 加:

```rust
use agentpipe_engine::audit::{self, RunEntry};
use agentpipe_engine::protocol::Event;

#[derive(serde::Serialize)]
pub struct RunSummary {
    pub run_id: String,
    pub name: String,
    pub status: Option<String>,
    pub total_cost_usd: f64,
    pub total_turns: u32,
    pub step_count: usize,
    pub complete: bool,
}

#[derive(serde::Serialize)]
pub struct DiffRow {
    pub step_id: String,
    pub kind: String, // only_a | only_b | changed
    pub a_status: Option<String>,
    pub a_cost: Option<f64>,
    pub b_status: Option<String>,
    pub b_cost: Option<f64>,
}

fn collect_runs(dir: &std::path::Path) -> Result<Vec<RunSummary>, String> {
    let mut ids: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".ndjson").map(str::to_string))
            .filter(|id| audit::is_valid_run_id(id))
            .collect(),
        Err(_) => return Ok(vec![]), // 目录不存在 = 无历史
    };
    ids.sort();
    ids.reverse();
    let mut out = Vec::new();
    for id in ids {
        let path = dir.join(format!("{id}.ndjson"));
        let entries = audit::read_run(&path).map_err(|e| e.to_string())?;
        let s = audit::run_summary(&entries);
        out.push(RunSummary {
            run_id: id, name: s.name, status: s.status,
            total_cost_usd: s.total_cost_usd, total_turns: s.total_turns,
            step_count: s.step_count, complete: s.complete,
        });
    }
    Ok(out)
}

fn build_diff(a: &[RunEntry], b: &[RunEntry]) -> Vec<DiffRow> {
    let (fa, fb) = (audit::step_finals(a), audit::step_finals(b));
    let mut keys: Vec<&String> = fa.keys().chain(fb.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut rows = Vec::new();
    for k in keys {
        match (fa.get(k), fb.get(k)) {
            (Some(x), None) => rows.push(DiffRow {
                step_id: k.clone(), kind: "only_a".into(),
                a_status: Some(x.status.clone()), a_cost: Some(x.cost_usd), b_status: None, b_cost: None,
            }),
            (None, Some(y)) => rows.push(DiffRow {
                step_id: k.clone(), kind: "only_b".into(),
                a_status: None, a_cost: None, b_status: Some(y.status.clone()), b_cost: Some(y.cost_usd),
            }),
            (Some(x), Some(y)) if x != y => rows.push(DiffRow {
                step_id: k.clone(), kind: "changed".into(),
                a_status: Some(x.status.clone()), a_cost: Some(x.cost_usd),
                b_status: Some(y.status.clone()), b_cost: Some(y.cost_usd),
            }),
            _ => {}
        }
    }
    rows
}

fn run_path_checked(run_id: &str) -> Result<std::path::PathBuf, String> {
    if !audit::is_valid_run_id(run_id) {
        return Err(format!("非法 run-id: {run_id}"));
    }
    Ok(crate::paths::runs_dir().join(format!("{run_id}.ndjson")))
}

fn view_run_impl(run_id: &str) -> Result<Vec<Event>, String> {
    let path = run_path_checked(run_id)?;
    let entries = audit::read_run(&path).map_err(|e| e.to_string())?;
    Ok(entries.into_iter().map(|e| e.event).collect())
}

#[tauri::command]
pub fn list_runs() -> Result<Vec<RunSummary>, String> {
    collect_runs(&crate::paths::runs_dir())
}

#[tauri::command]
pub fn view_run(run_id: String) -> Result<Vec<Event>, String> {
    view_run_impl(&run_id)
}

#[tauri::command]
pub fn diff_runs(a: String, b: String) -> Result<Vec<DiffRow>, String> {
    let pa = run_path_checked(&a)?;
    let pb = run_path_checked(&b)?;
    let ea = audit::read_run(&pa).map_err(|e| e.to_string())?;
    let eb = audit::read_run(&pb).map_err(|e| e.to_string())?;
    Ok(build_diff(&ea, &eb))
}
```

`Event` 已 `Serialize`(protocol.rs),可直接作命令返回。

- [ ] **Step 4: 注册命令**

`src-tauri/src/main.rs` 的 `invoke_handler` 追加 `commands::list_runs, commands::view_run, commands::diff_runs`。

- [ ] **Step 5: 测试 + 构建**

Run: `cargo test -p agentpipe-tauri && cargo build -p agentpipe-tauri`
Expected: 全绿(含 3 新测试)。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/commands.rs src-tauri/src/main.rs
git commit -m "feat(tauri): 审计读命令 list_runs/view_run/diff_runs(复用 engine audit)"
```

---

### Task 4: 前端类型 + ipc + useHistory

**Files:**
- Modify: `ui/src/types.ts`(RunSummary / DiffRow + Verify 的 command)
- Modify: `ui/src/ipc.ts`(listRuns/viewRun/diffRuns/onRunStartedId)
- Create: `ui/src/state/useHistory.ts`
- Test: `ui/src/state/useHistory.test.ts`

**Interfaces:**
- Produces:`ipc.listRuns/viewRun/diffRuns/onRunStartedId`;`useHistory()` 返回 `{ summaries, refresh, openState(run_id) }`;`replayToState(events) -> RunState`(纯函数,折叠 runReducer)。
- Consumes:`runReducer`、`initialRunState`、`EngineEvent`。

- [ ] **Step 1: 加类型 + ipc(无独立测试,随后用)**

`ui/src/types.ts` 加:

```ts
export type Verify = {
  by: "codex" | "claude" | "command";
  action?: CodexAction;
  base?: string; path?: string; prompt?: string;
  skill?: string;
  command?: string; // command verifier(exit 0 = 达成)
  max_retries?: number;
  on_unmet?: "gate" | "fail" | "continue";
  feedback?: boolean;
};

export type RunSummary = {
  run_id: string; name: string; status: string | null;
  total_cost_usd: number; total_turns: number; step_count: number; complete: boolean;
};
export type DiffRow = {
  step_id: string; kind: "only_a" | "only_b" | "changed";
  a_status: string | null; a_cost: number | null;
  b_status: string | null; b_cost: number | null;
};
```
(替换原 Verify 定义;其余类型不变。)

`ui/src/ipc.ts` 的 `ipc` 对象加:

```ts
  listRuns: () => invoke<RunSummary[]>("list_runs"),
  viewRun: (runId: string) => invoke<EngineEvent[]>("view_run", { runId }),
  diffRuns: (a: string, b: string) => invoke<DiffRow[]>("diff_runs", { a, b }),
  onRunStartedId: (cb: (runId: string) => void): Promise<UnlistenFn> =>
    listen<{ run_id: string }>("engine://run-started-id", (e) => cb(e.payload.run_id)),
```
(import 补 `RunSummary, DiffRow`。注意 tauri invoke 参数名 camelCase 自动映射 snake:`run_id`→`runId`、命令名按注册名。)

- [ ] **Step 2: 写 useHistory + replayToState 失败测试**

`ui/src/state/useHistory.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { replayToState } from "./useHistory";
import type { EngineEvent } from "../types";

describe("replayToState", () => {
  it("折叠事件序列重建终态", () => {
    const events: EngineEvent[] = [
      { type: "RunStarted", name: "demo" },
      { type: "StepStarted", step_id: "a", kind: "claude" },
      { type: "StepFinished", step_id: "a", status: "Done", summary: "ok", metrics: { num_turns: 2, duration_ms: 2000, cost_usd: 0.4 } },
      { type: "RunFinished", status: "Success" },
    ];
    const st = replayToState(events);
    expect(st.name).toBe("demo");
    expect(st.order).toEqual(["a"]);
    expect(st.steps["a"].status).toBe("Done");
    expect(st.runStatus).toBe("Success");
  });

  it("空序列返回初始态", () => {
    expect(replayToState([]).order).toEqual([]);
  });
});
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cd ui && npx vitest run src/state/useHistory.test.ts`
Expected: FAIL(`replayToState` 未定义)。

- [ ] **Step 4: 实现 useHistory**

`ui/src/state/useHistory.ts`:

```ts
import { useCallback, useEffect, useState } from "react";
import { ipc } from "../ipc";
import type { EngineEvent, RunSummary } from "../types";
import { runReducer, initialRunState, type RunState } from "./runReducer";

/** 纯函数:把 view_run 的事件序列折叠成 RunState(与 live 共用 runReducer)。 */
export function replayToState(events: EngineEvent[]): RunState {
  return events.reduce(runReducer, initialRunState());
}

export interface HistoryController {
  summaries: RunSummary[];
  refresh: () => void;
  openState: (runId: string) => Promise<RunState>;
}

export function useHistory(): HistoryController {
  const [summaries, setSummaries] = useState<RunSummary[]>([]);
  const refresh = useCallback(() => {
    ipc.listRuns().then(setSummaries).catch(() => setSummaries([]));
  }, []);
  useEffect(() => { refresh(); }, [refresh]);
  const openState = useCallback(async (runId: string): Promise<RunState> => {
    const events = await ipc.viewRun(runId);
    return replayToState(events);
  }, []);
  return { summaries, refresh, openState };
}
```

- [ ] **Step 5: 运行测试确认通过 + 类型检查**

Run: `cd ui && npx vitest run src/state/useHistory.test.ts && npm run build`
Expected: 测试 PASS;tsc 通过(注意 types.ts 改了 Verify,可能波及 StepDrawer——若 build 报错属 Task 6 范围,本任务只保证新代码类型自洽;如 build 因 Verify 改动报已有文件错,记录并在 Task 6 修)。

> 若 `npm run build` 因 Verify 类型扩展导致既有组件报错,本步改用 `npx vitest run`(只验测试)+ `npx tsc --noEmit src/state/useHistory.ts` 不可行(tsc 全量);故本步以 vitest 绿为准,完整 build 绿留到 Task 6 收口。

- [ ] **Step 6: 提交**

```bash
git add ui/src/types.ts ui/src/ipc.ts ui/src/state/useHistory.ts ui/src/state/useHistory.test.ts
git commit -m "feat(ui): 审计 ipc + useHistory(replayToState 复用 runReducer)"
```

---

### Task 5: 前端 RecordsPanel 历史化 + 只读回看 + DiffView

**Files:**
- Modify: `ui/src/App.tsx`(组合 useRuns + useHistory,RunFinished 后 refresh,选中路由)
- Modify: `ui/src/records/RecordsPanel.tsx`(持久化列表 + 活跃置顶 + 成本 + 选两条 diff)
- Create: `ui/src/records/DiffView.tsx`
- Modify: `ui/src/console/Console.tsx`(只读回看:接受外部 RunState,isLive=false 时不显示 gate/中止)
- Modify: `ui/src/styles.css`(diff/成本/历史项必要样式,用既有 token)

**Interfaces:**
- Consumes:`useHistory`(summaries/refresh/openState)、`useRuns`、`ipc.diffRuns`、`ipc.onRunStartedId`。
- 数据流:App 持有 live(useRuns)与 history(useHistory);选中分两类——活跃 live run 走现有实时路径;持久化 run 经 `openState(run_id)` 得到 RunState 交 Console 只读渲染。RunFinished 后 `history.refresh()`。

实现要点(按既有模式,组件 JSX 参照现有 RecordsPanel/Console):

- App:`const hist = useHistory();` `const runs = useRuns();`。监听 RunFinished(可在 useRuns 暴露一个 onFinished 回调,或 App 内 effect 比对 activeId 由非空变 null 时 `hist.refresh()`)。维护 `selected: { kind:"live", id } | { kind:"history", runId, state } | null`。挂 `ipc.onRunStartedId` 把 run-id 关联到当前 live 记录(存 useRuns 记录上,用于结束后在历史里高亮同一条;拿不到则忽略)。
- RecordsPanel props 改为接收 `summaries: RunSummary[]`、`liveRun: {id,name}|null`、`selectedKey`、`onSelectLive`、`onSelectHistory`、以及 diff 选择态(`compareIds: string[]`、`onToggleCompare`、`onCompare`)。渲染:活跃 live 置顶(运行中点);其下持久化列表(名称/状态点/`r.step_count` 步/`$total_cost_usd`(>0 显示)/未完成标 ⚠)。每条加一个 checkbox 选入对比;选满 2 条显示「对比」按钮 → `onCompare`。
- 去重:live 运行中只显示 live 条;结束后 `refresh` 拉到持久化条,live 条移除(App 在 activeId 变 null 时清除 live selected 或切到对应 history 条)。
- Console:加可选 prop `replayState?: RunState`;当传入(回看)时用它渲染、`isLive=false`、不显示 gate 与中止按钮、底部 prompt 栏照常(快速运行不受影响)。现有 live 路径不变。
- DiffView:接收 `rows: DiffRow[]` + 两个 run 名,渲染分桶(`only_a`→`- 仅A`、`only_b`→`+ 仅B`、`changed`→`~ status $a → status $b`);空 rows 显示「无差异」。以浮层/侧栏呈现(参照 drawer 样式)。
- styles.css:新增 `.diff-row`、成本/步数 meta、对比 checkbox 的最小样式,全部用 `:root` 既有 token(颜色/间距),不硬编码。

- [ ] **Step 1: DiffView + 其纯渲染测试(可选组件测)**

创建 `ui/src/records/DiffView.tsx`(纯展示组件,props 见上)。最小测试 `ui/src/records/DiffView.test.tsx`(用 React Testing Library? 项目未装 RTL → 改为对一个纯函数 `bucketLabel(kind)` 做单测,或跳过 DOM 测,改测 App 的选择 reducer)。**决策:** 不引入 RTL(避免新依赖);DiffView 的可测逻辑抽成纯函数 `diffRowText(row): string` 并单测:

```ts
// DiffView.tsx 导出
export function diffRowText(r: DiffRow): string {
  if (r.kind === "only_a") return `- ${r.step_id}: 仅 A (${r.a_status})`;
  if (r.kind === "only_b") return `+ ${r.step_id}: 仅 B (${r.b_status})`;
  return `~ ${r.step_id}: ${r.a_status} $${(r.a_cost ?? 0).toFixed(2)} → ${r.b_status} $${(r.b_cost ?? 0).toFixed(2)}`;
}
```

`ui/src/records/DiffView.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { diffRowText } from "./DiffView";

describe("diffRowText", () => {
  it("only_a / only_b / changed 三类文案", () => {
    expect(diffRowText({ step_id: "x", kind: "only_a", a_status: "Done", a_cost: 0.1, b_status: null, b_cost: null })).toContain("仅 A");
    expect(diffRowText({ step_id: "y", kind: "only_b", a_status: null, a_cost: null, b_status: "Done", b_cost: 0.2 })).toContain("仅 B");
    expect(diffRowText({ step_id: "z", kind: "changed", a_status: "Done", a_cost: 0.1, b_status: "Failed", b_cost: 0.3 })).toContain("→");
  });
});
```

- [ ] **Step 2: 跑测试确认失败 → 实现 DiffView**

Run: `cd ui && npx vitest run src/records/DiffView.test.ts` → FAIL(未定义)→ 实现 `diffRowText` + DiffView 组件(组件用 `diffRowText` 渲染每行)→ PASS。

- [ ] **Step 3: Console 只读回看 prop**

给 `Console` 加 `replayState?: RunState`;`const state = replayState ?? record?.state ?? null;`;`const live = !replayState && isLive && !!state && !state.runStatus;`。回看时 gate 块(`state.activeGate && live`)自然不显示(live=false),中止按钮(`live &&`)也不显示。其余渲染复用。

- [ ] **Step 4: RecordsPanel 历史化 + App 接线**

按"实现要点"改 RecordsPanel props 与渲染、App 组合 useRuns+useHistory+选择路由+RunFinished 刷新+onRunStartedId 关联+diff 触发。styles.css 加最小样式。

- [ ] **Step 5: 全量前端验证**

Run: `cd ui && npm run build && npm test`
Expected: tsc 通过、所有 vitest 绿(runReducer/useRuns/useHistory/DiffView)。手动启动验收留 Task 7。

- [ ] **Step 6: 提交**

```bash
git add ui/src/App.tsx ui/src/records/ ui/src/console/Console.tsx ui/src/styles.css
git commit -m "feat(ui): RecordsPanel 历史化 + 只读回看 + DiffView + 成本展示"
```

---

### Task 6: Composer 校验门编排(verify 编辑器)

**Files:**
- Modify: `ui/src/composer/StepDrawer.tsx`(claude Fields 加 verify 编辑区)
- Modify: `ui/src/composer/StepCard.tsx`(stepSummary 追加 verify 摘要)
- Test: `ui/src/composer/verify.test.ts`(verify 写回纯函数)

**Interfaces:**
- Produces:`composer/verifyEdit.ts` 纯 helper:`setVerifyEnabled(step, on)`、`patchVerify(step, patch)`、`verifySummary(verify)`(供 StepDrawer 与 StepCard 共用,逻辑可测)。
- Consumes:`Verify`、`Step`(claude)。

- [ ] **Step 1: 写 verify helper 失败测试**

`ui/src/composer/verify.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { setVerifyEnabled, patchVerify, verifySummary } from "./verifyEdit";
import type { Step } from "../types";

const claude = (): Extract<Step, { kind: "claude" }> => ({ id: "s", kind: "claude", prompt: "do" });

describe("verifyEdit", () => {
  it("启用产出默认 verify(by codex),停用则删除", () => {
    const on = setVerifyEnabled(claude(), true);
    expect(on.verify?.by).toBe("codex");
    const off = setVerifyEnabled(on, false);
    expect(off.verify).toBeUndefined();
  });
  it("patchVerify 合并字段", () => {
    const s = setVerifyEnabled(claude(), true);
    const s2 = patchVerify(s, { by: "command", command: "cargo test" });
    expect(s2.verify?.by).toBe("command");
    expect(s2.verify?.command).toBe("cargo test");
  });
  it("verifySummary 反映 by", () => {
    expect(verifySummary({ by: "command", command: "x" })).toContain("command");
    expect(verifySummary(undefined)).toBe("");
  });
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/composer/verify.test.ts` → FAIL。

- [ ] **Step 3: 实现 verifyEdit.ts**

`ui/src/composer/verifyEdit.ts`:

```ts
import type { Step, Verify } from "../types";
type Claude = Extract<Step, { kind: "claude" }>;

const DEFAULT_VERIFY: Verify = { by: "codex", action: "review-mr", base: "dev", max_retries: 2, on_unmet: "gate", feedback: true };

export function setVerifyEnabled(step: Claude, on: boolean): Claude {
  if (on) return { ...step, verify: step.verify ?? { ...DEFAULT_VERIFY } };
  const { verify: _drop, ...rest } = step;
  return rest as Claude;
}

export function patchVerify(step: Claude, patch: Partial<Verify>): Claude {
  const base = step.verify ?? { ...DEFAULT_VERIFY };
  return { ...step, verify: { ...base, ...patch } };
}

export function verifySummary(v: Verify | undefined): string {
  return v ? `+verify:${v.by}` : "";
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/composer/verify.test.ts` → PASS。

- [ ] **Step 5: StepDrawer 接入 verify 编辑 UI(claude case)**

在 `Fields` 的 `case "claude"` 末尾,skill 字段后,加可折叠 verify 区:启用复选(`setVerifyEnabled`)、by 选择(codex/claude/command)、按 by 条件字段(codex: action+base/path/prompt;claude: prompt+skill;command: command)、max_retries(number)、on_unmet(select)、feedback(checkbox)。所有写回走 `patchVerify(step, {...})` 再 `onChange`。仅 claude 步骤显示(引擎 verify 只挂 claude)。command 选了但空 → 行内红字提示。

`StepCard.tsx` 的 `stepSummary` claude 分支追加 `verifySummary(step.verify)`(非空时拼到 tags)。

- [ ] **Step 6: 全量前端验证**

Run: `cd ui && npm run build && npm test`
Expected: tsc 通过(此时 types.ts 的 Verify 扩展已被 StepDrawer 正确消费,Task 4 遗留的 build 问题在此收口)、所有测试绿。

- [ ] **Step 7: 提交**

```bash
git add ui/src/composer/
git commit -m "feat(ui): Composer 编排 verify 校验门(codex/claude/command)"
```

---

### Task 7: 文档 + 全量验证 + 手动启动验收

**Files:**
- Modify: `README.md`(GUI 启动说明)
- Modify: `docs/specs/.../` 不动

- [ ] **Step 1: README 加 GUI 节**

`README.md` 追加:

```markdown
## GUI(Tauri 桌面端)

```bash
cargo tauri dev            # 开发启动(自动起 ui vite + tauri 窗口)
# 或仅前端: cd ui && npm run dev
```

功能:左侧历史记录(持久化于 ~/.agentpipe/runs/,含成本,可两两对比),中间控制台(实时执行 / 历史只读回看 / 底部 prompt 快速运行),右侧编排(可视化建 task.yaml,含 verify 校验门 codex/claude/command)。

stub 演示同 CLI:设 AGENTPIPE_CLAUDE_BIN / AGENTPIPE_CODEX_BIN 指向 tests/fixtures 下的 stub 脚本。
```

- [ ] **Step 2: 全量自动验证(完工门)**

Run:
```bash
cargo build && cargo test
cd ui && npm run build && npm test
```
Expected: workspace cargo 全绿(engine + tauri 新测试)、ui tsc 通过、所有 vitest 绿。逐项记录真实通过状态。

- [ ] **Step 3: 手动启动验收(图形环境,人工)**

记录到验收清单(若当前环境无显示,标注"留人工",不阻塞自动门):
1. `cargo tauri dev` 起窗口。
2. 底部 prompt 用 stub 跑一次 → 控制台实时出步骤 → 结束。
3. 左侧历史出现该 run(名称/成本/状态);重启 app 仍在(持久化验证)。
4. 点开历史条目 → 控制台只读回看,步骤/成本正确,无 gate/中止按钮。
5. 勾选两条 → 对比 → DiffView 出 仅A/仅B/变化。
6. 右侧编排一个 claude 步骤,启用 verify by:command `cargo test`,保存 → 重新加载该 task.yaml 校验门保留。

- [ ] **Step 4: 提交**

```bash
git add README.md
git commit -m "docs: GUI 启动说明 + 验收清单"
```

---

## Self-Review(写完后核对 spec)

- spec A1(bridge 落盘 + run-id 事件)→ Task 2 ✅
- spec A2(list_runs/view_run/diff_runs + step_finals/run_summary 下沉 + Serialize)→ Task 1 + Task 3 ✅
- spec A3(useHistory + RecordsPanel 历史化 + 只读回看 + DiffView + 成本)→ Task 4 + Task 5 ✅
- spec B1(types.ts Verify + command)→ Task 4 Step 1 ✅
- spec B2(StepDrawer verify 编辑 + stepSummary)→ Task 6 ✅
- spec §4 测试(engine 纯函数 / tauri 集成 / 前端 vitest / 构建门)→ 各任务 TDD + Task 7 ✅
- spec §6 不变式(引擎纯净 / fail-closed / 单一来源 / 类型镜像)→ Task 1(单一来源)+ Task 2(纯净)+ Task 3(allowlist)✅
- 类型一致性:`StepFinal`/`RunSummaryCore`(engine) ↔ `RunSummary`/`DiffRow`(tauri DTO) ↔ `RunSummary`/`DiffRow`(ts) 字段对齐;`replayToState`/`useHistory`/`verifyEdit` 跨任务签名一致 ✅
- 已知风险:Task 4 改 types.ts Verify 后,`npm run build` 全量 tsc 可能因 StepDrawer 尚未消费新字段而仅是"未使用"层面(Verify 是放宽不是收紧,既有 codex/claude 仍合法)→ 一般不破坏 build;若破坏,Task 6 收口。Task 5 是 UI 重构,组件 JSX 体量较大,实现者按既有 RecordsPanel/Console/drawer 模式写,逻辑已抽纯函数(replayToState/diffRowText)单测护住。
