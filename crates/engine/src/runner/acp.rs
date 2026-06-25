//! ACP (Agent Client Protocol) runner.
//!
//! 经 stdio JSON-RPC 驱动任何实现 ACP server 的外部 agent(claude-agent-acp /
//! codex-acp / gemini-cli --acp / ...)。设计见 docs/specs/2026-06-25-acp-integration-design.md。
//!
//! engine 主路径是同步,本模块在内部建一个 current-thread tokio runtime,把 SDK 的
//! async API `block_on` 起来,对外暴露与 ClaudeRunner/CodexRunner 同形的 sync 接口。

use crate::control::Control;
use crate::error::EngineError;
use crate::protocol::StepMetrics;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, CreateTerminalRequest, InitializeRequest, KillTerminalRequest,
    NewSessionRequest, PromptRequest, ReadTextFileRequest, ReleaseTerminalRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate, TerminalOutputRequest, TextContent,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};
use agent_client_protocol::{AcpAgent, Agent, ConnectionTo};
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// ACP 单步墙钟上限(秒),挂死兜底;到点取消并以 Err 返回。
/// `AGENTPIPE_ACP_TIMEOUT_SECS` 覆盖(>0 生效);默认与 Claude 同 30 分钟。
const DEFAULT_ACP_TIMEOUT_SECS: u64 = 1800;

/// 主循环轮询间隔(ms):每隔此时间检查一次 Control::is_aborted 与 timeout 墙钟。
/// 100ms 在 UX 响应性(按 abort ≤ 100ms 退出)与 CPU 占用之间折中。
const POLL_INTERVAL_MS: u64 = 100;

/// 一个 ACP step 的配置:外部 agent 二进制启动命令。
///
/// command 是完整 shell-quoted 命令(SDK 的 `AcpAgent::from_str` 用 shell-words 切分),
/// 例如 `"npx @agentclientprotocol/claude-agent-acp"` 或绝对路径 + args。
#[derive(Debug, Clone)]
pub struct AcpConfig {
    pub agent: String,
    pub command: String,
}

/// 一条从 notification callback 收到的事件:同时承担 transcript 留痕与 answer 聚合。
/// `MsgChunk` 同时存 raw text(拼 answer 用)与 rendered 行(transcript 用),其他类型
/// 只留 rendered。两类合一个 Vec,避免 chunks/transcript 两份 state 冗余。
enum AcpEvent {
    MsgChunk { text: String, rendered: String },
    Other { rendered: String },
}

impl AcpEvent {
    fn rendered(&self) -> &str {
        match self {
            AcpEvent::MsgChunk { rendered, .. } | AcpEvent::Other { rendered } => rendered,
        }
    }
    fn msg_text(&self) -> Option<&str> {
        match self {
            AcpEvent::MsgChunk { text, .. } => Some(text),
            _ => None,
        }
    }
}

/// 把所有反向 fs/terminal request type 一次性注册成 method_not_found reject handler。
/// 见 §7.4 / commit c2e6d02:逐 type 而非 dispatch fallback,否则 SDK 1.0 会把
/// outbound initialize 的 response 路径也拦掉。新增反向 type 加进列表即可。
macro_rules! reject_unsupported_reverse_requests {
    ($builder:expr, [$($T:ty),* $(,)?] $(,)?) => {{
        let b = $builder;
        $(
            let b = b.on_receive_request(
                async move |_r: $T, responder, _cx| {
                    responder.respond_with_error(
                        agent_client_protocol::Error::method_not_found(),
                    )
                },
                agent_client_protocol::on_receive_request!(),
            );
        )*
        b
    }};
}

