use agentpipe_engine::manifest::{CodexAction, Manifest, OnUnmet, RunMode, StepKind, Verifier};

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
fn parses_claude_step_with_verify() {
    let yaml = r#"
version: 1
name: v
target: /tmp
steps:
  - id: impl
    kind: claude
    prompt: 实现
    verify:
      by: codex
      action: review-mr
      base: dev
      max_retries: 3
      on_unmet: fail
"#;
    let m = Manifest::parse(yaml).unwrap();
    m.validate().expect("valid");
    match &m.steps[0].kind {
        StepKind::Claude { verify: Some(v), .. } => {
            assert_eq!(v.by, Verifier::Codex);
            assert_eq!(v.action, Some(CodexAction::ReviewMr));
            assert_eq!(v.base.as_deref(), Some("dev"));
            assert_eq!(v.max_retries, 3);
            assert_eq!(v.on_unmet, OnUnmet::Fail);
            assert!(v.feedback); // 默认 true
        }
        other => panic!("expected claude+verify, got {other:?}"),
    }
}

#[test]
fn verify_defaults_max_retries_and_gate() {
    let yaml = r#"
version: 1
name: v
target: /tmp
steps:
  - id: impl
    kind: claude
    prompt: 实现
    verify:
      by: codex
      action: review-mr
      base: dev
"#;
    let m = Manifest::parse(yaml).unwrap();
    match &m.steps[0].kind {
        StepKind::Claude { verify: Some(v), .. } => {
            assert_eq!(v.max_retries, 2); // 默认
            assert_eq!(v.on_unmet, OnUnmet::Gate); // 默认最保守
        }
        _ => unreachable!(),
    }
}

#[test]
fn validate_verify_review_mr_requires_base() {
    let yaml = r#"
version: 1
name: bad
target: /tmp
steps:
  - id: impl
    kind: claude
    prompt: 实现
    verify:
      by: codex
      action: review-mr
"#;
    let m = Manifest::parse(yaml).unwrap();
    let err = m.validate().unwrap_err();
    assert!(err.to_string().contains("verify") && err.to_string().contains("base"));
}

#[test]
fn parses_claude_verifier() {
    let yaml = r#"
version: 1
name: v
target: /tmp
steps:
  - id: impl
    kind: claude
    prompt: 实现
    verify:
      by: claude
      prompt: 判定目标是否达成
      skill: four-dimension-review
"#;
    let m = Manifest::parse(yaml).unwrap();
    m.validate().expect("valid");
    match &m.steps[0].kind {
        StepKind::Claude { verify: Some(v), .. } => {
            assert_eq!(v.by, Verifier::Claude);
            assert_eq!(v.prompt.as_deref(), Some("判定目标是否达成"));
            assert_eq!(v.skill.as_deref(), Some("four-dimension-review"));
        }
        other => panic!("expected claude verifier, got {other:?}"),
    }
}

#[test]
fn validate_claude_verifier_requires_prompt() {
    let yaml = r#"
version: 1
name: bad
target: /tmp
steps:
  - id: impl
    kind: claude
    prompt: 实现
    verify:
      by: claude
"#;
    let m = Manifest::parse(yaml).unwrap();
    let err = m.validate().unwrap_err().to_string();
    assert!(err.contains("claude") && err.contains("prompt"));
}

#[test]
fn validate_verify_max_retries_capped() {
    let yaml = r#"
version: 1
name: bad
target: /tmp
steps:
  - id: impl
    kind: claude
    prompt: 实现
    verify:
      by: codex
      action: review-mr
      base: dev
      max_retries: 99
"#;
    let m = Manifest::parse(yaml).unwrap();
    assert!(m.validate().unwrap_err().to_string().contains("max_retries"));
}

#[test]
fn validate_accepts_sample() {
    let yaml = include_str!("../../../tests/fixtures/sample-task.yaml");
    Manifest::parse(yaml)
        .unwrap()
        .validate()
        .expect("sample should be valid");
}
