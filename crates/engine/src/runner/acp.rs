//! ACP (Agent Client Protocol) runner.
//!
//! 经 stdio JSON-RPC 驱动任何实现 ACP server 的外部 agent(claude-agent-acp /
//! codex-acp / gemini-cli --acp / ...)。设计见 docs/specs/2026-06-25-acp-integration-design.md。
//!
//! engine 主路径是同步,本模块在**专用线程**(`std::thread::scope`)内建一个
//! current-thread tokio runtime,把 SDK 的 async API `block_on` 起来,对外暴露与
//! ClaudeRunner/CodexRunner 同形的 sync 接口。
//!
//! 为什么用专用线程而不是直接 block_on:Tauri 等嵌入方主路径已在 tokio runtime 内,
//! 直接 `Builder::new_current_thread().build().block_on(...)` 会触发
//! "Cannot start a runtime from within a runtime" panic(review §A finding #5)。
//! 专用线程把 tokio 完全隔离在线程内,与外部 runtime 无关。

use crate::control::Control;
use crate::error::EngineError;
use crate::protocol::StepMetrics;
use std::sync::Once;
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
use std::sync::Arc;
use std::sync::mpsc::{self as std_mpsc, RecvTimeoutError};
use std::time::{Duration, Instant};

/// ACP 单步墙钟上限(秒),挂死兜底;到点取消并以 Err 返回。
/// `AGENTPIPE_ACP_TIMEOUT_SECS` 覆盖(>0 生效);默认与 Claude 同 30 分钟。
const DEFAULT_ACP_TIMEOUT_SECS: u64 = 1800;

/// 主线程轮询间隔(ms):每隔此时间检查一次 Control::is_aborted。
/// 工作线程的 timeout / abort 由 tokio `sleep_until` + `Notify` 事件驱动,
/// 不再 100ms 轮询(review §A finding #6 修旧版 select! starvation)。
const MAIN_POLL_INTERVAL_MS: u64 = 100;

/// 一次性 warn:ACP runner 不上报 cost,budget_usd 对 ACP step 形同虚设
/// (review §A finding #1)。
static WARN_ACP_COST_BYPASS: Once = Once::new();

