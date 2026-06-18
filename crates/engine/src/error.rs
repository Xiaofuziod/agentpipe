use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("manifest parse error: {0}")]
    Parse(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("cli error: {0}")]
    Cli(String),
    #[error("worktree error: {0}")]
    Worktree(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
