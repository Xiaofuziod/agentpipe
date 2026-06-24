//! 修复 GUI 从 Finder / Dock / launchd 启动时被精简的 PATH。
//!
//! 终端启动的进程继承用户 shell 的完整 PATH;但 Finder / Dock 启动的 .app 只拿到
//! `/usr/bin:/bin:/usr/sbin:/sbin`,找不到装在 `~/.local/bin`(claude)、
//! `/opt/homebrew/bin`(codex / gh)等处的 CLI,引擎 `spawn claude` 直接 ENOENT。
//!
//! 这里用登录 shell 取真实 PATH 注回**本进程**环境,使引擎 spawn 的子进程(claude /
//! codex)以及它们自身再起的 git / gh 都能解析到这些二进制。CLI 版(`agentpipe run`)
//! 从终端启动本就有完整 PATH,不经 GUI 入口,故不受影响。

const MARKER: &str = "__AGENTPIPE_PATH__";

/// 在 GUI 进程启动早期调用。仅 macOS 生效;PATH 已"富"(含常见非系统 bin 目录)则跳过,
/// 省去 dev / 终端启动时的 shell 开销。best-effort:取不到就保持原样。
#[cfg(target_os = "macos")]
pub fn repair_path_from_login_shell() {
    let current = std::env::var("PATH").unwrap_or_default();
    if looks_rich(&current) {
        return;
    }
    if let Some(path) = login_shell_path() {
        std::env::set_var("PATH", path);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn repair_path_from_login_shell() {}

/// 判断 PATH 是否已包含用户级 bin 目录 —— 是则说明继承自终端,无需修复。
/// Finder 精简 PATH(`/usr/bin:/bin:/usr/sbin:/sbin`)三项都不命中,必走修复。
fn looks_rich(path: &str) -> bool {
    path.split(':').any(|p| {
        p == "/opt/homebrew/bin" || p == "/usr/local/bin" || p.ends_with("/.local/bin")
    })
}

/// 起登录+交互 shell(source 完 .zprofile/.zshrc)打印 PATH。用 marker 包裹,
/// 抵御 rc 文件向 stdout 打印的噪声。失败 / 空一律返回 None(保持原 PATH)。
#[cfg(target_os = "macos")]
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let probe = format!("command printf '{MARKER}%s{MARKER}' \"$PATH\"");
    let out = std::process::Command::new(shell)
        .args(["-ilc", &probe])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    extract_path(&String::from_utf8_lossy(&out.stdout))
}

/// 从 shell stdout 里抽出两个 marker 之间的 PATH。trim 后为空则 None。
fn extract_path(stdout: &str) -> Option<String> {
    let mut parts = stdout.split(MARKER);
    parts.next()?; // marker 前的噪声
    let path = parts.next()?.trim(); // 两个 marker 之间
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_finder_path_is_not_rich() {
        assert!(!looks_rich("/usr/bin:/bin:/usr/sbin:/sbin"));
    }

    #[test]
    fn homebrew_or_local_bin_counts_as_rich() {
        assert!(looks_rich("/usr/bin:/opt/homebrew/bin:/bin"));
        assert!(looks_rich("/usr/bin:/usr/local/bin"));
        assert!(looks_rich("/Users/me/.local/bin:/usr/bin"));
    }

    #[test]
    fn extract_ignores_rc_noise_around_markers() {
        let stdout = format!("welcome to my shell\n{MARKER}/opt/homebrew/bin:/usr/bin{MARKER}\n");
        assert_eq!(
            extract_path(&stdout).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn extract_none_when_no_marker_or_empty() {
        assert_eq!(extract_path("just noise, no markers"), None);
        assert_eq!(extract_path(&format!("{MARKER}{MARKER}")), None);
        assert_eq!(extract_path(&format!("{MARKER}   {MARKER}")), None);
    }
}
