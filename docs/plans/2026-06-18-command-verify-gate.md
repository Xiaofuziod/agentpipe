# command verify gate 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 verify 门加第三个 verifier `by: command`,用 shell 命令退出码做确定性校验(exit 0 = 达成)。

**Architecture:** 复用现有 `runner::run_command`(已带进程组 / pgid 登记 / abort kill / 行回调),command verifier 只是它的一个调用点;判据走现有 `Verdict` + 重试 / `on_unmet` / feedback 环,不动控制流。

**Tech Stack:** Rust(engine crate),serde / serde_yml,无新依赖。

设计依据:`docs/specs/2026-06-18-command-verify-gate-design.md`。

## Global Constraints

- 判定 fail-closed:命令非零退出 / spawn 失败 / 被信号杀死,一律 `Verdict::ChangesRequested`,绝不静默判达成。
- 错误文案沿用现有 `verify_once` 的 `"校验执行失败: {e}"`(executor.rs:307/325),不另造。
- 命令以 `sh -c "<cmd> 2>&1"` 执行,cwd = `self.ctx.cwd`,stderr 并入 stdout(既捕获进 findings 又经 `on_line` 实时流)。
- `verify.max_retries` 硬上限 `MAX_VERIFY_RETRIES = 10` 不变,command verifier 同样受约束。
- 测试放对应文件的 `#[cfg(test)] mod tests`(manifest.rs 需新建该模块;executor.rs 已有,在文件末尾)。
- 提交信息用中文。

---

### Task 1: manifest schema + 校验

**Files:**
- Modify: `crates/engine/src/manifest.rs`(Verifier enum + Verify struct + validate_step 的 Claude/verify 分支)
- Test: `crates/engine/src/manifest.rs` 内新建 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:`Verifier::Command` 变体(serde 序列化为 `"command"`);`Verify.command: Option<String>` 字段。
- Consumes:无(纯 schema)。

- [ ] **Step 1: 写失败测试(新建 mod tests)**

