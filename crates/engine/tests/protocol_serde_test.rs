use agentpipe_engine::protocol::{Command, Event, GateKind};

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
fn command_deserializes_from_type_tag() {
    let c: Command =
        serde_json::from_str(r#"{"type":"ApproveGate","step_id":"fix","artifact":null}"#).unwrap();
    assert!(matches!(c, Command::ApproveGate { .. }));
}
