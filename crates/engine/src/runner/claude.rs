use super::run_command;
use crate::control::Control;
use crate::error::EngineError;
use std::path::Path;

pub struct ClaudeRunner {
    bin: String,
}

pub struct ClaudeOutcome {
    pub full_output: String,
    pub last_line: String,
}

impl ClaudeRunner {
    pub fn new(bin: String) -> Self {
        Self { bin }
    }

    /// claude step 一律以 CLI 最高权限跑:headless 下唯有 bypassPermissions 能让 claude
    /// 自主 edit + bash(提交/建 MR 需要),acceptEdits 只放行编辑、挡 bash。
    /// 见 docs/specs/cli-behavior-findings.md。不暴露超时旋钮,挂死靠控制台 Interrupt 兜底。
    pub fn run(
        &self,
        prompt: &str,
        skill: Option<&str>,
        control: Option<&Control>,
        on_line: &mut dyn FnMut(&str),
        cwd: &Path,
    ) -> Result<ClaudeOutcome, EngineError> {
        let full_prompt = match skill {
            Some(s) => format!("/{s} {prompt}"),
            None => prompt.to_string(),
        };
        let args = vec![
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
            "-p".to_string(),
            full_prompt,
        ];
        let (stdout, success) =
            run_command(&self.bin, &args, cwd, None, None, control, on_line)?;
        if !success {
            return Err(EngineError::Cli("claude 非零退出".into()));
        }
        let last_line = stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .to_string();
        Ok(ClaudeOutcome {
            full_output: stdout,
            last_line,
        })
    }
}
