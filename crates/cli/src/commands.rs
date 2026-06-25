//! 只读子命令(view / cost / runs / diff)。
use std::path::PathBuf;

use agentpipe_engine::audit::{aggregate_cost, is_valid_run_id, read_run, step_finals, RunEntry};
use agentpipe_engine::protocol::Event;

use crate::render::{format_metrics, render_event};
use crate::runs_dir;

/// 解析 run-id → ndjson 路径,带 allowlist 校验(防路径穿越)。
fn run_path(run_id: &str) -> Option<PathBuf> {
    if !is_valid_run_id(run_id) {
        eprintln!("invalid run-id: {run_id}");
        return None;
    }
    Some(runs_dir().join(format!("{run_id}.ndjson")))
}

/// 校验 run-id 并读取该 run 的事件:非法 run-id → 退出 2,读取失败 → 退出 1。
/// view / cost / diff 共用的加载入口。
fn load_run(run_id: &str) -> Vec<RunEntry> {
    let Some(path) = run_path(run_id) else { std::process::exit(2) };
    match read_run(&path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to read {run_id}: {e}");
            std::process::exit(1);
        }
    }
}


pub fn runs() {
    let dir = runs_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        println!("(no runs: {})", dir.display());
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
        println!("(no runs)");
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
    let entries = load_run(run_id);
    for entry in &entries {
        // 只读:render_event 不碰 stdin,AwaitingGate 仅显示当时在等待
        println!("{}", render_event(&entry.event));
    }
    let complete = matches!(entries.last().map(|e| &e.event), Some(Event::RunFinished { .. }));
    if !complete {
        println!("⚠ incomplete (no RunFinished; interrupted or crashed)");
    }
}

pub fn cost(run_id: &str) {
    let entries = load_run(run_id);
    let s = aggregate_cost(&entries);
    println!("run {run_id}");
    for (step, m) in &s.steps {
        println!("  {step}: {}", format_metrics(m.num_turns, m.duration_ms, m.cost_usd));
    }
    println!("total: {}", format_metrics(s.total_turns, s.total_duration_ms, s.total_cost_usd));
}

pub fn diff(a: &str, b: &str) {
    let (fa, fb) = (step_finals(&load_run(a)), step_finals(&load_run(b)));
    let mut keys: Vec<&String> = fa.keys().chain(fb.keys()).collect();
    keys.sort();
    keys.dedup();

    println!("diff {a} ↔ {b}");
    for k in keys {
        match (fa.get(k), fb.get(k)) {
            (Some(x), None) => println!("  - {k}: only A ({})", x.status),
            (None, Some(y)) => println!("  + {k}: only B ({})", y.status),
            (Some(x), Some(y)) if x != y => {
                println!("  ~ {k}: {} ${:.2} → {} ${:.2}", x.status, x.cost_usd, y.status, y.cost_usd);
            }
            _ => {}
        }
    }
}
