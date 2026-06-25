//! ACP integration smoke test — 跑真实外部 ACP server。
//!
//! 默认 opt-out:CI 不跑(避免依赖 node/npm 与外网)。
//! 开发本机:`AGENTPIPE_ACP_INTEGRATION=1 cargo test -p agentpipe-engine --test acp_smoke`
//!
//! 命令默认 `npx --yes @agentclientprotocol/claude-agent-acp`,可经
//! `AGENTPIPE_ACP_SMOKE_CMD` 覆盖(例如 `codex-acp` 或绝对路径)。
//!
//! 验证:initialize 握手成功 + prompt 拿到非空 answer。不验内容(模型回复非确定)。

use agentpipe_engine::runner::acp::{AcpConfig, AcpRunner};

fn smoke_command() -> String {
    std::env::var("AGENTPIPE_ACP_SMOKE_CMD")
        .unwrap_or_else(|_| "npx --yes @agentclientprotocol/claude-agent-acp".to_string())
}

#[test]
fn real_agent_smoke() {
    if std::env::var("AGENTPIPE_ACP_INTEGRATION").ok().as_deref() != Some("1") {
        eprintln!(
            "skip:设置 AGENTPIPE_ACP_INTEGRATION=1 以跑真实 ACP agent 烟测 \
             (默认命令 `npx --yes @agentclientprotocol/claude-agent-acp`,\
             可经 AGENTPIPE_ACP_SMOKE_CMD 覆盖)"
        );
        return;
    }

    let command = smoke_command();
    eprintln!("acp smoke: command={command}");
    // 真实 agent 首次 spawn 可能需要拉 npm 包或冷启动模型 SDK,给 120s。
    let runner = AcpRunner::with_timeout(
        AcpConfig {
            agent: "smoke".to_string(),
            command,
        },
        120,
    );
    let mut progress = Vec::<String>::new();
    let cwd = std::env::current_dir().unwrap();
    let outcome = runner
        .run(
            "请回复一个字:好",
            None,
            &mut |line, _round| progress.push(line.to_string()),
            &cwd,
        )
        .expect("真实 agent 应当返回 answer");
    eprintln!("acp smoke: answer={:?}", outcome.answer);
    assert!(!outcome.answer.trim().is_empty(), "answer 不应为空");
    assert!(
        !progress.is_empty(),
        "应至少收到 1 行 streaming progress,实际 0 行"
    );
}
