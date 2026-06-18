# 运行持久化与审计 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 每次 run 落一份 `{ts,event}` NDJSON 审计;CLI 升级为 clap 子命令,补 `view / diff / cost / runs / validate` 与 `--dry-run / --json`。

**Architecture:** 引擎保持纯净(仍只 `events.send`);新增 engine `audit` 模块做消费者侧的 `RunRecorder`(落盘)+ NDJSON 读取 / cost 聚合;CLI 先把"事件→字符串"抽成纯 `render_event`(解开展示与 stdin 交互的耦合),只读子命令复用它。

**Tech Stack:** Rust;engine 加 `chrono`;cli 加 `clap`(derive)/ `chrono` / `serde_json`。

设计依据:`docs/specs/2026-06-18-run-persistence-audit-design.md`。

## Global Constraints

- 审计是旁路:`RunRecorder::record` 内部 I/O 错误只 `eprintln!` 告警,绝不冒泡打断 run。
- 每条 record 后立即 flush(防进程崩溃丢尾)。
- run-id = `<UTC %Y%m%dT%H%M%SZ>-<slug(name)>`;外部传入 run-id 必须过 `is_valid_run_id`(只允许 `[A-Za-z0-9_-]`),防路径穿越。
- NDJSON 行 schema 固定 `{"ts": <rfc3339 millis>, "event": <Event 序列化>}`;首行 RunStarted、末行 RunFinished(无末行 = 中断)。
- `--json`:机器数据走 stdout(同上 `{ts,event}` schema),人读走 stderr;非 `--json` 维持现状(人读 stdout)。
- 存储根:`$AGENTPIPE_HOME`(若设)否则 `$HOME`,下接 `.agentpipe/runs/`(unix;项目已 cfg(unix))。
- `Event` 已 `#[derive(Serialize, Deserialize)]`(protocol.rs:30),序列化零成本。
- 提交信息用中文。

---

### Task 1: 抽 render_event 纯函数(前置重构,行为不变)

**Files:**
- Create: `crates/cli/src/render.rs`
- Modify: `crates/cli/src/main.rs`(声明 `mod render`;事件循环改用 `render_event`)
- Test: `crates/cli/src/render.rs` 内 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:`pub fn render_event(event: &Event) -> String`(纯函数,无 I/O / 无 stdin)。
- Consumes:`agentpipe_engine::protocol::{Event, StepStatus}`。

- [ ] **Step 1: 写失败测试**

新建 `crates/cli/src/render.rs`:

```rust
use agentpipe_engine::protocol::{Event, StepStatus};

#[cfg(test)]
mod tests {
    use super::*;
    use agentpipe_engine::protocol::StepMetrics;

    #[test]
    fn renders_step_started() {
        let e = Event::StepStarted { step_id: "impl".into(), kind: "claude".into() };
        assert_eq!(render_event(&e), "  ▷ [claude] impl");
    }

    #[test]
    fn renders_finished_with_metrics() {
        let e = Event::StepFinished {
            step_id: "impl".into(),
            status: StepStatus::Done,
            summary: "done".into(),
            metrics: Some(StepMetrics { num_turns: 7, duration_ms: 41200, cost_usd: 0.83 }),
        };
        assert_eq!(render_event(&e), "  ✓ impl: done · 7 轮 · 41.2s · $0.83");
    }

    #[test]
    fn renders_awaiting_gate_without_prompting() {
        let e = Event::StepAwaitingGate {
            step_id: "plan".into(),
            suggestion: "审批".into(),
            expects_artifact: false,
            gate_kind: agentpipe_engine::protocol::GateKind::Decision,
        };
        assert_eq!(render_event(&e), "  ⏸ plan: 审批");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

先在 `crates/cli/src/main.rs` 顶部加 `mod render;`(否则测试不被编译)。
Run: `cargo test -p agentpipe-cli render`
Expected: 编译失败 —— `render_event` 未定义。

- [ ] **Step 3: 实现 render_event**

在 `render.rs` 的 `use` 之后(`#[cfg(test)]` 之前)加:

