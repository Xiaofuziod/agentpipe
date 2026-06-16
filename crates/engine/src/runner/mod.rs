pub mod claude;
pub mod codex;

use crate::error::EngineError;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// spawn 一个命令,返回 (stdout, exit_success)。黑盒:不解析协议,只收文本。
///
/// - `stdin`:有则通过管道喂给子进程(写完即关闭 → EOF);无则置 null,避免子进程
///   误继承父进程 stdin。
/// - `timeout_secs`:有则到点 kill 子进程并以 success=false 返回,防止 CLI 挂死拖垮整条流水线。
///
/// stdout 在独立线程里读到 EOF,避免管道缓冲写满导致的死锁;stderr 直接 inherit 到终端。
pub fn run_command(
    bin: &str,
    args: &[String],
    cwd: &Path,
    stdin: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<(String, bool), EngineError> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

    let mut child = cmd
        .spawn()
        .map_err(|e| EngineError::Cli(format!("spawn {bin} 失败: {e}")))?;

    if let Some(s) = stdin {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(s.as_bytes());
        } // si 在此 drop → 子进程收到 EOF
    }

    // 独立线程读 stdout 到 EOF,防止缓冲写满阻塞子进程。
    let mut out_pipe = child
        .stdout
        .take()
        .ok_or_else(|| EngineError::Cli("无法获取子进程 stdout".into()))?;
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = out_pipe.read_to_string(&mut buf);
        buf
    });

    match timeout_secs {
        Some(secs) => {
            let deadline = Instant::now() + Duration::from_secs(secs);
            loop {
                match child
                    .try_wait()
                    .map_err(|e| EngineError::Cli(e.to_string()))?
                {
                    Some(status) => {
                        let stdout = reader.join().unwrap_or_default();
                        return Ok((stdout, status.success()));
                    }
                    None => {
                        if Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            // 不 join reader:子进程的孙辈(如 shell 下的 sleep)可能仍持有
                            // stdout 写端,join 会阻塞到其自然结束。超时路径输出本就按失败丢弃,
                            // 直接返回;reader 线程在管道 EOF 后自行结束(分离)。
                            return Ok((String::new(), false));
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        }
        None => {
            let status = child
                .wait()
                .map_err(|e| EngineError::Cli(e.to_string()))?;
            let stdout = reader.join().unwrap_or_default();
            Ok((stdout, status.success()))
        }
    }
}