pub struct AcpRunner {
    config: AcpConfig,
    timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct AcpOutcome {
    /// 聚合所有 `AgentMessageChunk` 的 text;空则 fail-loud(见 spec §7.5)。
    pub answer: String,
    /// MVP 不开 `unstable_end_turn_token_usage` feature → 始终 None。
    pub metrics: Option<StepMetrics>,
    /// 原始 transcript(每个 update 一行可读串),留作调试。
    pub full_transcript: String,
}

impl AcpRunner {
    pub fn new(config: AcpConfig) -> Self {
        let timeout_secs = std::env::var("AGENTPIPE_ACP_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_ACP_TIMEOUT_SECS);
        Self {
            config,
            timeout_secs,
        }
    }

    /// 显式指定超时(秒),供测试注入小值。
    pub fn with_timeout(config: AcpConfig, timeout_secs: u64) -> Self {
        Self {
            config,
            timeout_secs,
        }
    }

    /// 跑一次 ACP prompt 并返回聚合的 answer。
    ///
    /// 内部建 current-thread tokio runtime → `block_on(run_inner)`。run_inner 用
    /// `tokio::select!` 同时跑 connect_with future + progress channel drain +
    /// 100ms 轮询(检查 `Control::is_aborted` 与 timeout 墙钟)。任一触发则退出,
    /// connect_with future 被 drop 时 SDK 内部清理子进程(spawn AcpAgent 时拿到
    /// 的 transport 句柄一起释放)。
    pub fn run(
        &self,
        prompt: &str,
        control: Option<&Control>,
        on_progress: &mut dyn FnMut(&str, Option<u32>),
        cwd: &Path,
    ) -> Result<AcpOutcome, EngineError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EngineError::Cli(format!("acp: tokio runtime 创建失败: {e}")))?;
        rt.block_on(self.run_inner(prompt, control, on_progress, cwd))
    }

