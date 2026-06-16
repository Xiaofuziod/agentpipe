use super::run_command;
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

    /// allow_writes 决定权限姿态(确切 flag 以 Task 1 实测为准,这里给默认形态)。
    /// timeout_secs 透传到 run_command,防止 claude 挂死拖垮流水线。
    pub fn run(
        &self,
        prompt: &str,
        skill: Option<&str>,
        allow_writes: bool,
        timeout_secs: Option<u64>,
        cwd: &Path,
    ) -> Result<ClaudeOutcome, EngineError> {
        let full_prompt = match skill {
            Some(s) => format!("/{s} {prompt}"),
            None => prompt.to_string(),
        };
        let mut args = vec!["-p".to_string(), full_prompt];
        if allow_writes {
            // 占位:Task 1 实测确认确切 flag 后替换
            args.push("--permission-mode".into());
            args.push("acceptEdits".into());
        }
        let (stdout, success) = run_command(&self.bin, &args, cwd, None, timeout_secs)?;
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