```rust
/// 事件 → 人读一行。纯函数:无任何 I/O / stdin,view / dry-run / run 共用。
pub fn render_event(event: &Event) -> String {
    match event {
        Event::RunStarted { name } => format!("▶ Run: {name}"),
        Event::StepStarted { step_id, kind } => format!("  ▷ [{kind}] {step_id}"),
        Event::StepProgress { line, .. } => format!("    {line}"),
        Event::StepFinished { step_id, status, summary, metrics } => {
            let mark = if matches!(status, StepStatus::Skipped) { "⏭" } else { "✓" };
            let m = metrics
                .as_ref()
                .map(|m| format!(
                    " · {} 轮 · {:.1}s · ${:.2}",
                    m.num_turns,
                    m.duration_ms as f64 / 1000.0,
                    m.cost_usd
                ))
                .unwrap_or_default();
            format!("  {mark} {step_id}: {summary}{m}")
        }
        Event::StepFailed { step_id, error } => format!("  ✗ {step_id}: {error}"),
        Event::LoopIteration { loop_id, iteration } => format!("  ↻ {loop_id} 第 {iteration} 轮"),
        Event::LoopConverged { loop_id, iterations } => format!("  ✓ {loop_id} {iterations} 轮收敛"),
        Event::LoopMaxReached { loop_id, max } => format!("  ⚠ {loop_id} 到上限 {max} 仍未干净"),
        Event::StepAwaitingGate { step_id, suggestion, .. } => format!("  ⏸ {step_id}: {suggestion}"),
        Event::RunFinished { status } => format!("■ 结束: {status:?}"),
    }
}
```

- [ ] **Step 4: 重构 main.rs 事件循环用 render_event(行为不变)**

把 `main.rs` 现有 `for event in erx { match event { ... } }` 整段替换为:

```rust
    for event in erx {
        println!("{}", render::render_event(&event));
        match &event {
            Event::StepAwaitingGate { step_id, expects_artifact, .. } => {
                let cmd = prompt_gate(step_id, *expects_artifact);
                let _ = ctx.send(cmd);
            }
            Event::RunFinished { .. } => break,
            _ => {}
        }
    }
```

清理 `main.rs` 顶部不再直接用到的 import(`StepStatus` 移到 render.rs;保留 `Event`、`Command`)。`prompt_gate` 函数保持不变。

- [ ] **Step 5: 运行测试 + 编译**

Run: `cargo test -p agentpipe-cli render && cargo build`
Expected: 3 个测试 PASS,编译通过。

- [ ] **Step 6: 提交**

```bash
git add crates/cli/src/render.rs crates/cli/src/main.rs
git commit -m "refactor(cli): 抽 render_event 纯函数,解开展示与交互耦合"
```

---

### Task 2: engine chrono 依赖 + run-id 生成 / 校验

**Files:**
- Modify: `crates/engine/Cargo.toml`(加 chrono)
- Create: `crates/engine/src/audit.rs`(run_id / slug / is_valid_run_id)
- Modify: `crates/engine/src/lib.rs`(`pub mod audit;`)
- Test: `crates/engine/src/audit.rs` 内 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:`pub fn run_id(name: &str, started: chrono::DateTime<chrono::Utc>) -> String`;`pub fn is_valid_run_id(id: &str) -> bool`。
- Consumes:`chrono`。

- [ ] **Step 1: 加 chrono 依赖**

`crates/engine/Cargo.toml` 的 `[dependencies]` 加:

```toml
chrono = "0.4"
```

- [ ] **Step 2: 写失败测试(新建 audit.rs)**

新建 `crates/engine/src/audit.rs`:

```rust
use chrono::{DateTime, Utc};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn run_id_combines_timestamp_and_slug() {
        let t = Utc.with_ymd_and_hms(2026, 6, 18, 14, 22, 33).unwrap();
        assert_eq!(run_id("Add Verify Gate!", t), "20260618T142233Z-add-verify-gate");
    }

    #[test]
    fn run_id_is_always_valid() {
        let t = Utc.with_ymd_and_hms(2026, 6, 18, 0, 0, 0).unwrap();
        assert!(is_valid_run_id(&run_id("名字 with 中文 & symbols///", t)));
    }

    #[test]
    fn allowlist_rejects_path_traversal() {
        assert!(!is_valid_run_id("../etc/passwd"));
        assert!(!is_valid_run_id("a/b"));
        assert!(!is_valid_run_id(""));
        assert!(is_valid_run_id("20260618T142233Z-ok_id-1"));
    }
}
```

- [ ] **Step 3: 在 lib.rs 注册模块**

`crates/engine/src/lib.rs` 加一行(按字母序):

```rust
pub mod audit;
```

- [ ] **Step 4: 运行测试确认失败**

Run: `cargo test -p agentpipe-engine run_id`
Expected: 编译失败 —— `run_id` / `is_valid_run_id` / `slug` 未定义。

- [ ] **Step 5: 实现 run_id / slug / is_valid_run_id**

在 `audit.rs` 的 `use` 之后(`#[cfg(test)]` 之前)加:

