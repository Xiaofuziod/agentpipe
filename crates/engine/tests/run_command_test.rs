//! run_command 的 stderr 处置。
//!
//! 背景(docs/specs/2026-07-10-codex-stderr-eagain-design.md):stderr 曾 inherit 宿主,
//! GUI 下宿主 stderr 是非阻塞管道,子进程狂写 stderr 触发 EAGAIN → 子进程 panic 自杀,
//! 引擎只看到"输出没了"。改为捕获后,必须持续排空,否则管道写满换成子进程阻塞。

mod common;

use agentpipe_engine::runner::run_command;
use common::fixture;
use std::path::PathBuf;

/// 子进程写入远超管道容量(macOS 默认 64KB)的 stderr:
/// ① 不得死锁(排空) ② stderr 被捕获而非漏给宿主 ③ 保留尾部供诊断。
#[test]
fn drains_child_stderr_beyond_pipe_capacity_and_captures_tail() {
    let out = run_command(
        &fixture("stub-stderr-flood.sh"),
        &[],
        &PathBuf::from("."),
        None,
        Some(30),
        None,
        &mut |_: &str| {},
    )
    .expect("run_command 不该返回 Err");

    assert!(
        out.success,
        "子进程应正常退出;stderr 未排空则它阻塞在 write,到点被 kill → success=false"
    );
    assert!(out.stdout.contains("stdout-marker"), "stdout 仍须完整捕获");
    assert!(
        out.stderr_tail.contains("stderr-flood-2000"),
        "stderr 尾部(最后写入的行)必须保留,那里才是崩溃原因"
    );
}

/// 尾部有预算上限:22k 行 diff 不能整份塞进 EngineError / audit。
#[test]
fn stderr_tail_is_budgeted_not_unbounded() {
    let out = run_command(
        &fixture("stub-stderr-flood.sh"),
        &[],
        &PathBuf::from("."),
        None,
        Some(30),
        None,
        &mut |_: &str| {},
    )
    .expect("run_command 不该返回 Err");

    // stub 写了约 160KB;尾部保留必须远小于此。
    assert!(
        out.stderr_tail.len() <= 16 * 1024,
        "stderr 尾部应截断到预算内,实际 {} 字节",
        out.stderr_tail.len()
    );
    assert!(
        !out.stderr_tail.contains("stderr-flood-1 "),
        "超预算的开头部分应被丢弃,只留尾部"
    );
}
