use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// 运行时控制句柄:中止标志 + 当前在跑子进程组 id。
/// 引擎与宿主(Tauri)各持一份 Arc,宿主置中止 + 杀进程,引擎在边界检查后停下。
#[derive(Default)]
pub struct Control {
    abort: AtomicBool,
    current_pgid: Mutex<Option<u32>>,
}

impl Control {
    /// 请求中止:置标志 + 杀掉当前子进程组。
    pub fn request_abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
        self.kill_current();
    }

    pub fn is_aborted(&self) -> bool {
        self.abort.load(Ordering::SeqCst)
    }

    /// 登记/清除当前在跑子进程的进程组 id(= 组长 pid)。
    pub fn set_current(&self, pgid: Option<u32>) {
        *self.current_pgid.lock().unwrap() = pgid;
    }

    /// 杀掉当前子进程组(连带 shell 起的孙辈,解决"杀 bash 杀不掉 sleep")。
    pub fn kill_current(&self) {
        if let Some(pgid) = *self.current_pgid.lock().unwrap() {
            #[cfg(unix)]
            unsafe {
                libc::killpg(pgid as i32, libc::SIGKILL);
            }
            #[cfg(not(unix))]
            {
                let _ = pgid;
            }
        }
    }
}