```rust
/// run-id = <UTC 紧凑时间戳>-<slug(name)>;构造即保证只含 [A-Za-z0-9_-]。
pub fn run_id(name: &str, started: DateTime<Utc>) -> String {
    let ts = started.format("%Y%m%dT%H%M%SZ");
    let s = slug(name);
    if s.is_empty() {
        ts.to_string()
    } else {
        format!("{ts}-{s}")
    }
}

/// name → 小写、非字母数字折成单个 `-`、去首尾 `-`、截断 ≤40 字符(截断后再去尾 `-`)。
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let mut s: String = out.trim_matches('-').chars().take(40).collect();
    while s.ends_with('-') {
        s.pop();
    }
    s
}

/// 校验外部传入的 run-id:只允许 [A-Za-z0-9_-],非空。防路径穿越。
pub fn is_valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test -p agentpipe-engine -- run_id allowlist`
Expected: PASS(3 个测试)。

- [ ] **Step 7: 提交**

```bash
git add crates/engine/Cargo.toml crates/engine/src/audit.rs crates/engine/src/lib.rs
git commit -m "feat(engine): audit 模块 run-id 生成 + allowlist 校验"
```

---

### Task 3: RunRecorder 落盘 + event_json_line

**Files:**
- Modify: `crates/engine/src/audit.rs`(RunRecorder + event_json_line)
- Test: `crates/engine/src/audit.rs` 内 `mod tests`

**Interfaces:**
- Consumes:`crate::protocol::Event`;`run_id`(Task 2);`serde_json`(engine 已依赖)。
- Produces:`pub fn event_json_line(event: &Event) -> String`;`pub struct RunRecorder` 带 `open(run_dir: &Path, name: &str) -> std::io::Result<Self>` / `record(&mut self, event: &Event)` / `run_id(&self) -> &str` / `path(&self) -> &Path`。

- [ ] **Step 1: 写失败测试**

在 `audit.rs` 的 `mod tests` 内追加:

```rust
    use crate::protocol::{Event, RunStatus};
    use std::path::PathBuf;

    fn unique_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agentpipe-test-{}-{tag}", std::process::id()))
    }

    #[test]
    fn recorder_writes_ndjson_lines() {
        let dir = unique_dir("rec");
        let mut r = RunRecorder::open(&dir, "demo run").unwrap();
        r.record(&Event::RunStarted { name: "demo run".into() });
        r.record(&Event::RunFinished { status: RunStatus::Success });
        let path = r.path().to_path_buf();
        drop(r);

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert!(first.get("ts").is_some());
        assert_eq!(first["event"]["type"], "RunStarted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn event_json_line_has_ts_and_event() {
        let line = event_json_line(&Event::StepFailed { step_id: "x".into(), error: "boom".into() });
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["event"]["type"], "StepFailed");
        assert_eq!(v["event"]["error"], "boom");
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p agentpipe-engine recorder`
Expected: 编译失败 —— `RunRecorder` / `event_json_line` 未定义。

- [ ] **Step 3: 实现 event_json_line + RunRecorder**

在 `audit.rs`(`is_valid_run_id` 之后、`#[cfg(test)]` 之前)加:

```rust
use crate::protocol::Event;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// 一行审计:{"ts": <rfc3339 millis>, "event": <Event>}。落盘与 --json stdout 共用。
pub fn event_json_line(event: &Event) -> String {
    serde_json::json!({
        "ts": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "event": event,
    })
    .to_string()
}

/// 把一次 run 的事件追加进 ~/.agentpipe/runs/<run-id>.ndjson。
/// 审计是旁路:record 内部 I/O 错误只告警,不冒泡。
pub struct RunRecorder {
    writer: BufWriter<File>,
    run_id: String,
    path: PathBuf,
}

impl RunRecorder {
    /// RunStarted 时创建;run_dir 不存在则建。失败 → 调用方降级为不落盘。
    pub fn open(run_dir: &Path, name: &str) -> std::io::Result<Self> {
        fs::create_dir_all(run_dir)?;
        let id = run_id(name, Utc::now());
        let path = run_dir.join(format!("{id}.ndjson"));
        let file = File::create(&path)?;
        Ok(Self { writer: BufWriter::new(file), run_id: id, path })
    }

    /// 追加一行并立即 flush(BufWriter 默认缓冲会在崩溃时丢尾 —— 那几行恰是排错最需要的)。
    pub fn record(&mut self, event: &Event) {
        let line = event_json_line(event);
        if let Err(e) = writeln!(self.writer, "{line}").and_then(|_| self.writer.flush()) {
            eprintln!("(审计写入失败,已忽略: {e})");
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p agentpipe-engine -- recorder event_json_line`
Expected: PASS(2 个测试)。

