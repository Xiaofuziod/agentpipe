pub mod claude;
pub mod codex;

use crate::control::Control;
use crate::error::EngineError;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// spawn 一个命令,返回 (stdout, exit_success)。黑盒:不解析协议,只收文本。
///
/// - `stdin`:有则经管道喂入(写完即关闭 → EOF);无则置 null。
/// - `timeout_secs`:有则到点 kill 并以 success=false 返回。
/// - `control`:有则把子进程放进独立进程组并登记 pgid,供宿主 Abort 杀整组;返回前清空。
/// - `on_line`:每读到一行 stdout 即回调一次(实时进度);仅转发文本,不解析协议。
///
/// stdout 在独立线程按行读、经 channel 送回,主循环 drain + 回调,避免管道缓冲死锁;
/// stderr inherit 到终端。
#[allow(clippy::too_many_arguments)]
pub fn run_command(
    bin: &str,
    args: &[String],
    cwd: &Path,
    stdin: Option<&str>,
    timeout_secs: Option<u64>,
    control: Option<&Control>,
    on_line: &mut dyn FnMut(&str),
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

    let out_pipe = child
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

    // reader 线程按行读 stdout,经 channel 送回(避免管道缓冲写满死锁)。
    let (line_tx, line_rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let reader = BufReader::new(out_pipe);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if line_tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut full = String::new();
    let drain = |rx: &mpsc::Receiver<String>, full: &mut String, on_line: &mut dyn FnMut(&str)| {
        while let Ok(l) = rx.try_recv() {
            on_line(&l);
            full.push_str(&l);
            full.push('\n');
        }
    };

    let deadline = timeout_secs.map(|s| Instant::now() + Duration::from_secs(s));
    let success = loop {
        drain(&line_rx, &mut full, on_line);
        match child
            .try_wait()
            .map_err(|e| EngineError::Cli(e.to_string()))?
        {
            Some(status) => {
                let _ = reader.join(); // 等 reader 读完剩余行
                drain(&line_rx, &mut full, on_line);
                break status.success();
            }
            None => {
                if let Some(dl) = deadline {
                    if Instant::now() >= dl {
                        let _ = child.kill();
                        let _ = child.wait();
                        drain(&line_rx, &mut full, on_line);
                        break false; // 超时按失败
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };

    if let Some(c) = control {
        c.set_current(None);
    }
    Ok((full, success))
}