/// 一个 ACP step 的配置:外部 agent 二进制启动命令。
///
/// command 是完整 shell-quoted 命令(SDK 的 `AcpAgent::from_str` 用 shell-words 切分),
/// 例如 `"npx @agentclientprotocol/claude-agent-acp"` 或绝对路径 + args。
#[derive(Debug, Clone)]
pub struct AcpConfig {
    pub agent: String,
    pub command: String,
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
        let timeout_secs =
            super::timeout_secs_from_env("AGENTPIPE_ACP_TIMEOUT_SECS", DEFAULT_ACP_TIMEOUT_SECS);
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
    /// 工作线程内建 current-thread tokio runtime,与外部 runtime 隔离(防 Tauri 嵌入
    /// 时 block_on 嵌套 panic — review §A finding #5)。工作线程内用
    /// `tokio::select!` + `sleep_until(deadline)` + `Notify` 实现 timeout / abort 的
    /// 事件驱动响应(不再 100ms 轮询,review §A finding #6)。
    ///
    /// 主线程职责:
    /// 1. drain 进度行 → 调 `on_progress`(on_progress 不必 Send,留主线程)
    /// 2. 每 100ms 检查 `Control::is_aborted` → 设了就 notify 工作线程
    /// 3. progress channel 关闭后取最终结果
    pub fn run(
        &self,
        prompt: &str,
        control: Option<&Control>,
        on_progress: &mut dyn FnMut(&str, Option<u32>),
        cwd: &Path,
    ) -> Result<AcpOutcome, EngineError> {
        // 一次性显式告警:ACP runner 当前 metrics 永远 None,budget_usd 对 ACP step 无效。
        // review §A finding #1 兜底:加 cost 提取还要等 SDK feature 升级,先把 budget
        // bypass 这件事让用户可观测,不会"以为有 budget 实则没有"。
        // 用 eprintln 而非 tracing::warn:CLI / Tauri 都没装 tracing_subscriber,
        // tracing::warn 会被默认 NoopSubscriber 吞 — 与 context.rs add_cost/set_budget
        // 的 warn 风格统一,保证用户能看到。
        WARN_ACP_COST_BYPASS.call_once(|| {
            eprintln!(
                "[agentpipe] WARN: ACP runner 当前不上报 token cost(MVP 未启用 \
                 unstable_end_turn_token_usage),ACP step 不计入 budget_usd — \
                 如需 budget 兜底请混合 claude / codex step。"
            );
        });

        let config = self.config.clone();
        let timeout_secs = self.timeout_secs;
        let prompt_owned = prompt.to_string();
        let cwd_owned = cwd.to_path_buf();

        // 工作线程 → 主线程:每条 progress 行 + 最终 answer/transcript
        let (progress_tx, progress_rx) = std_mpsc::channel::<String>();
        let (result_tx, result_rx) =
            std_mpsc::sync_channel::<Result<AcpOutcome, EngineError>>(1);

        // abort 信号(主线程 → 工作线程):Notify::notify_waiters 是事件驱动,工作线程
        // 在 select! 里等 notified(),不需要轮询。
        let abort_notify = Arc::new(tokio::sync::Notify::new());
        let abort_notify_worker = abort_notify.clone();

        // 用 std::thread::scope 共享 control 借用,避免要求 'static + Arc clone。
        // Control: Send + Sync(AtomicBool + Mutex<Option<u32>>),&Control: Send。
        std::thread::scope(|s| {
            // ── 工作线程:跑 tokio runtime + ACP 协议 ──
            s.spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = result_tx.send(Err(EngineError::Cli(format!(
                            "acp: tokio runtime 创建失败: {e}"
                        ))));
                        return;
                    }
                };
                let res = rt.block_on(run_acp_session(
                    &config,
                    timeout_secs,
                    &prompt_owned,
                    &cwd_owned,
                    progress_tx,
                    abort_notify_worker,
                ));
                let _ = result_tx.send(res);
            });

            // ── 主线程:drain 进度 + 轮询 abort ──
            loop {
                match progress_rx.recv_timeout(Duration::from_millis(MAIN_POLL_INTERVAL_MS)) {
                    Ok(line) => on_progress(&line, None),
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(c) = control {
                            if c.is_aborted() {
                                // 事件驱动通知 worker:notify_waiters 唤醒 select! 的
                                // abort_notify.notified() 分支,立即返回 abort Err。
                                abort_notify.notify_waiters();
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            // worker 退出 → 取结果。block(short)是必要的:worker 在 send result 后
            // 释放 result_tx,这里等到 worker 真正 send 完。
            match result_rx.recv() {
                Ok(r) => r,
                Err(_) => Err(EngineError::Cli("acp: 工作线程异常退出未返回结果".into())),
            }
        })
    }
}