- [ ] **Step 5: 提交**

```bash
git add crates/engine/src/audit.rs
git commit -m "feat(engine): RunRecorder 落盘 NDJSON(每条 flush)+ event_json_line"
```

---

### Task 4: NDJSON 读取 + cost 聚合

**Files:**
- Modify: `crates/engine/src/audit.rs`(RunEntry / read_run / CostSummary / aggregate_cost)
- Test: `crates/engine/src/audit.rs` 内 `mod tests`

**Interfaces:**
- Consumes:`Event`、`StepMetrics`(protocol)。
- Produces:`pub struct RunEntry { pub ts: String, pub event: Event }`;`pub fn read_run(path: &Path) -> std::io::Result<Vec<RunEntry>>`;`pub struct CostSummary { pub steps: Vec<(String, StepMetrics)>, pub total_cost_usd: f64, pub total_turns: u32, pub total_duration_ms: u64 }`;`pub fn aggregate_cost(entries: &[RunEntry]) -> CostSummary`。

- [ ] **Step 1: 写失败测试**

在 `mod tests` 内追加:

```rust
    use crate::protocol::{StepMetrics, StepStatus};

    #[test]
    fn read_run_roundtrips_recorder_output() {
        let dir = unique_dir("read");
        let mut r = RunRecorder::open(&dir, "x").unwrap();
        r.record(&Event::RunStarted { name: "x".into() });
        r.record(&Event::StepFinished {
            step_id: "impl".into(),
            status: StepStatus::Done,
            summary: "ok".into(),
            metrics: Some(StepMetrics { num_turns: 3, duration_ms: 1000, cost_usd: 0.5 }),
        });
        let path = r.path().to_path_buf();
        drop(r);

        let entries = read_run(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].event, Event::RunStarted { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn aggregate_cost_sums_step_metrics() {
        let entries = vec![
            RunEntry { ts: "".into(), event: Event::RunStarted { name: "x".into() } },
            RunEntry { ts: "".into(), event: Event::StepFinished {
                step_id: "a".into(), status: StepStatus::Done, summary: "".into(),
                metrics: Some(StepMetrics { num_turns: 2, duration_ms: 1000, cost_usd: 0.30 }),
            }},
            RunEntry { ts: "".into(), event: Event::StepFinished {
                step_id: "b".into(), status: StepStatus::Done, summary: "".into(),
                metrics: Some(StepMetrics { num_turns: 5, duration_ms: 2000, cost_usd: 0.70 }),
            }},
        ];
        let s = aggregate_cost(&entries);
        assert_eq!(s.steps.len(), 2);
        assert_eq!(s.total_turns, 7);
        assert_eq!(s.total_duration_ms, 3000);
        assert!((s.total_cost_usd - 1.0).abs() < 1e-9);
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p agentpipe-engine -- read_run aggregate_cost`
Expected: 编译失败 —— `read_run` / `RunEntry` / `aggregate_cost` / `CostSummary` 未定义。

- [ ] **Step 3: 实现 read_run + aggregate_cost**

在 `audit.rs`(RunRecorder 之后、`#[cfg(test)]` 之前)加。先在文件顶部 `use crate::protocol::Event;` 那行扩成 `use crate::protocol::{Event, StepMetrics};`,然后:

```rust
/// 一条审计记录:落盘时刻 + 反序列化出的事件。
#[derive(Debug, Clone)]
pub struct RunEntry {
    pub ts: String,
    pub event: Event,
}

/// 读 NDJSON;跳过空行与解析失败的行(容损,不让单行坏数据废掉整次回放)。
pub fn read_run(path: &Path) -> std::io::Result<Vec<RunEntry>> {
    let text = fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let ts = v.get("ts").and_then(|t| t.as_str()).unwrap_or("").to_string();
        if let Some(ev) = v.get("event") {
            if let Ok(event) = serde_json::from_value::<Event>(ev.clone()) {
                out.push(RunEntry { ts, event });
            }
        }
    }
    Ok(out)
}

/// 一次 run 的成本/耗时聚合。
#[derive(Debug, Default, PartialEq)]
pub struct CostSummary {
    pub steps: Vec<(String, StepMetrics)>,
    pub total_cost_usd: f64,
    pub total_turns: u32,
    pub total_duration_ms: u64,
}

pub fn aggregate_cost(entries: &[RunEntry]) -> CostSummary {
    let mut s = CostSummary::default();
    for e in entries {
        if let Event::StepFinished { step_id, metrics: Some(m), .. } = &e.event {
            s.total_cost_usd += m.cost_usd;
            s.total_turns += m.num_turns;
            s.total_duration_ms += m.duration_ms;
            s.steps.push((step_id.clone(), m.clone()));
        }
    }
    s
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p agentpipe-engine`
Expected: engine 全部 PASS(含 audit 新测试)。

