use crate::error::EngineError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub version: u32,
    pub name: String,
    pub target: PathBuf,
    #[serde(default)]
    pub mode: RunMode,
    pub steps: Vec<Step>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Step,
    #[default]
    Auto,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Step {
    pub id: String,
    #[serde(flatten)]
    pub kind: StepKind,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StepKind {
    Claude {
        prompt: String,
        #[serde(default)]
        skill: Option<String>,
        /// 可选校验门:步骤跑完后判目标是否达成,未达成带反馈重试。见 verify-gate spec。
        #[serde(default)]
        verify: Option<Verify>,
    },
    Codex {
        action: CodexAction,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        base: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
    },
    Human {
        instruction: String,
        #[serde(default)]
        expects: Option<String>,
    },
    Loop {
        until: String,
        max: u32,
        body: Vec<Step>,
    },
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexAction {
    ReviewDoc,
    ReviewMr,
    Ask,
}

/// 校验门:由 codex 或 claude 判"目标达成"。见 verify-gate spec。
#[derive(Debug, Deserialize, Serialize)]
pub struct Verify {
    pub by: Verifier,
    /// codex verifier:判据形态(review-mr / review-doc / ask)。claude verifier 忽略。
    #[serde(default)]
    pub action: Option<CodexAction>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    /// codex(ask 指令)或 claude(判定指令)的 prompt。
    #[serde(default)]
    pub prompt: Option<String>,
    /// claude verifier 的 skill(可选)。
    #[serde(default)]
    pub skill: Option<String>,
    /// command verifier 的 shell 命令(仅 by: command 用);exit 0 = 达成。
    #[serde(default)]
    pub command: Option<String>,
    /// 未达成时重跑干活步骤的次数上限(0 = 纯质量门,不重试)。
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub on_unmet: OnUnmet,
    /// 重试时把 verifier findings 作为反馈注入干活 prompt。
    #[serde(default = "default_true")]
    pub feedback: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Verifier {
    Codex,
    /// claude 只读判定(`--permission-mode plan`),回复末行 `VERDICT: pass|fail`。
    Claude,
    /// shell 命令判定:exit 0 = 达成,否则未达成。findings = 输出尾部。
    Command,
}

/// 重试耗尽后的升级策略。默认 gate:最保守,交人决策。
#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OnUnmet {
    #[default]
    Gate,
    Fail,
    Continue,
}

fn default_max_retries() -> u32 {
    2
}
fn default_true() -> bool {
    true
}

/// verify.max_retries 的硬上限,防 runaway(每次重试都烧一个 claude + 一个 codex)。
const MAX_VERIFY_RETRIES: u32 = 10;

impl Manifest {
    pub fn parse(yaml: &str) -> Result<Self, EngineError> {
        serde_yml::from_str(yaml).map_err(|e| EngineError::Parse(e.to_string()))
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        for step in &self.steps {
            Self::validate_step(step)?;
        }
        Ok(())
    }

    /// codex 判据三个 action 的必填字段校验(codex step 与 verify 门复用)。
    fn validate_codex_fields(
        step_id: &str,
        ctx: &str,
        action: &CodexAction,
        path: &Option<String>,
        base: &Option<String>,
        prompt: &Option<String>,
    ) -> Result<(), EngineError> {
        let missing = match action {
            CodexAction::ReviewDoc if path.is_none() => Some(("review-doc", "path")),
            CodexAction::ReviewMr if base.is_none() => Some(("review-mr", "base")),
            CodexAction::Ask if prompt.is_none() => Some(("ask", "prompt")),
            _ => None,
        };
        match missing {
            Some((act, field)) => Err(EngineError::Validation(format!(
                "step '{step_id}': {ctx} {act} 需要 {field} 字段"
            ))),
            None => Ok(()),
        }
    }

    fn validate_step(step: &Step) -> Result<(), EngineError> {
        match &step.kind {
            StepKind::Claude { verify, .. } => {
                if let Some(v) = verify {
                    match v.by {
                        Verifier::Codex => {
                            let action = v.action.as_ref().ok_or_else(|| {
                                EngineError::Validation(format!(
                                    "step '{}': verify by codex 需要 action 字段",
                                    step.id
                                ))
                            })?;
                            Self::validate_codex_fields(&step.id, "verify codex", action, &v.path, &v.base, &v.prompt)?;
                        }
                        Verifier::Claude => {
                            if v.prompt.is_none() {
                                return Err(EngineError::Validation(format!(
                                    "step '{}': verify by claude 需要 prompt 字段(判定指令)",
                                    step.id
                                )));
                            }
                        }
                        Verifier::Command => {
                            if v.command.as_deref().map(str::trim).unwrap_or("").is_empty() {
                                return Err(EngineError::Validation(format!(
                                    "step '{}': verify by command 需要 command 字段(shell 命令)",
                                    step.id
                                )));
                            }
                        }
                    }
                    if v.max_retries > MAX_VERIFY_RETRIES {
                        return Err(EngineError::Validation(format!(
                            "step '{}': verify.max_retries 不能超过 {MAX_VERIFY_RETRIES}",
                            step.id
                        )));
                    }
                }
                Ok(())
            }
            StepKind::Codex {
                action,
                path,
                base,
                prompt,
            } => Self::validate_codex_fields(&step.id, "codex", action, path, base, prompt),
            StepKind::Loop { body, until, .. } => {
                if until != "codex-clean" {
                    return Err(EngineError::Validation(format!(
                        "step '{}': Phase 1 仅支持 until: codex-clean",
                        step.id
                    )));
                }
                // codex-clean 的收敛判据取 body 里 codex step 的 verdict;无则永不收敛,
                // 必然空转到 max,属配置错误,提前拒绝而非静默 runaway。
                if !body
                    .iter()
                    .any(|s| matches!(s.kind, StepKind::Codex { .. }))
                {
                    return Err(EngineError::Validation(format!(
                        "step '{}': until codex-clean 的 loop body 必须含至少一个 codex step",
                        step.id
                    )));
                }
                for s in body {
                    Self::validate_step(s)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml_with_verify(verify_block: &str) -> String {
        format!(
            "version: 1\nname: t\ntarget: /tmp\nmode: auto\nsteps:\n  - id: impl\n    kind: claude\n    prompt: \"do\"\n    verify:\n{verify_block}"
        )
    }

    #[test]
    fn command_verify_requires_command() {
        let y = yaml_with_verify("      by: command\n");
        let m = Manifest::parse(&y).unwrap();
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("command"), "err = {err}");
    }

    #[test]
    fn command_verify_ok_with_command() {
        let y = yaml_with_verify("      by: command\n      command: \"cargo test\"\n");
        let m = Manifest::parse(&y).unwrap();
        assert!(m.validate().is_ok());
    }
}
