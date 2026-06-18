use std::path::PathBuf;

/// ~/.agentpipe/runs(AGENTPIPE_HOME 优先)。与 CLI runs_dir 同义。
pub fn runs_dir() -> PathBuf {
    let base = std::env::var("AGENTPIPE_HOME")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".agentpipe").join("runs")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runs_dir_uses_agentpipe_home() {
        // 仅验证拼接结构(不改全局 env,避免与其他测试竞争):末两段固定
        let d = runs_dir();
        assert!(d.ends_with("runs"));
        assert!(d.to_string_lossy().contains(".agentpipe"));
    }
}