- [ ] **Step 5: 提交**

```bash
git add crates/engine/src/audit.rs
git commit -m "feat(engine): NDJSON 读取 + cost 聚合"
```

---

### Task 5: CLI clap 化 + run(落盘 / --json / --dry-run)+ validate

**Files:**
- Modify: `crates/cli/Cargo.toml`(加 clap / chrono / serde_json)
- Modify: `crates/cli/src/main.rs`(clap CLI 定义 + run / validate 子命令 + runs_dir 工具)

**Interfaces:**
- Consumes:`render_event`(Task 1);`audit::{RunRecorder, event_json_line}`(Task 3);`Manifest`、`Executor`。
- Produces:`fn runs_dir() -> std::path::PathBuf`(`view`/`cost`/`runs`/`diff` 复用)。

- [ ] **Step 1: 加依赖**

`crates/cli/Cargo.toml` 的 `[dependencies]` 加:

```toml
clap = { version = "4", features = ["derive"] }
chrono = "0.4"
serde_json = "1"
```

- [ ] **Step 2: 写 CLI 骨架 + runs_dir + run/validate**

把 `crates/cli/src/main.rs` 顶部到 `main` 函数体整体改成下面结构(保留文件末尾的 `prompt_gate` 不变,保留 `mod render;`):

```rust
mod render;

use agentpipe_engine::audit::{event_json_line, RunRecorder};
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event};
use clap::{Parser, Subcommand};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

#[derive(Parser)]
#[command(name = "agentpipe", about = "本地研发流程编排引擎")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 执行 task.yaml
    Run {
        task: String,
        /// 只解析 + 校验 + 打印执行计划,不起任何 CLI 子进程
        #[arg(long)]
        dry_run: bool,
        /// 事件以 NDJSON 写 stdout,人读日志写 stderr
        #[arg(long)]
        json: bool,
    },
    /// 仅解析 + 校验 task.yaml
    Validate { task: String },
    /// 列出历史 run
    Runs,
    /// 重读某次 run 的事件
    View { run_id: String },
    /// 某次 run 的成本拆解
    Cost { run_id: String },
    /// 对比两次 run
    Diff { run_a: String, run_b: String },
}

/// ~/.agentpipe/runs(AGENTPIPE_HOME 优先)。
fn runs_dir() -> PathBuf {
    let base = std::env::var("AGENTPIPE_HOME")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".agentpipe").join("runs")
}

fn load_manifest(path: &str) -> Manifest {
    let yaml = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("读取 {path} 失败: {e}");
        std::process::exit(1);
    });
    match Manifest::parse(&yaml).and_then(|m| {
        m.validate()?;
        Ok(m)
    }) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("manifest 错误: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run { task, dry_run, json } => cmd_run(&task, dry_run, json),
        Cmd::Validate { task } => {
            load_manifest(&task);
            println!("✓ {task} 校验通过");
        }
        Cmd::Runs => commands::runs(),
        Cmd::View { run_id } => commands::view(&run_id),
        Cmd::Cost { run_id } => commands::cost(&run_id),
        Cmd::Diff { run_a, run_b } => commands::diff(&run_a, &run_b),
    }
}

fn cmd_run(task: &str, dry_run: bool, json: bool) {
    let manifest = load_manifest(task);

    if dry_run {
        println!("▶ 执行计划: {}", manifest.name);
        for step in &manifest.steps {
            println!("{}", render::render_plan_step(step));
        }
        return;
    }

    let bins = RunnerBins {
        claude: std::env::var("AGENTPIPE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        codex: std::env::var("AGENTPIPE_CODEX_BIN").unwrap_or_else(|_| "codex".into()),
    };
    let (etx, erx) = mpsc::channel::<Event>();
    let (ctx, crx) = mpsc::channel::<Command>();
    let name = manifest.name.clone();
    let control = std::sync::Arc::new(agentpipe_engine::control::Control::default());
    let handle = thread::spawn(move || {
        let mut ex = Executor::new(manifest, bins, control, etx, crx);
        ex.run()
    });

    // RunStarted 时开 recorder;失败降级为不落盘(审计是旁路)。
    let mut recorder: Option<RunRecorder> = None;
    // 人读输出去向:--json 时人读走 stderr,数据走 stdout。
    macro_rules! human {
        ($($a:tt)*) => {{
            if json { eprintln!($($a)*); } else { println!($($a)*); }
        }};
    }

    for event in erx {
        if matches!(event, Event::RunStarted { .. }) {
            recorder = RunRecorder::open(&runs_dir(), &name)
                .map_err(|e| eprintln!("(审计未启用: {e})"))
                .ok();
            if let Some(r) = &recorder {
                human!("(审计: {})", r.path().display());
            }
        }
        if let Some(r) = &mut recorder {
            r.record(&event);
        }
        if json {
            println!("{}", event_json_line(&event));
        }
        human!("{}", render::render_event(&event));

        match &event {
            Event::StepAwaitingGate { step_id, expects_artifact, .. } => {
                let cmd = prompt_gate(step_id, *expects_artifact);
                let _ = ctx.send(cmd);
            }
            Event::RunFinished { .. } => break,
            _ => {}
        }
    }
    let _ = handle.join();
}

mod commands;
```

