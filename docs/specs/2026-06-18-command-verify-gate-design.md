# command verify gate:用确定性命令(exit code)做校验门(设计)

日期 2026-06-18。借鉴 Microsoft Conductor 的 script step(`run pytest between implement and review`)。配套调研见 `docs/research/2026-06-18-ccswarm-conductor-comparison.md` §2。

## 1. 问题

`verify` 门(`docs/specs/2026-06-17-verify-gate-design.md` 的 A1)已落地,`Verifier` 现有两个变体:

- `by: codex` —— codex review 输出结构化 `Verdict`。
- `by: claude` —— claude 只读判定,末行 `VERDICT: pass|fail`。

两者都是 **LLM 判定**:让模型读 diff / 文档去判"目标达成没有"。但很多"达成"判据本就是**确定性**的:测试绿、`cargo build` 过、lint 干净、某文件存在。对这类判据:

- 让 codex/claude 读 diff 再判,又慢(多烧一次 LLM 调用)、又可能误判(模型读漏)、还不稳定(同输入不同结论)。
- 命令的退出码是现成的、零成本、可重放的 pass/fail 信号,且天然 fail-closed。

目标:给 verify 门加第三个 `Verifier::Command` 变体,**子进程退出码即 verdict**,复用现有重试 / on_unmet / feedback 全套环路。

## 2. 现有可复用资产(已落地)

- `manifest.rs`:`Verify { by: Verifier, action, base, path, prompt, skill, max_retries, on_unmet, feedback }`,`Verifier` enum,`OnUnmet { Gate, Fail, Continue }`,`MAX_VERIFY_RETRIES` 上限。
- `executor.rs::verify_once(&self, v: &Verify, on_line)` -> `(Verdict, String)`:按 `v.by` 分派,Claude 走 `claude.run` 解析 `VERDICT:`,Codex 走 `codex.review`。
- `executor.rs` claude 干活分支(L152-248):跑完 → `verify_once` → `Met` finish / `Unmet` 按 `attempt < max_retries` 重试(注入 feedback)/ 耗尽走 `on_unmet`。
- runner 子进程封装与 `ctx.cwd`(target 仓库目录),命令在此 cwd 跑。

新变体寄生这一整套,**不动控制流,只加一个分派 arm + 一个 runner 方法 + schema 字段**。

## 3. Schema 设计

`Verifier` 加 `Command` 变体;`Verify` 加一个可选 `command` 字段(仅 `by: command` 用)。

```yaml
- id: implement
  kind: claude
  prompt: "按执行文档实现…"
  verify:
    by: command               # codex | claude | command(新)
    command: "cargo test"     # 在 target 仓库 cwd 下执行;exit 0 = 达成
    max_retries: 2            # 复用:未达成时重跑干活步骤上限
    on_unmet: gate            # 复用:gate(默认) | fail | continue
    feedback: true            # 复用:未达成时把命令输出尾部作为反馈注入重试 prompt
```

判定语义(fail-closed):

| 命令结果 | verdict |
|---|---|
| exit code == 0 | Met(达成) |
| exit code != 0 | Unmet(未达成),findings = 输出尾部 |
| spawn 失败 / 命令不存在 / 信号杀死 | Unmet,findings = 错误原因 |

> 与 codex-clean 同一信条:判定侧任何异常都走最保守分支(Unmet),绝不静默判达成。

Rust 类型变更(`manifest.rs`):

```rust
pub enum Verifier {
    Codex,
    Claude,
    Command,                       // 新
}

pub struct Verify {
    pub by: Verifier,
    // ... 现有 codex/claude 字段不变 ...
    #[serde(default)]
    pub command: Option<String>,   // 新,仅 by: command 用
    // max_retries / on_unmet / feedback 不变
}
```

命令执行约定(Phase 1,保持最小):

- 整条 `command` 字符串经 shell 执行(`sh -c "<command>"`,Windows 留待需要时再议),与现有 CLI 黑盒调用同一假设。
- cwd = `ctx.cwd`(target 仓库);env 继承当前进程。
- **不支持** `{{step.field}}` 插值(Phase 1);命令是固定校验,需要时 Phase 2 再加,与现有 codex/claude verify 的 path/prompt 插值对齐。
- 输出捕获:stdout + stderr 合并,尾部截断(建议 4KB,与日志预览同量级)作为 findings。

## 4. Executor 语义

`verify_once` 加一个 `Verifier::Command` 分派 arm(其余分支不动):

