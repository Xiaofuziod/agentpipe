use chrono::{DateTime, Utc};

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
            // 注:_ 不是 alphanumeric → 折成 -;is_valid_run_id 另行允许 _(外部传入 id)
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

use crate::protocol::{Event, StepMetrics};
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crate::protocol::{Event, RunStatus};
    use std::path::PathBuf;

    fn unique_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agentpipe-test-{}-{tag}", std::process::id()))
    }

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

    #[test]
    fn slug_retrim_after_truncation() {
        // 39 个 'a' + '!' + 'b' → 截断前 slug "aaa…a-b"(41 字符)
        // take(40) → "aaa…a-",再去尾 '-' → "aaa…a"(39 字符)
        let name = "a".repeat(39) + "!b";
        let t = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let id = run_id(&name, t);
        assert!(!id.contains("--"), "不应有连续破折号: {id}");
        assert!(!id.ends_with('-'), "不应以破折号结尾: {id}");
        assert!(is_valid_run_id(&id), "应是合法 run-id: {id}");
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

    #[test]
    fn read_run_roundtrips_recorder_output() {
        use crate::protocol::StepStatus;

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
        use crate::protocol::StepStatus;

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
}