> 注:`render::render_event` 已在 Task 1 落地;本步新增对 `render::render_plan_step` 与 `commands::*` 的引用,分别在 Step 3 / Task 6 实现 —— 故本步编译不过是预期,Step 4 先补 `render_plan_step` 与一个 `commands` 占位让 run/validate 可编译。

- [ ] **Step 3: 加 render_plan_step(dry-run 用)**

在 `crates/cli/src/render.rs` 加:

```rust
use agentpipe_engine::manifest::{Step, StepKind};

/// dry-run:把一个 step 渲染成一行计划。纯函数。
pub fn render_plan_step(step: &Step) -> String {
    let detail = match &step.kind {
        StepKind::Claude { verify, skill, .. } => {
            let s = skill.as_deref().map(|s| format!(" skill={s}")).unwrap_or_default();
            let v = verify.as_ref().map(|_| " +verify").unwrap_or_default();
            format!("claude{s}{v}")
        }
        StepKind::Codex { action, .. } => format!("codex {action:?}"),
        StepKind::Human { .. } => "human".into(),
        StepKind::Loop { until, max, body } => {
            format!("loop until={until} max={max} ({} 步)", body.len())
        }
    };
    format!("  - {} [{detail}]", step.id)
}
```

> 若 `StepKind::Human` 变体字段名不同,以 `crates/engine/src/manifest.rs` 实际定义为准(用 `..` 忽略字段即可)。

- [ ] **Step 4: 建 commands 占位模块(让 run/validate 先编译)**

新建 `crates/cli/src/commands.rs`:

```rust
//! 只读子命令(view / cost / runs / diff)。Task 6 / 7 实现。
pub fn runs() {
    eprintln!("(runs 未实现)");
}
pub fn view(_run_id: &str) {
    eprintln!("(view 未实现)");
}
pub fn cost(_run_id: &str) {
    eprintln!("(cost 未实现)");
}
pub fn diff(_a: &str, _b: &str) {
    eprintln!("(diff 未实现)");
}
```

- [ ] **Step 5: 编译 + 验证 run/validate/dry-run**

Run: `cargo build`
Expected: 编译通过。

```bash
cargo build
# validate
./target/debug/agentpipe validate tests/fixtures/sample-task.yaml   # → ✓ 校验通过
# dry-run
./target/debug/agentpipe run tests/fixtures/sample-task.yaml --dry-run   # → 打印执行计划,不起子进程
# 真跑(stub)+ 落盘
AGENTPIPE_HOME=/tmp/ap-home \
AGENTPIPE_CLAUDE_BIN=$PWD/tests/fixtures/stub-claude.sh \
AGENTPIPE_CODEX_BIN=$PWD/tests/fixtures/stub-codex.sh \
STUB_VERDICT=clean \
./target/debug/agentpipe run tests/fixtures/sample-task.yaml
ls /tmp/ap-home/.agentpipe/runs/    # → 应有一个 <run-id>.ndjson
# --json
AGENTPIPE_HOME=/tmp/ap-home AGENTPIPE_CLAUDE_BIN=... AGENTPIPE_CODEX_BIN=... STUB_VERDICT=clean \
./target/debug/agentpipe run tests/fixtures/sample-task.yaml --json | head -1   # → 一行 {"ts":...,"event":...}
```

Expected: validate/dry-run 行为正确;真跑落盘出 `.ndjson`;`--json` 首行是 NDJSON,人读提示在 stderr。

- [ ] **Step 6: 提交**

