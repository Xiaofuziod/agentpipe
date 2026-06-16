pub mod claude;
pub mod codex;

use crate::error::EngineError;
use std::path::Path;
use std::process::Command;

/// spawn 一个命令,返回 (stdout, exit_success)。黑盒:不解析协议,只收文本。
pub fn run_command(bin: &str, args: &[String], cwd: &Path) -> Result<(String, bool), EngineError> {
    let output = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| EngineError::Cli(format!("spawn {bin} 失败: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok((stdout, output.status.success()))
}
