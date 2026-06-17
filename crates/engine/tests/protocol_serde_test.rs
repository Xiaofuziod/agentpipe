use agentpipe_engine::protocol::{Command, Event, GateKind, StepMetrics, StepStatus};

#[test]
fn event_serializes_with_type_tag() {
    let e = Event::StepStarted {
        step_id: "rev".into(),
        kind: "codex".into(),
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains("\"type\":\"StepStarted\""));
    assert!(j.contains("\"step_id\":\"rev\""));
}

#[test]
fn gate_event_carries_kind() {
    let e = Event::StepAwaitingGate {
        step_id: "fix".into(),
        suggestion: "s".into(),
        expects_artifact: false,
        gate_kind: GateKind::Decision,
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains("\"gate_kind\":\"decision\""));
}

#[test]
fn step_progress_carries_round() {
    // UI 镜像(ui/src/types.ts)依赖字段名 "round";锁住 wire shape。
    let e = Event::StepProgress {
        step_id: "impl".into(),
        line: "调用 Bash".into(),
        round: Some(2),
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains("\"type\":\"StepProgress\""));
    assert!(j.contains("\"round\":2"));
}

#[test]
fn step_finished_carries_metrics() {
    // UI 镜像依赖 metrics.{num_turns,duration_ms,cost_usd};锁住 wire shape。
    let e = Event::StepFinished {
        step_id: "impl".into(),
        status: StepStatus::Done,
        summary: "done".into(),
        metrics: Some(StepMetrics {
            num_turns: 3,
            duration_ms: 3266,
            cost_usd: 0.49,
        }),
    };
    let j = serde_json::to_string(&e).unwrap();
    assert!(j.contains("\"num_turns\":3"));
    assert!(j.contains("\"duration_ms\":3266"));
    assert!(j.contains("\"cost_usd\":0.49"));
}

#[test]
fn command_deserializes_from_type_tag() {
    let c: Command =
        serde_json::from_str(r#"{"type":"ApproveGate","step_id":"fix","artifact":null}"#).unwrap();
    assert!(matches!(c, Command::ApproveGate { .. }));
}
