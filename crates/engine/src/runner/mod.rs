pub mod claude;
pub mod codex;

use crate::control::Control;
use crate::error::EngineError;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// spawn 一个命令,返回 (stdout, exit_success)。黑盒:不解析协议,只收文本。
///
/// - `stdin`:有则经管道喂入(写完即关闭 → EOF);无则置 null。
/// - `timeout_secs`:有则到点 kill 并以 success=false 返回。
/// - `control`:有则把子进程放进独立进程组并登记 pgid,供宿主 Abort 杀整组;返回前清空。
///
/// stdout 在独立线程读到 EOF,避免管道缓冲写满死锁;stderr inherit 到终端。
pub fn run_command(
    bin: &str,
    args: &[String],
    cwd: &Path,
    stdin: Option<&str>,
    timeout_secs: Option<u64>,
    control: Option<&Control>,
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
    // 独立进程组(自身为组长,pgid==pid),使 Abort 能 killpg 整组。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| EngineError::Cli(format!("spawn {bin} 失败: {e}")))?;

    let mut out_pipe = child
        .stdout
        .take()
        .ok_or_else(|| EngineError::Cli("无法获取子进程 stdout".into()))?;

    if let Some(c) = control {
        c.set_current(Some(child.id()));
    }

    if let Some(s) = stdin {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(s.as_bytes());
        } // si 在此 drop → 子进程收到 EOF
    }

    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = out_pipe.read_to_string(&mut buf);
        buf
    });

    let result: Result<(String, bool), EngineError> = match timeout_secs {
        Some(secs) => {
            let deadline = Instant::now() + Duration::from_secs(secs);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok((reader.join().unwrap_or_default(), status.success())),
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            // 不 join reader:孙辈可能仍持有管道写端,超时输出本就丢弃。
                            break Ok((String::new(), false));
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => break Err(EngineError::Cli(e.to_string())),
                }
            }
        }
        None => match child.wait() {
            Ok(status) => Ok((reader.join().unwrap_or_default(), status.success())),
            Err(e) => Err(EngineError::Cli(e.to_string())),
        },
    };

    if let Some(c) = control {
        c.set_current(None);
    }
    result
}