在 `crates/engine/src/manifest.rs` 文件末尾追加:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn yaml_with_verify(verify_block: &str) -> String {
        format!(
            "version: 1\nname: t\ntarget: /tmp\nmode: auto\nsteps:\n  - id: impl\n    kind: claude\n    prompt: \"do\"\n    verify:\n{verify_block}"
        )
    }

    #[test]
    fn command_verify_requires_command() {
        let y = yaml_with_verify("      by: command\n");
        let m = Manifest::parse(&y).unwrap();
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("command"), "err = {err}");
    }

    #[test]
    fn command_verify_ok_with_command() {
        let y = yaml_with_verify("      by: command\n      command: \"cargo test\"\n");
        let m = Manifest::parse(&y).unwrap();
        assert!(m.validate().is_ok());
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p agentpipe-engine command_verify`
Expected: 编译失败 —— `Verifier` 无 `Command` 变体 / `Verify` 无 `command` 字段(序列化 `by: command` 报未知变体)。

- [ ] **Step 3: 加 Command 变体 + command 字段**

在 `Verify` struct 内(`skill` 字段后、`max_retries` 前)加:

```rust
    /// command verifier 的 shell 命令(仅 by: command 用);exit 0 = 达成。
    #[serde(default)]
    pub command: Option<String>,
```

在 `Verifier` enum 加 `Command` 变体:

```rust
#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Verifier {
    Codex,
    /// claude 只读判定(`--permission-mode plan`),回复末行 `VERDICT: pass|fail`。
    Claude,
    /// shell 命令判定:exit 0 = 达成,否则未达成。findings = 输出尾部。
    Command,
}
```

- [ ] **Step 4: 在 validate_step 的 verify 分支加 Command 校验**

在 `validate_step` 的 `StepKind::Claude { verify, .. }` 分支里,`match v.by { ... }` 内 `Verifier::Claude => {...}` 之后加:

```rust
                        Verifier::Command => {
                            if v.command.as_deref().map(str::trim).unwrap_or("").is_empty() {
                                return Err(EngineError::Validation(format!(
                                    "step '{}': verify by command 需要 command 字段(shell 命令)",
                                    step.id
                                )));
                            }
                        }
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p agentpipe-engine command_verify`
Expected: PASS(2 个测试)。

- [ ] **Step 6: 提交**

```bash
git add crates/engine/src/manifest.rs
git commit -m "feat(engine): verify 门加 by:command schema + 校验"
```

---

### Task 2: executor command verifier 执行

**Files:**
- Modify: `crates/engine/src/executor.rs`(新增 free fn `command_verdict` + `tail`;`verify_once` 加 `Verifier::Command` arm)
- Test: `crates/engine/src/executor.rs` 末尾已有的 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes:`runner::run_command(bin, args, cwd, stdin, timeout, control, on_line) -> Result<(String, bool)>`(已存在);`Verifier::Command`、`Verify.command`(Task 1)。
- Produces:`fn command_verdict(cmd: &str, cwd: &Path, control: &Control, on_line: &mut dyn FnMut(&str)) -> (Verdict, String)`;`fn tail(s: &str, max_bytes: usize) -> String`。

- [ ] **Step 1: 写失败测试**

在 `crates/engine/src/executor.rs` 末尾 `mod tests` 内,`use super::parse_verdict;` 旁补 use 并追加测试:

```rust
    use super::{command_verdict, tail};
    use crate::control::Control;
    use std::path::Path;

    #[test]
    fn command_verdict_exit_zero_is_clean() {
        let ctrl = Control::default();
        let (v, f) = command_verdict("exit 0", Path::new("."), &ctrl, &mut |_l| {});
        assert!(matches!(v, Verdict::Clean));
        assert!(f.is_empty());
    }

    #[test]
    fn command_verdict_nonzero_is_changes_with_output() {
        let ctrl = Control::default();
        let (v, f) = command_verdict("echo boom; exit 1", Path::new("."), &ctrl, &mut |_l| {});
        assert!(matches!(v, Verdict::ChangesRequested));
        assert!(f.contains("boom"));
    }

    #[test]
    fn command_verdict_missing_binary_fail_closed() {
        let ctrl = Control::default();
        let (v, _f) = command_verdict("definitely_not_a_real_cmd_xyz", Path::new("."), &ctrl, &mut |_l| {});
        assert!(matches!(v, Verdict::ChangesRequested));
    }

    #[test]
    fn tail_keeps_suffix_on_char_boundary() {
        assert_eq!(tail("hello", 100), "hello");
        assert_eq!(tail("abcdefgh", 3), "fgh");
        // 多字节字符不被切碎
        let s = "藏字符串末尾"; // 每个汉字 3 字节
        let t = tail(s, 4); // 4 字节落在某汉字中间,应向后对齐到边界
        assert!(s.ends_with(&t));
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p agentpipe-engine command_verdict`
Expected: 编译失败 —— `command_verdict` / `tail` 未定义。

- [ ] **Step 3: 实现 command_verdict + tail**

在 `executor.rs` 的 `parse_verdict` free fn 之后(`#[cfg(test)]` 之前)加:

```rust
/// 用 shell 命令做校验门:exit 0 = 达成(Clean),否则 ChangesRequested(findings = 输出尾部)。
/// 复用 runner::run_command(进程组 / pgid 登记 / abort kill 已就绪);`2>&1` 把 stderr 并进捕获。
/// spawn / IO 失败、被信号杀死一律 fail-closed 为 ChangesRequested。
fn command_verdict(
    cmd: &str,
    cwd: &std::path::Path,
    control: &crate::control::Control,
    on_line: &mut dyn FnMut(&str),
) -> (Verdict, String) {
    let shell_cmd = format!("{cmd} 2>&1");
    match crate::runner::run_command(
        "sh",
        &["-c".into(), shell_cmd],
        cwd,
        None,
        None,
        Some(control),
        on_line,
    ) {
        Ok((_, true)) => (Verdict::Clean, String::new()),
        Ok((out, false)) => (Verdict::ChangesRequested, tail(&out, 4096)),
        Err(e) => (Verdict::ChangesRequested, format!("校验执行失败: {e}")),
    }
}

/// 取字符串末 max_bytes 字节,向后对齐到 char 边界(不切碎 UTF-8)。
fn tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p agentpipe-engine command_verdict && cargo test -p agentpipe-engine tail`
Expected: PASS(4 个测试)。

- [ ] **Step 5: 在 verify_once 接入 Command arm**

在 `verify_once` 的 `match v.by { ... }` 内,`Verifier::Claude => {...}` 之后加:

```rust
            Verifier::Command => {
                let cmd = match &v.command {
                    Some(c) if !c.trim().is_empty() => c.clone(),
                    _ => return (Verdict::ChangesRequested, "verify command 缺 command 字段".into()),
                };
                on_line("校验命令…", None);
                let mut fwd = |l: &str| on_line(l, None);
                command_verdict(&cmd, &self.ctx.cwd, self.control.as_ref(), &mut fwd)
            }
```

- [ ] **Step 6: 全量编译 + 测试**

Run: `cargo build && cargo test -p agentpipe-engine`
Expected: 编译通过,engine 全部测试 PASS。

- [ ] **Step 7: 真实 smoke(端到端,符合全局基线)**

手动验证一次(不入自动化,验完即弃):

```bash
cargo build
mkdir -p /tmp/cv-demo && rm -f /tmp/cv-demo/marker
cat > /tmp/cv-demo/task.yaml <<'EOF'
version: 1
name: "command verify smoke"
target: /tmp/cv-demo
mode: auto
steps:
  - id: impl
    kind: claude
    prompt: "无关紧要,stub 会直接成功"
    verify:
      by: command
      command: "test -f marker"
      max_retries: 1
      on_unmet: fail
EOF
AGENTPIPE_CLAUDE_BIN=$PWD/tests/fixtures/stub-claude.sh \
AGENTPIPE_CODEX_BIN=$PWD/tests/fixtures/stub-codex.sh \
./target/debug/agentpipe run /tmp/cv-demo/task.yaml
```

Expected: marker 不存在 → 校验未通过 → 重试 1 次 → 仍不存在 → `on_unmet: fail` 该步失败。
再 `touch /tmp/cv-demo/marker` 后重跑 → 校验通过、步骤成功。

- [ ] **Step 8: 提交**

```bash
git add crates/engine/src/executor.rs
git commit -m "feat(engine): command verifier 执行(复用 run_command,fail-closed)"
```

---

## Self-Review(写完后核对 spec)

- spec §3 schema(Verifier::Command + command 字段)→ Task 1 ✅
- spec §4 executor 语义(复用 run_command / success 判据 / 2>&1 / tail / fail-closed)→ Task 2 ✅
- spec §5 validate(command 必填)→ Task 1 Step 4 ✅
- spec §6 信任边界 → 无代码,文档已述,plan 无需任务 ✅
- spec §7 测试(exit0/exit1/坏命令/validate)→ Task 1+2 测试 ✅
- spec §8 非目标(插值 / Windows / 超时)→ 不实现,plan 不含 ✅
- 类型一致性:`command_verdict` / `tail` 签名在 Task 2 Interfaces 与实现一致;`Verifier::Command` 在两任务一致 ✅
