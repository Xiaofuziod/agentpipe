# codex 因宿主 stderr EAGAIN 崩溃 → review 活锁

日期：2026-07-10
状态：设计
影响：crates/engine/src/runner/mod.rs、runner/codex.rs、runner/claude.rs

## 1. 现象

GUI（`cargo tauri dev`）里跑 `mr-review-loop`，每轮 review 都渲染
`(无法解析 Codex 输出,按需修改处理)`，verdict 恒为 changes_requested，
`until: codex-clean` 永不收敛，循环跑到 `max: 10`。CLI 下同一 manifest 正常。

## 2. 根因

宿主 stderr 被子进程继承（`runner/mod.rs:46` 的 `Stdio::inherit()`）。
GUI 场景下宿主 stderr 是 cargo / tauri-cli 转发的非阻塞管道。codex review 时把
整个 `git diff` 写进 stderr（实测 22168 行），管道写满后返回 EAGAIN，codex 侧
panic 自杀：

```
thread 'codex-main' panicked at library/std/src/io/stdio.rs:1165:9:
failed printing to stderr: Resource temporarily unavailable (os error 35)
```

codex 死在输出最终结构化消息之前，于是 stdout 为空、`-o` 文件不写。

CLI 下宿主 stderr 是终端（阻塞写），永不 EAGAIN，故该缺陷只在 GUI 暴露。

## 3. 放大缺陷

三处叠加，把"子进程崩溃"伪装成"审查结论"：

| 位置 | 问题 |
|---|---|
| runner/mod.rs:46 | stderr 继承宿主，既触发 EAGAIN，又让崩溃原因不进 audit / UI |
| runner/codex.rs:297 | `let (stdout, _success)` 丢弃退出码，非零退出不 fail-loud |
| runner/codex.rs:528 | parse_review 把"进程失败"与"输出格式错"压成同一 fallback |

结果：fix 步骤拿着 `(无法解析 Codex 输出,按需修改处理)` 这条空 finding，
驱动 claude 以 bypassPermissions 在真实仓库自主写码，每轮一次，直到 max。

对照：`runner/claude.rs:85` 对 `!success` 是 fail-closed 返回 Err。
两个 runner 语义本就不对偶，codex 侧是唯一的宽松分支。

## 4. 方案

### 候选 A（采纳）：捕获 stderr + 收紧退出码

run_command 把 stderr 改为 `Stdio::piped()`，起独立 reader 线程持续排空，分块经
channel 送回主线程；主线程按字节预算滚动保留尾部（丢头留尾），返回给调用方。
codex / claude 在失败路径把这段尾部带进 EngineError。

reader 走 channel 而非共享 buffer，是为了和 stdout reader 同构：超时路径不 join
它，主线程返回后接收端 drop，线程下次 send 失败即自行结束。共享 buffer 没有这条
退出通道，会把线程永久漏在 read 上。

codex 侧退出码语义收紧为：

| stdout 可解析 | 退出码 | 行为 |
|---|---|---|
| 是 | 任意 | 采用解析结果（不因非零退出丢弃） |
| 否 | 0 | 保持既有 fallback（changes_requested + 无法解析） |
| 否 | 非零 | fail-loud 返回 Err，携带 stderr 尾部 |

保留"解析失败且退出 0 → fallback"是为了不破坏既有语义
（stub-codex-malformed-finding.sh 退出 0，测试期望 fallback 而非 Err）。
只有"既没有可用输出、进程又异常退出"才升级为 step 失败。

### 候选 B：stderr 直接 Stdio::null()

杜绝 EAGAIN，改动最小。否决：彻底丢失诊断信息，用户永远看不到 codex 为什么
失败，与"校验失败的报错要可解释"基线冲突。

### 候选 C：把宿主 stderr 改回阻塞（fcntl 清 O_NONBLOCK）

治标。宿主 stderr 归 Tauri / cargo 管，改它有跨进程副作用；Windows 无对应
语义；且仍会把 22k 行 diff 灌进宿主日志。否决。

## 5. 风险

stderr 改 piped 后必须持续排空，否则管道写满会让子进程阻塞在 write —— 换一种
死法。reader 线程与 stdout 同构：超时 kill 路径不 join，避免孙辈逃出进程组时
挂死引擎线程。

## 6. 验收

- 新增测试：子进程狂写 stderr（远超管道容量）不死锁，run_command 正常返回。
- 新增测试：codex 非零退出且无可解析输出 → Err，错误信息含 stderr 尾部。
- 新增测试：codex 非零退出但 stdout 有合法 JSON → 仍采用解析结果。
- 回归：`malformed_finding_missing_core_field_falls_back_to_changes_requested` 仍过。
- 全量 `cargo test` 绿。