    async fn run_inner(
        &self,
        prompt: &str,
        control: Option<&Control>,
        on_progress: &mut dyn FnMut(&str, Option<u32>),
        cwd: &Path,
    ) -> Result<AcpOutcome, EngineError> {
        // notification callback 累计;主流程在 connect_with 退出后取最终值。
        let events: Arc<Mutex<Vec<AcpEvent>>> = Arc::new(Mutex::new(Vec::new()));

        // 流式 progress 通道:notification 内 `send`(同步),主循环 `recv` 后调
        // `on_progress`。on_progress 是 `&mut dyn FnMut`,不 Send/Sync,所以
        // 必须在主线程(=主 future)里调用,不能直接传进 SDK callback。
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        // spawn 外部 agent;AcpAgent 同时实现 transport,直接传给 connect_with。
        let agent = AcpAgent::from_str(&self.config.command).map_err(|e| {
            EngineError::Cli(format!(
                "acp: 无法启动 agent `{}`: {e}",
                self.config.command
            ))
        })?;

        let cwd_owned = cwd.to_path_buf();

        let builder = agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                {
                    let events = events.clone();
                    let tx = progress_tx.clone();
                    move |notification: SessionNotification, _cx| {
                        let events = events.clone();
                        let tx = tx.clone();
                        async move {
                            let rendered = format_update_for_log(&notification.update);
                            let event = match &notification.update {
                                SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                                    ContentBlock::Text(t) => AcpEvent::MsgChunk {
                                        text: t.text.clone(),
                                        rendered: rendered.clone(),
                                    },
                                    _ => AcpEvent::Other {
                                        rendered: rendered.clone(),
                                    },
                                },
                                _ => AcpEvent::Other {
                                    rendered: rendered.clone(),
                                },
                            };
                            events.lock().unwrap().push(event);
                            // 把渲染行送给主流程喂 on_progress(失败 = 主流程已退,丢弃即可)。
                            let _ = tx.send(rendered);
                            Ok(())
                        }
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |_request: RequestPermissionRequest, responder, _cx| {
                    // MVP 策略:统一 Cancelled,不弹审批(spec §7.4 / V2 接入 GateKind::Decision)。
                    tracing::warn!("acp: permission/request 收到,MVP 一律拒绝");
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ))
                },
                agent_client_protocol::on_receive_request!(),
            );

        // 反向 fs/terminal capability 全部 fail-loud method_not_found:MVP 不声明这些能力
        // (spec §3 / §7.4),agent 应直接用本机 fs;万一发了反向请求,立即回错而不让
        // send_request 永远阻塞(spec §6 防 agent 卡死)。
        let builder = reject_unsupported_reverse_requests!(
            builder,
            [
                ReadTextFileRequest,
                WriteTextFileRequest,
                CreateTerminalRequest,
                TerminalOutputRequest,
                ReleaseTerminalRequest,
                KillTerminalRequest,
                WaitForTerminalExitRequest,
            ],
        );

        let connect_fut = builder.connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
                // initialize 握手 + 版本协商。
                let init = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                // 协议版本 fail-loud:只接受 V1(MVP 锁 stable wire,见 spec §7.2 / §7.3)。
                // server 退化到 V0 / 返回未来 V2 都直接报错,不静默继续。
                if init.protocol_version != ProtocolVersion::V1 {
                    return Err(agent_client_protocol::util::internal_error(format!(
                        "acp: 协议版本不匹配,需要 V1,server 返回 {}",
                        init.protocol_version.as_u16()
                    )));
                }

                let new_session = connection
                    .send_request(NewSessionRequest::new(cwd_owned))
                    .block_task()
                    .await?;
                let session_id = new_session.session_id;

                let prompt_resp = connection
                    .send_request(PromptRequest::new(
                        session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(prompt.to_string()))],
                    ))
                    .block_task()
                    .await?;

                Ok::<_, agent_client_protocol::Error>(prompt_resp.stop_reason)
            });

        // drop 本地的 tx(callback 内的 tx_for_cb 还活着),让 progress_rx 在 connect 完后能收尾。
        drop(progress_tx);

        let started = Instant::now();
        let timeout = Duration::from_secs(self.timeout_secs);
        tokio::pin!(connect_fut);

        // 主循环:边喂 progress 边轮询 abort / timeout,直到 connect_fut 结束。
        let outcome: Result<_, EngineError> = loop {
            tokio::select! {
                biased;
                // 优先 drain progress,避免 backlog 让用户看到一卡一卡的进度。
                Some(line) = progress_rx.recv() => {
                    on_progress(&line, None);
                }
                res = &mut connect_fut => {
                    break match res {
                        Ok(stop) => Ok(stop),
                        Err(e) => Err(EngineError::Cli(format!("acp: 连接/通信失败: {e}"))),
                    };
                }
                _ = tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)) => {
                    if let Some(c) = control {
                        if c.is_aborted() {
                            break Err(EngineError::Cli("acp: 被用户中止".into()));
                        }
                    }
                    if started.elapsed() > timeout {
                        break Err(EngineError::Cli(format!(
                            "acp: 步骤超时(>{}s)",
                            self.timeout_secs
                        )));
                    }
                }
            }
        };

        // 收尾:把 progress_rx 剩余的行也喂出去(connect_fut 已结束,不再产生新行)。
        // 注:abort / timeout 路径下 connect_fut 被 drop,SDK transport 释放 → 子进程被 SIGTERM,
        // 仍然走这里把 transcript 末段刷出去,不丢可观测信息。
        while let Ok(line) = progress_rx.try_recv() {
            on_progress(&line, None);
        }

        let stop_reason = outcome?;

        let events = events.lock().unwrap();
        let answer: String = events.iter().filter_map(AcpEvent::msg_text).collect();
        if answer.trim().is_empty() {
            return Err(EngineError::Cli(format!(
                "acp: agent 未返回任何文本 (stop_reason: {stop_reason:?})"
            )));
        }
        let full_transcript = events
            .iter()
            .map(AcpEvent::rendered)
            .collect::<Vec<_>>()
            .join("\n");
        Ok(AcpOutcome {
            answer,
            metrics: None,
            full_transcript,
        })
    }
}

/// 把一个 SessionUpdate 渲染成一行人类可读日志。
fn format_update_for_log(update: &SessionUpdate) -> String {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
            ContentBlock::Text(t) => format!("[msg] {}", t.text.replace('\n', " ")),
            _ => "[msg] <non-text>".to_string(),
        },
        SessionUpdate::AgentThoughtChunk(chunk) => match &chunk.content {
            ContentBlock::Text(t) => format!("[think] {}", truncate(&t.text, 80)),
            _ => "[think] <non-text>".to_string(),
        },
        SessionUpdate::ToolCall(tc) => format!("[tool] {:?}", tc),
        SessionUpdate::ToolCallUpdate(tc) => format!("[tool-update] {:?}", tc),
        SessionUpdate::Plan(p) => format!("[plan] {:?}", p),
        _ => format!("[update] {:?}", update),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "..."
    }
}
