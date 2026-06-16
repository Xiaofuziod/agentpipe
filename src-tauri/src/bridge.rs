use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event};
use std::sync::mpsc;
use std::thread;
use tauri::{AppHandle, Emitter, Manager};

pub const EVENT_CHANNEL: &str = "engine://event";

pub struct Started {
    pub commands: mpsc::Sender<Command>,
}

/// 在后台线程跑引擎,事件经转发线程 emit 到 webview。Run 终止后清空 AppState.active。
pub fn start(app: AppHandle, manifest: Manifest, bins: RunnerBins) -> Started {
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();

    // 转发线程:引擎事件 → webview
    let forward_app = app.clone();
    thread::spawn(move || {
        for evt in event_rx {
            let is_final = matches!(evt, Event::RunFinished { .. });
            let _ = forward_app.emit(EVENT_CHANNEL, evt);
            if is_final {
                if let Some(st) = forward_app.try_state::<crate::state::AppState>() {
                    *st.active.lock().unwrap() = None;
                }
            }
        }
        // event_tx 在引擎线程结束时 drop → 本循环退出
    });

    // 引擎线程
    thread::spawn(move || {
        let mut ex = Executor::new(manifest, bins, event_tx, cmd_rx);
        ex.run();
    });

    Started { commands: cmd_tx }
}
