//! Mock ACP agent — 用于 acp_runner.rs 的 fixture。
//!
//! 行为由 `MOCK_ACP_SCENARIO` 环境变量选择,默认 `happy`。覆盖场景见
//! docs/specs/2026-06-25-acp-integration-design.md §8.2。
//!
//! 当前实现的场景:
//! - happy:流 3 个 chunk 后 EndTurn,client 应拼出完整 answer。
//! - empty:不发任何 chunk 直接 EndTurn,client 必须 fail-loud。
//! - long_stream:每秒发 1 个 chunk × 30 次,留窗口让 abort/timeout 测试触发。
//! - wrong_version:initialize 返回 ProtocolVersion::V0,client 必须 fail-loud。

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionId,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Dispatch, Result, Stdio};
use std::time::Duration;

fn scenario() -> String {
    std::env::var("MOCK_ACP_SCENARIO").unwrap_or_else(|_| "happy".into())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let scenario = scenario();
    eprintln!("[mock-acp-agent] scenario={scenario}");

    Agent
        .builder()
        .name("mock-acp-agent")
        .on_receive_request(
            {
                let scenario = scenario.clone();
                async move |initialize: InitializeRequest, responder, _cx| {
                    if scenario == "wrong_version" {
                        // 故意返回 V0 让 client 侧 protocolVersion 检查 fail-loud。
                        let _ = initialize;
                        return responder.respond(InitializeResponse::new(ProtocolVersion::V0));
                    }
                    responder.respond(
                        InitializeResponse::new(initialize.protocol_version)
                            .agent_capabilities(AgentCapabilities::new()),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_new_session: NewSessionRequest, responder, _cx| {
                responder.respond(NewSessionResponse::new(SessionId::new("mock-sess-1")))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let scenario = scenario.clone();
                async move |prompt: PromptRequest, responder, cx: ConnectionTo<Client>| {
                    match scenario.as_str() {
                        "happy" => {
                            // 流 3 个 chunk 后返回 EndTurn。
                            for chunk_text in ["你好", ",", "我是 mock。"] {
                                let _ = cx.send_notification(SessionNotification::new(
                                    prompt.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(chunk_text)),
                                    )),
                                ));
                            }
                            responder.respond(PromptResponse::new(StopReason::EndTurn))
                        }
                        "empty" => {
                            // 不发任何 chunk,直接 EndTurn。client 应当 fail-loud。
                            responder.respond(PromptResponse::new(StopReason::EndTurn))
                        }
                        "long_stream" => {
                            // 每秒发一个 chunk × 30 次,给 abort / timeout 测试足够窗口。
                            for i in 0..30u32 {
                                let _ = cx.send_notification(SessionNotification::new(
                                    prompt.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(format!(
                                            "chunk-{i}"
                                        ))),
                                    )),
                                ));
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                            responder.respond(PromptResponse::new(StopReason::EndTurn))
                        }
                        _ => responder.respond(PromptResponse::new(StopReason::EndTurn)),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_dispatch(
            async move |message: Dispatch, cx: ConnectionTo<Client>| {
                message.respond_with_error(
                    agent_client_protocol::util::internal_error("unhandled message"),
                    cx,
                )
            },
            agent_client_protocol::on_receive_dispatch!(),
        )
        .connect_to(Stdio::new())
        .await
}
