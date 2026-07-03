//! 集成测试共享 helper。子目录 mod 形式(`tests/common/mod.rs`),cargo 不会把它
//! 编译成独立测试二进制;各测试文件用 `mod common;` 引入。此前 ENV_LOCK/EnvGuard
//! 在 executor_test 与 codex_runner_test 各一份逐字拷贝、fixture 三份 —— 收敛单点,
//! 改 panic-safety / 路径规则只动这一处。
//!
//! 各测试二进制只用到子集(如 claude_runner_test 只用 fixture),对未用项放开
//! dead_code —— 共享模块按二进制分别编译,逐一 cfg 判定不值得。
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, Once};

/// 进程级 env 变量竞态防护:stub 行为靠 STUB_* env 控制,并行测试必须串行化。
pub static ENV_LOCK: Mutex<()> = Mutex::new(());

static BUILD_MOCK_ACP: Once = Once::new();

/// env 变量的 RAII 清理:测试退出(正常或 panic 展开)自动 remove_var,防止跨测试
/// 污染(review-2 §C finding #10:裸 remove_var 在断言之后,panic 跳过清理 →
/// 同进程后续抢到 ENV_LOCK 的测试看到泄漏的 env var)。
pub struct EnvGuard(&'static str);

impl EnvGuard {
    pub fn set(key: &'static str, value: &str) -> Self {
        std::env::set_var(key, value);
        Self(key)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var(self.0);
    }
}

/// tests/fixtures/ 下 stub 脚本的绝对路径。
pub fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

/// 构建并返回 `examples/mock_acp_agent` 二进制路径。scenario 由调用点通过
/// `env MOCK_ACP_SCENARIO=<name> <bin>` 组合,避免把场景焊在 helper 里。
pub fn mock_acp_agent_bin() -> String {
    BUILD_MOCK_ACP.call_once(|| {
        let status = Command::new("cargo")
            .args(["build", "--quiet", "--example", "mock_acp_agent"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .expect("spawn cargo build");
        assert!(
            status.success(),
            "cargo build --example mock_acp_agent 失败"
        );
    });
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let bin = workspace_root.join("target/debug/examples/mock_acp_agent");
    assert!(
        bin.exists(),
        "mock_acp_agent binary 不存在: {}",
        bin.display()
    );
    bin.to_string_lossy().into_owned()
}
