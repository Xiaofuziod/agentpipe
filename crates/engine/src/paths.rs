//! 数据目录单一来源:$AGENTPIPE_HOME(替代 HOME)或 $HOME,拼 `.agentpipe`。
//! 语义与 cli runs_dir / src-tauri paths 既有约定一致(AGENTPIPE_HOME 是替代
//! HOME,不是替代 ~/.agentpipe)。runs / agents.toml / (Phase B) watch state 共用。

use std::path::PathBuf;

pub fn base_dir() -> PathBuf {
    let base = std::env::var("AGENTPIPE_HOME")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".agentpipe")
}

pub fn registry_path() -> PathBuf {
    base_dir().join("agents.toml")
}
