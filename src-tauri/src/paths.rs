use std::path::PathBuf;

/// 持久化根:AGENTPIPE_HOME 优先,否则 HOME,再否则当前目录。
/// runs / tasks 都挂在它下面的 .agentpipe/ 里。
fn base() -> PathBuf {
    let dir = std::env::var("AGENTPIPE_HOME")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(dir)
}

/// ~/.agentpipe/runs(AGENTPIPE_HOME 优先)。与 CLI runs_dir 同义。
pub fn runs_dir() -> PathBuf {
    base().join(".agentpipe").join("runs")
}

/// ~/.agentpipe/tasks:编排器保存的 task.yaml 默认落点。
pub fn tasks_dir() -> PathBuf {
    base().join(".agentpipe").join("tasks")
}

/// 把编排器里用户填的保存路径解析成可写的绝对路径。
///
/// 规则:`~` / `~/` 展开到 HOME;绝对路径原样使用;相对路径或裸名落到
/// `~/.agentpipe/tasks/` 下。修复:GUI 从 Finder 启动时进程 cwd 是只读的 `/`,
/// 裸名 / 相对路径经 `std::fs` 解析会指向只读根 → EROFS(os error 30)。
/// `~` 是 shell 概念,Rust 不会自动展开,这里显式处理。
pub fn resolve_task_path(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        return home();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home().join(rest);
    }
    let p = PathBuf::from(trimmed);
    if p.is_absolute() {
        return p;
    }
    tasks_dir().join(trimmed)
}

/// 真实 HOME(用于 `~` 展开;不受 AGENTPIPE_HOME 影响)。取不到时退回 `.`。
fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
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

    #[test]
    fn resolve_keeps_absolute_path() {
        let p = resolve_task_path("/tmp/my-task.yaml");
        assert_eq!(p, PathBuf::from("/tmp/my-task.yaml"));
    }

    #[test]
    fn resolve_bare_name_goes_under_tasks_dir() {
        let p = resolve_task_path("Test01");
        assert!(p.is_absolute(), "裸名必须解析成绝对路径,否则 cwd=/ 时只读");
        assert!(p.ends_with("Test01"));
        assert!(p.to_string_lossy().contains(".agentpipe/tasks"));
    }

    #[test]
    fn resolve_trims_trailing_space() {
        // 用户填过 "review mr "(带尾空格),不能原样当文件名
        let p = resolve_task_path("review mr ");
        assert!(p.ends_with("review mr"));
    }

    #[test]
    fn resolve_expands_tilde() {
        let p = resolve_task_path("~/tasks/x.yaml");
        assert!(p.is_absolute());
        assert!(p.ends_with("tasks/x.yaml"));
        assert!(!p.to_string_lossy().contains('~'), "~ 必须被展开");
    }
}
