use agentpipe_engine::manifest::{CodexAction, Manifest, RunMode, StepKind};

#[test]
fn parses_sample_manifest() {
    let yaml = include_str!("../../../tests/fixtures/sample-task.yaml");
    let m = Manifest::parse(yaml).expect("should parse");
    assert_eq!(m.version, 1);
    assert_eq!(m.name, "demo");
    assert!(matches!(m.mode, RunMode::Auto));
    assert_eq!(m.steps.len(), 2);

    match &m.steps[0].kind {
        StepKind::Codex {
            action: CodexAction::ReviewDoc,
            path,
            ..
        } => {
            assert_eq!(path.as_deref(), Some("docs/spec.md"));
        }
        other => panic!("expected codex review-doc, got {other:?}"),
    }

    match &m.steps[1].kind {
        StepKind::Loop { until, max, body } => {
            assert_eq!(until, "codex-clean");
            assert_eq!(*max, 3);
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected loop, got {other:?}"),
    }
}

#[test]
fn rejects_invalid_yaml() {
    let err = Manifest::parse("not: [valid").unwrap_err();
    assert!(err.to_string().contains("parse"));
}

#[test]
fn validate_codex_review_doc_requires_path() {
    let yaml = r#"
version: 1
name: bad
target: /tmp
steps:
  - id: x
    kind: codex
    action: review-doc
"#;
    let m = Manifest::parse(yaml).unwrap();
    let err = m.validate().unwrap_err();
    assert!(err.to_string().contains("review-doc") && err.to_string().contains("path"));
}

#[test]
fn validate_codex_review_mr_requires_base() {
    let yaml = r#"
version: 1
name: bad
target: /tmp
steps:
  - id: x
    kind: codex
    action: review-mr
"#;
    let m = Manifest::parse(yaml).unwrap();
    assert!(m.validate().is_err());
}

#[test]
fn validate_loop_codex_clean_requires_codex_step() {
    let yaml = r#"
version: 1
name: bad
target: /tmp
steps:
  - id: l
    kind: loop
    until: codex-clean
    max: 2
    body:
      - id: only
        kind: claude
        prompt: x
"#;
    let m = Manifest::parse(yaml).unwrap();
    let err = m.validate().unwrap_err();
    assert!(err.to_string().contains("codex step"));
}

#[test]
fn validate_accepts_sample() {
    let yaml = include_str!("../../../tests/fixtures/sample-task.yaml");
    Manifest::parse(yaml)
        .unwrap()
        .validate()
        .expect("sample should be valid");
}
