use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// 运行时控制句柄(Phase 1b Task 2 占位,Task 6 充实进程组杀)。
#[derive(Default)]
pub struct Control {
    pub abort: AtomicBool,
    pub current_pid: Mutex<Option<u32>>,
}
