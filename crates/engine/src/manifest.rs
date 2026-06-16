use crate::error::EngineError;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub name: String,
    pub target: PathBuf,
    #[serde(default)]
    pub mode: RunMode,
    pub steps: Vec<Step>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Step,
    #[default]
    Auto,
}

#[derive(Debug, Deserialize)]
pub struct Step {
    pub id: String,
    #[serde(flatten)]
    pub kind: StepKind,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StepKind {
    Claude {
        prompt: String,
        #[serde(default)]
        skill: Option<String>,
        #[serde(default)]
        allow_writes: bool,
        #[serde(default)]
        timeout: Option<u64>,
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

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexAction {
    ReviewDoc,
    ReviewMr,
    Ask,
}

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

    fn validate_step(step: &Step) -> Result<(), EngineError> {
        match &step.kind {
            StepKind::Codex {
                action,
                path,
                base,
                prompt,
            } => match action {
                CodexAction::ReviewDoc if path.is_none() => Err(EngineError::Validation(format!(
                    "step '{}': codex review-doc 需要 path 字段",
                    step.id
                ))),
                CodexAction::ReviewMr if base.is_none() => Err(EngineError::Validation(format!(
                    "step '{}': codex review-mr 需要 base 字段",
                    step.id
                ))),
                CodexAction::Ask if prompt.is_none() => Err(EngineError::Validation(format!(
                    "step '{}': codex ask 需要 prompt 字段",
                    step.id
                ))),
                _ => Ok(()),
            },
            StepKind::Loop { body, until, .. } => {
                if until != "codex-clean" {
                    return Err(EngineError::Validation(format!(
                        "step '{}': Phase 1 仅支持 until: codex-clean",
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
