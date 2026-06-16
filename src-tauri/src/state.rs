use agentpipe_engine::control::Control;
use agentpipe_engine::protocol::Command;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

pub struct ActiveRun {
    pub commands: Sender<Command>,
    pub control: Arc<Control>,
}

#[derive(Default)]
pub struct AppState {
    pub active: Mutex<Option<ActiveRun>>,
}
