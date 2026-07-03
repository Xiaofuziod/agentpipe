//! ACP agent registry:`base_dir()/agents.toml` 把"启动命令"从模板解耦,
//! 模板只写 agent 名即可跨机器分享。见 acp-hardening spec D4。
//!
//! ```toml
//! [agents.gemini]
//! command = "gemini --experimental-acp"
//! ```

use crate::error::EngineError;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct AgentRegistry {
    map: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
struct RegistryFile {
    #[serde(default)]
    agents: HashMap<String, RegistryEntry>,
}

#[derive(serde::Deserialize)]
struct RegistryEntry {
    command: String,
}

impl AgentRegistry {
    /// 文件不存在 = 空表(仅用内联 command 的用户零感知);存在但解析失败 =
    /// fail-loud(防"改了 registry 没生效"的静默漂移)。
    pub fn load_default() -> Result<Self, EngineError> {
        Self::load_from(&crate::paths::base_dir().join("agents.toml"))
    }

    pub fn load_from(path: &Path) -> Result<Self, EngineError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(EngineError::Validation(format!(
                    "读取 agents registry {} 失败: {e}",
                    path.display()
                )))
            }
        };
        let parsed: RegistryFile = toml::from_str(&text).map_err(|e| {
            EngineError::Validation(format!(
                "agents registry {} 解析失败(TOML): {e}",
                path.display()
            ))
        })?;
        Ok(Self {
            map: parsed.agents.into_iter().map(|(k, v)| (k, v.command)).collect(),
        })
    }

    pub fn command_for(&self, agent: &str) -> Option<&str> {
        self.map.get(agent).map(String::as_str)
    }
}

/// pre-pass:把 registry 命中回填进 command 为 None 的 acp step(含 loop body 递归)。
/// 查不到不报错 —— "仍为 None" 由 Manifest::validate 给可解释报错,validate 保持纯函数。
/// 调用时机:宿主 parse 之后、validate / Executor::try_new 之前。
pub fn resolve_agents(manifest: &mut crate::manifest::Manifest, registry: &AgentRegistry) {
    fn walk(steps: &mut [crate::manifest::Step], registry: &AgentRegistry) {
        for s in steps {
            match &mut s.kind {
                crate::manifest::StepKind::Acp { agent, command, .. } => {
                    if command.is_none() {
                        if let Some(c) = registry.command_for(agent) {
                            *command = Some(c.to_string());
                        }
                    }
                }
                crate::manifest::StepKind::Loop { body, .. } => walk(body, registry),
                _ => {}
            }
        }
    }
    walk(&mut manifest.steps, registry);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_acp_commands(steps: &[crate::manifest::Step]) -> Vec<Option<String>> {
        let mut out = Vec::new();
        for s in steps {
            match &s.kind {
                crate::manifest::StepKind::Acp { command, .. } => out.push(command.clone()),
                crate::manifest::StepKind::Loop { body, .. } => out.extend(collect_acp_commands(body)),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn missing_file_is_empty_registry() {
        let r = AgentRegistry::load_from(Path::new("/nonexistent/agents.toml")).unwrap();
        assert!(r.command_for("gemini").is_none());
    }

    #[test]
    fn parses_entries() {
        let dir = std::env::temp_dir().join("ap-agents-test-ok");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("agents.toml");
        std::fs::write(&p, "[agents.gemini]\ncommand = \"gemini --acp\"\n").unwrap();
        let r = AgentRegistry::load_from(&p).unwrap();
        assert_eq!(r.command_for("gemini"), Some("gemini --acp"));
        assert!(r.command_for("unknown").is_none());
    }

    #[test]
    fn bad_toml_fails_loud() {
        let dir = std::env::temp_dir().join("ap-agents-test-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("agents.toml");
        std::fs::write(&p, "[agents.gemini\ncommand=").unwrap();
        let err = AgentRegistry::load_from(&p).unwrap_err().to_string();
        assert!(err.contains("解析失败"), "{err}");
    }

    #[test]
    fn resolve_fills_only_missing_command_recursively() {
        let y = "version: 1\nname: t\ntarget: /tmp\nsteps:\n  - id: a\n    kind: acp\n    agent: gemini\n    prompt: p\n  - id: b\n    kind: acp\n    agent: gemini\n    command: inline-wins\n    prompt: p\n  - id: l\n    kind: loop\n    until: codex-clean\n    max: 2\n    body:\n      - id: r\n        kind: codex\n        action: review-mr\n        base: main\n      - id: inner\n        kind: acp\n        agent: gemini\n        prompt: p\n";
        let mut m = crate::manifest::Manifest::parse(y).unwrap();
        let mut reg = AgentRegistry::default();
        reg.map.insert("gemini".into(), "gemini --acp".into());
        resolve_agents(&mut m, &reg);
        let cmds: Vec<Option<String>> = collect_acp_commands(&m.steps);
        assert_eq!(cmds, vec![
            Some("gemini --acp".into()),
            Some("inline-wins".into()),
            Some("gemini --acp".into()),
        ]);
    }
}