```bash
git add crates/cli/Cargo.toml crates/cli/src/main.rs crates/cli/src/render.rs crates/cli/src/commands.rs
git commit -m "feat(cli): clap 子命令 + run 落盘/--json/--dry-run + validate"
```

---

### Task 6: view / cost / runs 子命令

**Files:**
- Modify: `crates/cli/src/commands.rs`(实现 runs / view / cost)

**Interfaces:**
- Consumes:`runs_dir`(Task 5,改为 `pub(crate)`);`audit::{read_run, aggregate_cost, is_valid_run_id}`;`render::render_event`。

- [ ] **Step 1: 把 runs_dir 暴露给 commands 模块**

`crates/cli/src/main.rs` 的 `fn runs_dir()` 改为 `pub(crate) fn runs_dir()`。

- [ ] **Step 2: 实现 runs / view / cost**

把 `crates/cli/src/commands.rs` 整体替换为:

```rust
//! 只读子命令(view / cost / runs / diff)。
use agentpipe_engine::audit::{aggregate_cost, is_valid_run_id, read_run};
use std::path::PathBuf;

use crate::render::render_event;
use crate::runs_dir;

/// 解析 run-id → ndjson 路径,带 allowlist 校验(防路径穿越)。
fn run_path(run_id: &str) -> Option<PathBuf> {
    if !is_valid_run_id(run_id) {
        eprintln!("非法 run-id: {run_id}");
        return None;
    }
    Some(runs_dir().join(format!("{run_id}.ndjson")))
}

pub fn runs() {
    let dir = runs_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        println!("(无历史 run: {})", dir.display());
        return;
    };
    let mut ids: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".ndjson").map(str::to_string))
        .collect();
    ids.sort();
    ids.reverse(); // 时间戳前缀 → 倒序即最新在前
    if ids.is_empty() {
        println!("(无历史 run)");
        return;
    }
    for id in ids {
        if let Some(p) = run_path(&id) {
            let cost = read_run(&p).map(|e| aggregate_cost(&e).total_cost_usd).unwrap_or(0.0);
            println!("{id}  ${cost:.2}");
        }
    }
}

pub fn view(run_id: &str) {
    let Some(path) = run_path(run_id) else { std::process::exit(2) };
    let entries = match read_run(&path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("读取 {run_id} 失败: {e}");
            std::process::exit(1);
        }
    };
    for entry in &entries {
        // 只读:render_event 不碰 stdin,AwaitingGate 仅显示当时在等待
        println!("{}", render_event(&entry.event));
    }
}

pub fn cost(run_id: &str) {
    let Some(path) = run_path(run_id) else { std::process::exit(2) };
    let entries = match read_run(&path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("读取 {run_id} 失败: {e}");
            std::process::exit(1);
        }
    };
    let s = aggregate_cost(&entries);
    println!("run {run_id}");
    for (step, m) in &s.steps {
        println!("  {step}: {} 轮 · {:.1}s · ${:.2}", m.num_turns, m.duration_ms as f64 / 1000.0, m.cost_usd);
    }
    println!("总计: {} 轮 · {:.1}s · ${:.2}", s.total_turns, s.total_duration_ms as f64 / 1000.0, s.total_cost_usd);
}

pub fn diff(_a: &str, _b: &str) {
    eprintln!("(diff 未实现)");
}
```

- [ ] **Step 3: 编译 + 手动验证**

Run: `cargo build`
然后(沿用 Task 5 落盘的 run):

```bash
AGENTPIPE_HOME=/tmp/ap-home ./target/debug/agentpipe runs            # → 列出 run-id + 成本
RID=$(AGENTPIPE_HOME=/tmp/ap-home ./target/debug/agentpipe runs | head -1 | awk '{print $1}')
AGENTPIPE_HOME=/tmp/ap-home ./target/debug/agentpipe view "$RID"     # → 重放事件,不卡 stdin
AGENTPIPE_HOME=/tmp/ap-home ./target/debug/agentpipe cost "$RID"     # → per-step + 总成本
AGENTPIPE_HOME=/tmp/ap-home ./target/debug/agentpipe view "../etc/passwd"  # → 非法 run-id,退出 2
```

Expected: runs 倒序列出;view 重放不阻塞;cost 数值与 run 时一致;路径穿越被拒。

- [ ] **Step 4: 提交**

```bash
git add crates/cli/src/main.rs crates/cli/src/commands.rs
git commit -m "feat(cli): view / cost / runs 子命令(只读复用 render_event)"
```

---

### Task 7: diff 子命令

