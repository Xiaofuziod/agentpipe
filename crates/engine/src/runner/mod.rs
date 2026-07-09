pub mod acp;
pub mod claude;
pub mod codex;

use crate::control::Control;
use crate::error::EngineError;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 从环境变量读 runner 墙钟超时(秒);非正数或解析失败回落 `default`。
/// 三家 runner(claude/codex/acp)的 `AGENTPIPE_*_TIMEOUT_SECS` 走同一套语义。
pub(crate) fn timeout_secs_from_env(env_key: &str, default: u64) -> u64 {
    std::env::var(env_key)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// 子进程 stderr 的保留预算。诊断只需要最后的崩溃/报错现场,而 codex review 会把
/// 整份 git diff 写进 stderr(实测 22k 行),整份留存会撑爆 EngineError 与 audit。
const STDERR_TAIL_MAX_BYTES: usize = 8 * 1024;

/// 一次子进程调用的产物。
pub struct CommandOutput {
    /// 完整 stdout(逐行回调后累积)。
    pub stdout: String,
    /// stderr 尾部。按 `STDERR_TAIL_MAX_BYTES` 字节预算滚动保留,解码时丢弃被切开的
    /// 首行,故成串长度略小于预算(非法字节替换为 U+FFFD 时可略大)。失败路径的根因在这里。
    pub stderr_tail: String,
    /// 退出码是否为 0。超时被 kill 也记为 false。
    pub success: bool,
}

/// spawn 一个命令,返回 [`CommandOutput`]。黑盒:不解析协议,只收文本。
///
/// - `stdin`:有则经管道喂入(写完即关闭 → EOF);无则置 null。
/// - `timeout_secs`:有则到点 kill 并以 success=false 返回。
/// - `control`:有则把子进程放进独立进程组并登记 pgid,供宿主 Abort 杀整组;返回前清空。
/// - `on_line`:每读到一行 stdout 即回调一次(实时进度);仅转发文本,不解析协议。
///
/// stdout / stderr 各起一个独立线程持续读,避免管道缓冲写满导致子进程阻塞。
///
/// stderr 必须捕获、不能 inherit:宿主 stderr 在 GUI(Tauri)下是 cargo/tauri-cli 转发的
/// **非阻塞**管道,子进程狂写 stderr 会拿到 EAGAIN,而 Rust 子进程遇 stderr 写失败直接
/// panic 自杀(实测 codex 0.144:`failed printing to stderr: os error 35`)。引擎只看到
/// "输出没了",把进程崩溃误判成"输出无法解析"。
/// 见 docs/specs/2026-07-10-codex-stderr-eagain-design.md。
#[allow(clippy::too_many_arguments)]
pub fn run_command(
    bin: &str,
    args: &[String],
    cwd: &Path,
    stdin: Option<&str>,
    timeout_secs: Option<u64>,
    control: Option<&Control>,
    on_line: &mut dyn FnMut(&str),
) -> Result<CommandOutput, EngineError> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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

    let err_pipe = child
        .stderr
        .take()
        .ok_or_else(|| EngineError::Cli("无法获取子进程 stderr".into()))?;

    // stderr 独立线程持续排空,分块经 channel 送回(与 stdout reader 同构)。
    // 用 channel 而不是共享 buffer:超时路径不 join 这个线程,主线程返回后 err_rx drop,
    // 线程下次 send 失败即自行结束;共享 buffer 没有这条退出通道,会把线程永久漏在 read 上。
    let (err_tx, err_rx) = mpsc::channel::<Vec<u8>>();
    let err_reader = std::thread::spawn(move || {
        let mut src = err_pipe;
        let mut buf = [0u8; 4096];
        loop {
            match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if err_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    if let Some(c) = control {
        c.set_current(Some(child.id()));
        // 关竞态:若 request_abort 在 spawn 与 set_current 之间到达,那次 kill_current 扑空
        // (current 还是 None)。此处 set_current 后补查一次,把刚 spawn 的进程即时杀掉,
        // 否则要等它自然结束才停(中止延迟一个子进程时长)。
        if c.is_aborted() {
            c.kill_current();
        }
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

    // stderr 只保留尾部预算:滚动丢弃超出的头部,留下最后写入的内容(崩溃现场在那里)。
    let mut err_tail: Vec<u8> = Vec::new();
    let drain_err = |rx: &mpsc::Receiver<Vec<u8>>, tail: &mut Vec<u8>| {
        while let Ok(chunk) = rx.try_recv() {
            tail.extend_from_slice(&chunk);
            let len = tail.len();
            if len > STDERR_TAIL_MAX_BYTES {
                tail.drain(..len - STDERR_TAIL_MAX_BYTES);
            }
        }
    };

    let clear = |control: Option<&Control>| {
        if let Some(c) = control {
            c.set_current(None);
        }
    };
    let deadline = timeout_secs.map(|s| Instant::now() + Duration::from_secs(s));
    let success = loop {
        drain(&line_rx, &mut full, on_line);
        drain_err(&err_rx, &mut err_tail);
        match child.try_wait() {
            Err(e) => {
                clear(control);
                return Err(EngineError::Cli(e.to_string()));
            }
            Ok(Some(status)) => {
                // 立即清 pgid:子进程已被 reap,其 pid 可能被 OS 复用,缩小 killpg 误伤窗口
                clear(control);
                let _ = reader.join(); // 等 reader 读完剩余行
                let _ = err_reader.join(); // 同上:等 stderr 收完再取尾部
                drain(&line_rx, &mut full, on_line);
                drain_err(&err_rx, &mut err_tail);
                break status.success();
            }
            Ok(None) => {
                if let Some(dl) = deadline {
                    if Instant::now() >= dl {
                        // 杀整个进程组(含 shell 起的孙辈),不是只杀直接子进程
                        #[cfg(unix)]
                        unsafe {
                            libc::killpg(child.id() as i32, libc::SIGKILL);
                        }
                        #[cfg(not(unix))]
                        {
                            let _ = child.kill();
                        }
                        let _ = child.wait();
                        clear(control);
                        // 不 join reader:若孙辈逃出进程组 / killpg 失败,管道不会 EOF,
                        // join 会永久挂死引擎线程。超时输出本就丢弃,best-effort drain 已到的行,
                        // reader 线程在 line_rx drop 后自行结束(分离)。
                        // err_reader 同理不 join,同样靠 err_rx drop 收尾;超时诊断需要
                        // 已收下的那段 stderr,故此处仍 drain 一次。
                        drain(&line_rx, &mut full, on_line);
                        drain_err(&err_rx, &mut err_tail);
                        break false; // 超时按失败
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    Ok(CommandOutput { stdout: full, stderr_tail: tail_to_string(&err_tail), success })
}

/// 失败路径的错误信息后缀:把子进程 stderr 尾部附上。空 stderr 不产生噪音后缀。
/// claude / codex 两家 runner 共用,别各写一份(错误文案漂移最容易从这里开始)。
pub(crate) fn stderr_hint(tail: &str) -> String {
    let t = tail.trim();
    if t.is_empty() {
        String::new()
    } else {
        format!("。子进程 stderr 尾部:\n{t}")
    }
}

/// 把 stderr 尾部字节转成可读文本。滚动丢弃头部时会从任意字节切开,首行既可能是半个
/// UTF-8 字符、也可能是半句话 —— 丢掉第一个换行之前的残片,余下按 lossy 解码。
/// 整段无换行(单行超预算)时不丢,否则会把唯一的诊断信息清空。
fn tail_to_string(bytes: &[u8]) -> String {
    let start = match bytes.iter().position(|&b| b == b'\n') {
        Some(i) if i + 1 < bytes.len() => i + 1,
        _ => 0,
    };
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}
