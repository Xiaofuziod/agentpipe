mod common;

use agentpipe_engine::agents::load_and_resolve_if_needed;
use agentpipe_engine::manifest::{Manifest, StepKind};
use common::{EnvGuard, ENV_LOCK};

fn acp_commands(manifest: &Manifest) -> Vec<Option<String>> {
    fn walk(steps: &[agentpipe_engine::manifest::Step], out: &mut Vec<Option<String>>) {
        for step in steps {
            match &step.kind {
                StepKind::Acp { command, .. } => out.push(command.clone()),
                StepKind::Loop { body, .. } => walk(body, out),
                _ => {}
            }
        }
    }

    let mut out = Vec::new();
    walk(&manifest.steps, &mut out);
    out
}

#[test]
fn inline_acp_skips_bad_registry() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("ap-agents-it-skip-bad");
    let registry_dir = dir.join(".agentpipe");
    std::fs::create_dir_all(&registry_dir).unwrap();
    std::fs::write(registry_dir.join("agents.toml"), "[agents.gemini\ncommand=").unwrap();
    let _home = EnvGuard::set("AGENTPIPE_HOME", dir.to_str().unwrap());

    let mut manifest = Manifest::parse(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: gemini\n    command: inline\n    prompt: p\n",
    )
    .unwrap();

    load_and_resolve_if_needed(&mut manifest).expect("inline command 不应读取坏 registry");
    assert_eq!(acp_commands(&manifest), vec![Some("inline".into())]);
}

#[test]
fn named_acp_reads_bad_registry_and_fails_loud() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("ap-agents-it-need-bad");
    let registry_dir = dir.join(".agentpipe");
    std::fs::create_dir_all(&registry_dir).unwrap();
    std::fs::write(registry_dir.join("agents.toml"), "[agents.gemini\ncommand=").unwrap();
    let _home = EnvGuard::set("AGENTPIPE_HOME", dir.to_str().unwrap());

    let mut manifest = Manifest::parse(
        "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: gemini\n    prompt: p\n",
    )
    .unwrap();

    let err = load_and_resolve_if_needed(&mut manifest)
        .unwrap_err()
        .to_string();
    assert!(err.contains("解析失败"), "{err}");
}