**Files:**
- Modify: `crates/cli/src/commands.rs`(实现 diff)

**Interfaces:**
- Consumes:`read_run`、`run_path`(Task 6)。

- [ ] **Step 1: 实现 diff**

把 `commands.rs` 的 `pub fn diff(_a: &str, _b: &str)` 占位替换为:

```rust
pub fn diff(a: &str, b: &str) {
    let (Some(pa), Some(pb)) = (run_path(a), run_path(b)) else { std::process::exit(2) };
    let (ea, eb) = match (read_run(&pa), read_run(&pb)) {
        (Ok(ea), Ok(eb)) => (ea, eb),
        _ => {
            eprintln!("读取 run 失败");
            std::process::exit(1);
        }
    };

    // 按 step_id 提取终态(status + cost),对比两次 run。
    use agentpipe_engine::protocol::Event;
    use std::collections::BTreeMap;
    fn finals(entries: &[agentpipe_engine::audit::RunEntry]) -> BTreeMap<String, (String, f64)> {
        let mut m = BTreeMap::new();
        for e in entries {
            if let Event::StepFinished { step_id, status, metrics, .. } = &e.event {
                let cost = metrics.as_ref().map(|x| x.cost_usd).unwrap_or(0.0);
                m.insert(step_id.clone(), (format!("{status:?}"), cost));
            }
        }
        m
    }
    let (fa, fb) = (finals(&ea), finals(&eb));
    let mut keys: Vec<&String> = fa.keys().chain(fb.keys()).collect();
    keys.sort();
    keys.dedup();

    println!("diff {a} ↔ {b}");
    for k in keys {
        match (fa.get(k), fb.get(k)) {
            (Some(x), None) => println!("  - {k}: 仅 A ({})", x.0),
            (None, Some(y)) => println!("  + {k}: 仅 B ({})", y.0),
            (Some(x), Some(y)) if x != y => {
                println!("  ~ {k}: {} ${:.2} → {} ${:.2}", x.0, x.1, y.0, y.1);
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 2: 编译 + 手动验证**

Run: `cargo build`

```bash
# 跑两次 run(第二次让某步状态/成本不同),再 diff
AGENTPIPE_HOME=/tmp/ap-home ./target/debug/agentpipe runs   # 取两个 run-id
AGENTPIPE_HOME=/tmp/ap-home ./target/debug/agentpipe diff <RID_A> <RID_B>
```

Expected: 打印仅 A / 仅 B / 状态或成本变化的步骤;无差异的步骤不输出。

- [ ] **Step 3: 全量校验 + 提交**

Run: `cargo build && cargo test`
Expected: 全绿。

```bash
git add crates/cli/src/commands.rs
git commit -m "feat(cli): diff 子命令(按 step 终态对比两次 run)"
```

---

## Self-Review(写完后核对 spec)

- spec §3 RunRecorder 消费者侧 / 引擎纯净 → Task 3 + Task 5(recorder 在 cli 事件循环,executor 不改) ✅
- spec §4 run-id 格式 + allowlist + AGENTPIPE_HOME → Task 2(run_id/is_valid)+ Task 5(runs_dir) ✅
- spec §5 NDJSON {ts,event} schema + 首行 RunStarted → Task 3(event_json_line)+ Task 5(RunStarted 时 open+record) ✅
- spec §6 子命令(run/validate/runs/view/cost/diff + dry-run + json) → Task 5/6/7 ✅
- spec §6.1 render_event 解耦 / view 不卡 stdin → Task 1 + Task 6(view 用 render_event) ✅
- spec §7 cost 聚合 → Task 4 + Task 6(cost)✅
- spec §8 依赖(chrono / clap / serde_json)→ Task 2 / Task 5 ✅
- spec §9 测试(recorder / run-id / cost / diff / smoke)→ 各任务测试 + Task 5/6/7 手动 smoke ✅
- spec §10 非目标(replay 重执行 / undo / 远程 / 脱敏 / 轮转)→ 不实现 ✅
- spec §11 落地顺序 → Task 1→7 与之一致 ✅
- 类型一致性:`RunRecorder` / `event_json_line` / `read_run` / `RunEntry` / `aggregate_cost` / `CostSummary` / `is_valid_run_id` / `run_id` 在 engine 定义,cli 按同名消费;`render_event` / `render_plan_step` / `runs_dir`(pub(crate))/ `run_path` 签名跨任务一致 ✅
- 已知风险:`render_plan_step` 依赖 `StepKind` 变体字段;Task 5 Step 3 已注明以 manifest.rs 实际定义为准、用 `..` 兜底。
