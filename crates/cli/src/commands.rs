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
    use agentpipe_engine::protocol::Event;
    let complete = matches!(entries.last().map(|e| &e.event), Some(Event::RunFinished { .. }));
    if !complete {
        println!("⚠ 未完成(无 RunFinished,可能中断/崩溃)");
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
