//! 任务级 git worktree 隔离。开启 `manifest.worktree` 时,引擎在 Run 开始前
//! 为 target 仓库建一个全新 worktree(新分支),把所有 step 的 cwd 指向它,
//! target 主工作区不受影响。见 docs/specs/2026-06-18-worktree-isolation-design.md。
//!
//! 直接用 `std::process::Command` 一次性调 git(捕获 stdout/stderr),不走 runner 的
//! 流式/进程组通道 —— worktree 创建是同步的快操作,不需要实时进度或 Abort kill。

use crate::error::EngineError;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// 为 `target` 所在 git 仓库创建一个隔离 worktree(新分支),返回路径与分支名。
/// 任何失败(非 git 仓库 / git 未装 / worktree add 失败)都返回 Err —— 调用方
/// 必须 fail-closed,绝不退回 target 原地跑。
pub fn create(target: &Path, name: &str) -> Result<Worktree, EngineError> {
    let repo_root = git_toplevel(target)?;
    let suffix = unique_suffix();
    let branch = format!("agentpipe/{}-{}", slugify(name), suffix);
    let repo_name = repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");
    // 落 <repo_root>/../.agentpipe-worktrees/<repo_name>-<suffix>:同盘、隐藏、与主工作区分离。
    let base = repo_root
        .parent()
        .unwrap_or(repo_root.as_path())
        .join(".agentpipe-worktrees");
    std::fs::create_dir_all(&base)
        .map_err(|e| EngineError::Worktree(format!("创建 worktree 基目录 {} 失败: {e}", base.display())))?;
    let path = base.join(format!("{repo_name}-{suffix}"));

    let out = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .args(["worktree", "add", "-b"])
        .arg(&branch)
        .arg(&path)
        .output()
        .map_err(|e| EngineError::Worktree(format!("git worktree add 启动失败(git 是否安装?): {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(EngineError::Worktree(format!(
            "git worktree add 失败: {}",
            err.trim()
        )));
    }
    Ok(Worktree { path, branch })
}

/// `git -C target rev-parse --show-toplevel` 拿仓库根。失败 = 非 git 仓库 / git 未装。
fn git_toplevel(target: &Path) -> Result<PathBuf, EngineError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(target)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| EngineError::Worktree(format!("git 启动失败(git 是否安装?): {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(EngineError::Worktree(format!(
            "target {} 不是 git 仓库: {}",
            target.display(),
            err.trim()
        )));
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(PathBuf::from(s))
}

/// epoch 秒 + pid,保证同机并发/连续起 Run 不撞分支名。
fn unique_suffix() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}-{}", std::process::id())
}

/// 任务名 → 分支安全 slug:小写,非 [a-z0-9] 折成单 '-',去首尾 '-',限长 32,空则 "task"。
fn slugify(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            s.push('-');
            prev_dash = true;
        }
    }
    let s = s.trim_matches('-');
    let s: String = s.chars().take(32).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "task".into()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn slugify_sanitizes_and_falls_back() {
        assert_eq!(slugify("Fix Login Bug!"), "fix-login-bug");
        assert_eq!(slugify("  审查/合并  "), "task"); // 全非 ascii-alnum → 空 → 兜底
        assert_eq!(slugify(""), "task");
        assert_eq!(slugify("a__b--c"), "a-b-c");
    }

    fn git(args: &[&str], cwd: &Path) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git 可用")
            .status
            .success();
        assert!(ok, "git {args:?} 失败");
    }

    #[test]
    fn create_makes_worktree_in_temp_repo() {
        let tmp = std::env::temp_dir().join(format!("ap-wt-{}", unique_suffix()));
        let repo = tmp.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&["init", "-q"], &repo);
        git(&["config", "user.email", "t@t"], &repo);
        git(&["config", "user.name", "t"], &repo);
        std::fs::write(repo.join("f.txt"), "hi").unwrap();
        git(&["add", "."], &repo);
        git(&["commit", "-q", "-m", "init"], &repo);

        let wt = create(&repo, "My Task").expect("应成功建 worktree");
        assert!(wt.path.exists(), "worktree 路径应存在: {}", wt.path.display());
        assert!(wt.path.join("f.txt").exists(), "worktree 应是完整签出");
        assert!(wt.branch.starts_with("agentpipe/my-task-"), "分支名: {}", wt.branch);

        // git worktree list 应含新 worktree
        let list = Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "worktree", "list"])
            .output()
            .unwrap();
        let list = String::from_utf8_lossy(&list.stdout);
        assert!(
            list.contains(wt.path.to_str().unwrap()),
            "worktree list 应含 {}:\n{list}",
            wt.path.display()
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn create_fails_on_non_git_dir() {
        let tmp = std::env::temp_dir().join(format!("ap-nogit-{}", unique_suffix()));
        std::fs::create_dir_all(&tmp).unwrap();
        let err = create(&tmp, "x").unwrap_err();
        assert!(err.to_string().contains("不是 git 仓库"), "err = {err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
