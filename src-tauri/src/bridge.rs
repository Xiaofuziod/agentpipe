use agentpipe_engine::control::Control;
use agentpipe_engine::executor::{Executor, RunnerBins};
use agentpipe_engine::manifest::Manifest;
use agentpipe_engine::protocol::{Command, Event};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use tauri::{AppHandle, Emitter, Manager};

pub const EVENT_CHANNEL: &str = "engine://event";

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

    // 转发线程:引擎事件 → webview
    let forward_app = app.clone();
    thread::spawn(move || {
        for evt in event_rx {
            let _ = forward_app.emit(EVENT_CHANNEL, evt);
        }
        // event_rx 关闭 = 引擎线程结束(正常 RunFinished 后返回,或 panic)。
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