```rust
fn verify_once(&self, v: &Verify, on_line: &mut dyn FnMut(&str, Option<u32>)) -> (Verdict, String) {
    match v.by {
        Verifier::Codex  => { /* 现状不变 */ }
        Verifier::Claude => { /* 现状不变 */ }
        Verifier::Command => {
            let cmd = match &v.command {
                Some(c) if !c.trim().is_empty() => c,
                _ => return (Verdict::ChangesRequested, "verify command 缺 command 字段".into()),
            };
            on_line(&format!("校验命令: {cmd}"), None);
            // 返回 (exit_code, 合并输出尾部);Err = spawn/IO 失败
            match run_verify_command(cmd, &self.ctx.cwd, self.control.as_ref()) {
                Ok((0, _))       => (Verdict::Clean, String::new()),
                Ok((code, tail)) => (Verdict::ChangesRequested, format!("命令退出码 {code}\n{tail}")),
                Err(e)           => (Verdict::ChangesRequested, format!("校验执行失败: {e}")),
            }
        }
    }
}
```

`run_verify_command(cmd, cwd, control) -> Result<(i32, String)>`:

- `Command::new("sh").arg("-c").arg(cmd).current_dir(cwd)`,stdout+stderr 合并捕获,返回 `(exit_code, 输出尾部 ≤4KB)`。被信号杀死(无 exit code)按 `Err` 处理 → fail-closed。
- **可中断性**:与现有 runner 一致 —— spawn 前查 `control.is_aborted()`;spawn 后把子进程 pgid 注册到 `control.set_current(...)`,使 abort 能 `control.kill_current()` 杀掉长命令(`cargo test` 这类不会卡死整个 run)。`claude.run` / `codex.review` 现在正是这么做的(都收 `Some(self.control.as_ref())`),command 不得偷懒只查 flag。
- 错误文案沿用现有 verify_once 的"校验执行失败"(executor.rs:307/325),不另造一套。

下游不变:返回 `Verdict::ChangesRequested` 即触发现有 `Unmet` 路径 —— `attempt < max_retries` 带 feedback 重跑干活步骤,耗尽走 `on_unmet`。

## 5. 校验(manifest.rs validate)

claude verify 的校验分支加 command 判定:

- `by: command` → `command` 必填且非空;不得同时塞 codex/claude 专属字段(给 warning 或忽略,Phase 1 取忽略,文档注明)。
- `by: codex` → 仍要求 `action`(现状)。
- `by: claude` → 仍要求 `prompt`(现状)。
- `max_retries <= MAX_VERIFY_RETRIES`(现状,通用)。

## 6. 信任边界

verify command 在 target 仓库以当前进程权限跑任意 shell。这与 AgentPipe 既有假设同级:task.yaml 本就是用户自己写的、claude 一律 bypassPermissions、codex 全仓读写。command 来自**用户自己的 task.yaml**,信任级别与其中的 prompt 完全一致,**不引入新信任面**,无需额外确认。文档需点明:不要把不可信来源的 task.yaml 直接 run(此条对整个 AgentPipe 都成立,非本特性新增)。

## 7. 测试

- 单测 `verify_once` 的 command arm:exit 0 → Clean;exit 1 → ChangesRequested + findings 含输出;`sh -c "nonexistent_cmd_xyz"` → ChangesRequested(fail-closed)。
- 集成(stub 不需要):一个 task.yaml,implement 步骤 `verify: { by: command, command: "test -f expected.txt", max_retries: 1 }`,首轮文件不存在 → 重试一次 → 仍不存在 → on_unmet。用临时目录 + 真实 `sh` 跑(符合全局基线"真实进程 smoke 覆盖")。
- validate:`by: command` 缺 `command` → 报错带 message。

## 8. 非目标

- `{{step.field}}` 插值进 command(Phase 2 按需)。
- Windows shell 适配(目前 macOS/unix 优先,libc 已 cfg(unix))。
- 命令超时(归口到"整 run/单步 wall-clock timeout"那条独立 backlog,不在本 spec)。
- 把 codex-loop / codex verify 用 command 重表达(两者判据不同,互补共存)。

## 9. 与既有设计的关系

- codex-clean loop、codex verify、claude verify、command verify 是**四种收敛信号源**,共享同一条重试/gate 机制。command 是其中唯一确定性、零 LLM 成本的一种,适合"测试/构建/lint"类硬判据;LLM verifier 适合"设计合理性/代码味道"类软判据。选型原则:**能用命令判的优先用命令**(快、准、稳、省钱)。