/// 工作线程内的 ACP session:spawn agent → initialize → newSession → prompt → 收尾。
///
/// progress_tx: 每条渲染行送给主线程喂 on_progress;callback 内同步 send。
/// abort_notify: 主线程检测 Control::is_aborted 后调 notify_waiters 唤醒。
async fn run_acp_session(
    config: &AcpConfig,
    timeout_secs: u64,
    prompt: &str,
    cwd: &Path,
    progress_tx: std_mpsc::Sender<String>,
    abort_notify: Arc<tokio::sync::Notify>,
) -> Result<AcpOutcome, EngineError> {
    // chunk text 通过 tokio mpsc 串回主流程的 answer accumulator;
    // 不再用 Arc<Mutex<Vec<AcpEvent>>>(review §A finding #8 Mutex poison 隐患 +
    // §A finding #9 每 notification 三次 String clone 的热路径)。
    let (text_tx, mut text_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (transcript_tx, mut transcript_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();

    let agent = AcpAgent::from_str(&config.command).map_err(|e| {
        EngineError::Cli(format!("acp: 无法启动 agent `{}`: {e}", config.command))
    })?;

    let cwd_owned = cwd.to_path_buf();
    let prompt_owned = prompt.to_string();

    let builder = agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            {
                let text_tx = text_tx.clone();
                let transcript_tx = transcript_tx.clone();
                move |notification: SessionNotification, _cx| {
                    let text_tx = text_tx.clone();
                    let transcript_tx = transcript_tx.clone();
                    async move {
                        let rendered = format_update_for_log(&notification.update);
                        // 抓 chunk text → answer。其余 update 只进 transcript 不进 answer。
                        if let SessionUpdate::AgentMessageChunk(chunk) = &notification.update {
                            if let ContentBlock::Text(t) = &chunk.content {
                                let _ = text_tx.send(t.text.clone());
                            }
                        }
                        // rendered 一次性 move 进 transcript channel(主流程会 forward 到
                        // 主线程的 progress channel),不再 clone 三次。
                        let _ = transcript_tx.send(rendered);
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
                vec![ContentBlock::Text(TextContent::new(prompt_owned))],
            ))
            .block_task()
            .await?;

        Ok::<_, agent_client_protocol::Error>(prompt_resp.stop_reason)
    });

    // drop 本地的 text_tx / transcript_tx(callback 内的 clone 还活着),让两个 rx
    // 在 connect 完后自然收尾。
    drop(text_tx);
    drop(transcript_tx);

    let started = Instant::now();
    let deadline = started + Duration::from_secs(timeout_secs);
    tokio::pin!(connect_fut);

    let mut answer = String::new();
    let mut transcript_lines: Vec<String> = Vec::new();

    // ── 主循环:事件驱动 ──
    // 改造前:`select!{ progress | connect_fut | sleep(100ms) check_abort }`
    //         100ms 轮询是 CPU 浪费 + biased 下被 progress 流量饿死(review §A #6)。
    // 改造后:`select!{ text_rx | transcript_rx | connect_fut | abort_notify | sleep_until(deadline) }`
    //         abort/timeout 都是事件驱动,无轮询;abort_notify 与 progress 优先级
    //         同级,不再饿死。
    let stop_reason = loop {
        tokio::select! {
            Some(text) = text_rx.recv() => {
                answer.push_str(&text);
            }
            Some(line) = transcript_rx.recv() => {
                // forward 到主线程喂 on_progress;主线程会 break 出 loop 让我们收尾。
                // tx 关闭(主线程退) = 主流程已不接收,丢弃即可。
                let _ = progress_tx.send(line.clone());
                transcript_lines.push(line);
            }
            res = &mut connect_fut => {
                break match res {
                    Ok(stop) => Ok(stop),
                    Err(e) => Err(EngineError::Cli(format!("acp: 连接/通信失败: {e}"))),
                };
            }
            _ = abort_notify.notified() => {
                break Err(EngineError::Cli("acp: 被用户中止".into()));
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                break Err(EngineError::Cli(format!(
                    "acp: 步骤超时(>{}s)",
                    timeout_secs
                )));
            }
        }
    };

    // 收尾:把 text / transcript 残余尽量 drain(connect_fut 已结束,channel 仍可能
    // 有 in-flight notification 的尾巴 — try_recv 不阻塞,best-effort 拉空)。
    while let Ok(text) = text_rx.try_recv() {
        answer.push_str(&text);
    }
    while let Ok(line) = transcript_rx.try_recv() {
        let _ = progress_tx.send(line.clone());
        transcript_lines.push(line);
    }

    // drop progress_tx 让主线程的 recv 收到 Disconnected → 退出 drain 循环。
    drop(progress_tx);

    let stop_reason = stop_reason?;

    if answer.trim().is_empty() {
        return Err(EngineError::Cli(format!(
            "acp: agent 未返回任何文本 (stop_reason: {stop_reason:?})"
        )));
    }
    let full_transcript = transcript_lines.join("\n");
    Ok(AcpOutcome {
        answer,
        metrics: None,
        full_transcript,
    })
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
