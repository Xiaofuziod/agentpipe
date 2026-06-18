use agentpipe_engine::audit::RunRecorder;
use agentpipe_engine::control::Control;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event, RunStatus};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use tauri::{AppHandle, Emitter, Manager};

pub const EVENT_CHANNEL: &str = "engine://event";

#[derive(Clone, serde::Serialize)]
struct RunIdPayload {
    run_id: String,
}

fn run_name(evt: &Event) -> &str {
    match evt {
        Event::RunStarted { name } => name,
        _ => "run",
    }
}

pub struct Started {
    pub commands: mpsc::Sender<Command>,
    pub control: Arc<Control>,
}

/// 在后台线程跑引擎,事件经转发线程 emit 到 webview。Run 终止后清空 AppState.active。
pub fn start(app: AppHandle, manifest: Manifest, bins: RunnerBins) -> Started {
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let control = Arc::new(Control::default());
    let engine_control = control.clone();

    // 转发线程:引擎事件 → webview(+ 落 NDJSON,审计旁路)
    let forward_app = app.clone();
    thread::spawn(move || {
        let mut saw_finished = false;
        let mut recorder: Option<RunRecorder> = None;
        for evt in event_rx {
            if matches!(evt, Event::RunStarted { .. }) {
                recorder = RunRecorder::open(&crate::paths::runs_dir(), run_name(&evt))
                    .map_err(|e| eprintln!("(GUI 审计未启用: {e})"))
                    .ok();
                if let Some(r) = &recorder {
                    let _ = forward_app.emit(
                        "engine://run-started-id",
                        RunIdPayload { run_id: r.run_id().to_string() },
                    );
                }
            }
            if let Some(r) = &mut recorder {
                r.record(&evt);
            }
            if matches!(evt, Event::RunFinished { .. }) {
                saw_finished = true;
            }
            let _ = forward_app.emit(EVENT_CHANNEL, evt);
        }
        // event_rx 关闭 = 引擎线程结束。正常路径 executor 已 emit RunFinished;
        // 若没见过(引擎 panic 在 emit 前),合成一个终态,否则 webview 的 activeId/live/busy
        // 永不复位 → UI 永久卡死("等待引擎事件"、prompt 禁用)。
        if !saw_finished {
            let _ = forward_app.emit(
                EVENT_CHANNEL,
                Event::RunFinished {
                    status: RunStatus::Failed,
                },
            );
        }
        // 一律清空 active —— 不能只在 RunFinished 上清,否则引擎 panic 时会永久卡住单 Run 不变式。
        if let Some(st) = forward_app.try_state::<crate::state::AppState>() {
            *st.active.lock().unwrap() = None;
        }
    });

    // 引擎线程
    thread::spawn(move || {
        let mut ex = Executor::new(manifest, bins, engine_control, event_tx, cmd_rx);
        ex.run();
    });

    Started {
        commands: cmd_tx,
        control,
    }
}
