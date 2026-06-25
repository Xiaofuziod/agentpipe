use crate::context::{RunContext, StepOutput, Verdict};
use crate::control::Control;
use crate::manifest::{Manifest, OnUnmet, RunMode, Step, StepKind, Verifier, Verify};
use crate::protocol::{Command, Event, GateKind, RunStatus, StepMetrics, StepStatus};
use crate::runner::claude::ClaudeRunner;
use crate::runner::codex::CodexRunner;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

pub struct RunnerBins {
    pub claude: String,
    pub codex: String,
}

enum StepDecision {
    Retry,
    Skip,
    Abort,
}

pub struct Executor {
    manifest: Manifest,
    ctx: RunContext,
    claude: ClaudeRunner,
    codex: CodexRunner,
    control: Arc<Control>,
    events: Sender<Event>,
    commands: Receiver<Command>,
}

impl Executor {
    pub fn new(
        manifest: Manifest,
        bins: RunnerBins,
        control: Arc<Control>,
        events: Sender<Event>,
        commands: Receiver<Command>,
    ) -> Self {
        let ctx = RunContext::new(manifest.target.clone());
        Self {
            claude: ClaudeRunner::new(bins.claude),
            codex: CodexRunner::new(bins.codex),
            manifest,
            ctx,
            control,
            events,
            commands,
        }
    }

    pub fn run(&mut self) -> RunStatus {
        let _ = self.events.send(Event::RunStarted {
            name: self.manifest.name.clone(),
            target: self.manifest.target.display().to_string(),
        });
        // 隔离 worktree:开启则在跑任何 step 前建好,把 cwd 指向它。失败 fail-closed 终止,
        // 绝不退回 target 原地跑(否则隔离形同虚设)。
        if self.manifest.worktree {
            match crate::worktree::create(&self.manifest.target, &self.manifest.name) {
                Ok(wt) => {
                    let _ = self.events.send(Event::WorktreeReady {
                        path: wt.path.display().to_string(),
                        branch: wt.branch,
                    });
                    self.ctx.cwd = wt.path;
                }
                Err(e) => {
                    // 取内层 message,避免 "worktree 创建失败: worktree error: …" 双前缀
                    let error = match e {
                        crate::error::EngineError::Worktree(m) => m,
                        other => other.to_string(),
                    };
                    let _ = self.events.send(Event::WorktreeFailed { error });
                    let _ = self.events.send(Event::RunFinished {
                        status: RunStatus::Failed,
                    });
                    return RunStatus::Failed;
                }
            }
        }
        let steps = std::mem::take(&mut self.manifest.steps);
        let gated = matches!(self.manifest.mode, RunMode::Step);
        for step in &steps {
            if self.run_step(step, gated).is_err() {
                // run_step 的 Err 只来自 Abort 路径(失败已在内部走决策 gate)
                let status = if self.control.is_aborted() {
                    RunStatus::Aborted
                } else {
                    RunStatus::Failed
                };
                let _ = self.events.send(Event::RunFinished {
                    status: status.clone(),
                });
                return status;
            }
        }
        let _ = self.events.send(Event::RunFinished {
            status: RunStatus::Success,
        });
        RunStatus::Success
    }

    fn run_step(&mut self, step: &Step, gated: bool) -> Result<(), ()> {
        if self.control.is_aborted() {
            return Err(());
        }

        // Loop 自身不门控(body 子步骤逐个门控);Human 自身已有 gate,不重复。
        if gated && !matches!(step.kind, StepKind::Loop { .. } | StepKind::Human { .. }) {
            let _ = self.events.send(Event::StepAwaitingGate {
                step_id: step.id.clone(),
                suggestion: format!("即将执行 step '{}'", step.id),
                expects_artifact: false,
                gate_kind: GateKind::Step,
            });
            match self.commands.recv() {
                Ok(Command::ApproveGate { .. }) => {}
                Ok(Command::SkipStep { .. }) => {
                    self.emit_skipped(&step.id);
                    return Ok(());
                }
                _ => {
                    self.fail(&step.id, "aborted".into());
                    return Err(());
                }
            }
        }

        // Loop 自身不发 StepStarted —— 它没有对应的 StepFinished,宿主按 step_id 去重渲染时
        // 会把它显示成永久 "运行中" 的幽灵步骤(GUI 控制台见过)。loop 生命周期由
        // LoopIteration / LoopConverged / LoopMaxReached 表达,body 子步骤各自发 Started/Finished。
        let kind_name = match &step.kind {
            StepKind::Claude { .. } => Some("claude"),
            StepKind::Codex { .. } => Some("codex"),
            StepKind::Human { .. } => Some("human"),
            StepKind::Loop { .. } => None,
        };
        if let Some(kind) = kind_name {
            let _ = self.events.send(Event::StepStarted {
                step_id: step.id.clone(),
                kind: kind.into(),
            });
        }

        match &step.kind {
            StepKind::Codex { action, path, base, prompt } => {
                let path_i = path.as_ref().map(|p| self.ctx.interpolate(p));
                let base_i = base.as_ref().map(|b| self.ctx.interpolate(b));
                let prompt_i = prompt.as_ref().map(|p| self.ctx.interpolate(p));
                let mut on_line = self.progress_sink(&step.id);
                loop {
                    let res = self.codex.review(
                        action,
                        path_i.as_deref(),
                        base_i.as_deref(),
                        prompt_i.as_deref(),
                        Some(self.control.as_ref()),
                        &mut on_line,
                        &self.ctx.cwd,
                    );
                    match res {
                        Ok(out) => {
                            let summary = format!("verdict={:?}", out.verdict);
                            self.ctx.record(&step.id, StepOutput {
                                findings: Some(out.findings),
                                verdict: Some(out.verdict),
                                ..Default::default()
                            });
                            self.finish(&step.id, summary, None);
                            return Ok(());
                        }
                        Err(e) => match self.handle_failure(&step.id, e.to_string()) {
                            StepDecision::Retry => continue,
                            StepDecision::Skip => {
                                self.emit_skipped(&step.id);
                                return Ok(());
                            }
                            StepDecision::Abort => return Err(()),
                        },
                    }
                }
            }
            StepKind::Claude { prompt, skill, verify } => {
                let mut on_line = self.progress_sink(&step.id);
                let mut attempt = 0u32; // 校验重试计数(与失败重试独立)
                let mut feedback: Option<String> = None;
                loop {
                    let mut p = self.ctx.interpolate(prompt);
                    if let Some(f) = &feedback {
                        p.push_str(&format!("\n\n上一轮校验反馈:\n{f}\n请据此修正后重做。"));
                    }
                    let out = match self.claude.run(
                        &p,
                        skill.as_deref(),
                        Some(self.control.as_ref()),
                        &mut on_line,
                        &self.ctx.cwd,
                        false, // 干活步骤可写
                    ) {
                        Ok(out) => out,
                        Err(e) => match self.handle_failure(&step.id, e.to_string()) {
                            StepDecision::Retry => continue,
                            StepDecision::Skip => {
                                self.emit_skipped(&step.id);
                                return Ok(());
                            }
                            StepDecision::Abort => return Err(()),
                        },
                    };
                    let metrics = out.metrics;
                    let answer = out.answer;
                    self.ctx.record(&step.id, StepOutput {
                        artifact: Some(answer.clone()),
                        ..Default::default()
                    });

                    // 无校验门 → 退出码即完成(原行为)
                    let v = match verify {
                        None => {
                            self.finish(&step.id, "done".into(), metrics);
                            return Ok(());
                        }
                        Some(v) => v,
                    };
                    on_line("校验中…", None);
                    let (verdict, findings) = self.verify_once(v, &mut on_line);
                    // 暴露 verifier findings 供下游 {{<id>.findings}} 引用
                    self.ctx.record(&step.id, StepOutput {
                        artifact: Some(answer.clone()),
                        findings: Some(findings.clone()),
                        ..Default::default()
                    });
                    if matches!(verdict, Verdict::Clean) {
                        on_line("校验通过", None);
                        self.finish(&step.id, "done · 已校验".into(), metrics);
                        return Ok(());
                    }
                    // 未达成:还有重试预算就带反馈重跑
                    if attempt < v.max_retries {
                        attempt += 1;
                        on_line(&format!("校验未通过,第 {attempt} 次重试"), None);
                        feedback = if v.feedback { Some(findings) } else { None };
                        continue;
                    }
                    // 重试耗尽 → 升级策略
                    match v.on_unmet {
                        OnUnmet::Continue => {
                            self.finish(&step.id, "done · 未达标(continue)".into(), metrics);
                            return Ok(());
                        }
                        OnUnmet::Fail => {
                            self.fail(&step.id, format!("校验未通过(已重试 {} 次)", v.max_retries));
                            return Err(());
                        }
                        OnUnmet::Gate => {
                            let suggestion = format!(
                                "校验未通过(重试 {} 次仍未达标),选择 重试 / 跳过 / 中止\n{findings}",
                                v.max_retries
                            );
                            match self.decision_gate(&step.id, suggestion) {
                                StepDecision::Retry => {
                                    attempt = 0; // 人工再批一次,给新预算
                                    feedback = if v.feedback { Some(findings) } else { None };
                                    continue;
                                }
                                StepDecision::Skip => {
                                    self.emit_skipped(&step.id);
                                    return Ok(());
                                }
                                StepDecision::Abort => return Err(()),
                            }
                        }
                    }
                }
            }
            StepKind::Human { instruction, expects, value } => {
                let instr = self.ctx.interpolate(instruction);
                self.run_human(step, &instr, expects.is_some(), value.as_deref())
            }
            StepKind::Loop { until, max, body } => self.run_loop(&step.id, until, *max, body, gated),
        }
    }

    /// 发一个决策 gate(重试/跳过/中止)并阻塞等宿主指令。已中止则直接 Abort 不弹门。
    /// 失败重试与校验未达成共用这条通道。
    fn decision_gate(&self, step_id: &str, suggestion: String) -> StepDecision {
        if self.control.is_aborted() {
            return StepDecision::Abort;
        }
        let _ = self.events.send(Event::StepAwaitingGate {
            step_id: step_id.to_string(),
            suggestion,
            expects_artifact: false,
            gate_kind: GateKind::Decision,
        });
        match self.commands.recv() {
            Ok(Command::ApproveGate { .. }) => StepDecision::Retry,
            Ok(Command::SkipStep { .. }) => StepDecision::Skip,
            _ => StepDecision::Abort,
        }
    }

    /// step 失败(进程非零)的处理:发 StepFailed 后走决策 gate。
    fn handle_failure(&mut self, step_id: &str, err: String) -> StepDecision {
        if self.control.is_aborted() {
            return StepDecision::Abort;
        }
        let _ = self.events.send(Event::StepFailed {
            step_id: step_id.to_string(),
            error: err,
        });
        self.decision_gate(step_id, "step 失败,选择 重试 / 跳过 / 中止".into())
    }

    /// 跑一次校验门,返回 (verdict, findings)。verifier 自身失败/不可解析一律
    /// fail-closed 为 ChangesRequested(绝不静默判过)。
    fn verify_once(&self, v: &Verify, on_line: &mut dyn FnMut(&str, Option<u32>)) -> (Verdict, String) {
        match v.by {
            Verifier::Codex => {
                // action 必有(validate 已保证);防御性兜底 fail-closed。
                let action = match &v.action {
                    Some(a) => a,
                    None => return (Verdict::ChangesRequested, "verify codex 缺 action".into()),
                };
                let path_i = v.path.as_ref().map(|p| self.ctx.interpolate(p));
                let base_i = v.base.as_ref().map(|b| self.ctx.interpolate(b));
                let prompt_i = v.prompt.as_ref().map(|p| self.ctx.interpolate(p));
                match self.codex.review(
                    action,
                    path_i.as_deref(),
                    base_i.as_deref(),
                    prompt_i.as_deref(),
                    Some(self.control.as_ref()),
                    on_line,
                    &self.ctx.cwd,
                ) {
                    Ok(out) => (out.verdict, out.findings),
                    Err(e) => (Verdict::ChangesRequested, format!("校验执行失败: {e}")),
                }
            }
            Verifier::Claude => {
                let q = self.ctx.interpolate(v.prompt.as_deref().unwrap_or(""));
                let judge = format!(
                    "{q}\n\n请只读判定上述目标是否达成,不要修改任何文件。\
                     在回复最后单独一行输出 `VERDICT: pass`(达成)或 `VERDICT: fail`(未达成)。"
                );
                match self.claude.run(
                    &judge,
                    v.skill.as_deref(),
                    Some(self.control.as_ref()),
                    on_line,
                    &self.ctx.cwd,
                    true, // verifier 只读(plan 模式)
                ) {
                    Ok(out) => (parse_verdict(&out.answer), out.answer),
                    Err(e) => (Verdict::ChangesRequested, format!("校验执行失败: {e}")),
                }
            }
            Verifier::Command => {
                let cmd = match &v.command {
                    Some(c) if !c.trim().is_empty() => c.as_str(),
                    _ => return (Verdict::ChangesRequested, "verify command 缺 command 字段".into()),
                };
                on_line("校验命令…", None);
                command_verdict(cmd, &self.ctx.cwd, self.control.as_ref(), &mut |l| on_line(l, None))
            }
        }
    }

    fn run_human(
        &mut self,
        step: &Step,
        instruction: &str,
        expects_artifact: bool,
        seed: Option<&str>,
    ) -> Result<(), ()> {
        // 启动时预置了人工输入 → 直接记录为产物,跳过 gate(不阻塞 recv)。
        // 插值后为空视同未预置,回退到正常 gate(防止 GUI 漏填仍能人工补)。
        if let Some(raw) = seed {
            let val = self.ctx.interpolate(raw);
            if !val.trim().is_empty() {
                self.ctx.record(
                    &step.id,
                    StepOutput {
                        artifact: Some(val),
                        ..Default::default()
                    },
                );
                self.finish(&step.id, "approved (preset)".into(), None);
                return Ok(());
            }
        }
        let _ = self.events.send(Event::StepAwaitingGate {
            step_id: step.id.clone(),
            suggestion: instruction.to_string(),
            expects_artifact,
            gate_kind: GateKind::Human,
        });
        match self.commands.recv() {
            Ok(Command::ApproveGate { artifact, .. }) => {
                self.ctx.record(&step.id, StepOutput {
                    artifact,
                    ..Default::default()
                });
                self.finish(&step.id, "approved".into(), None);
                Ok(())
            }
            Ok(Command::SkipStep { .. }) => {
                self.emit_skipped(&step.id);
                Ok(())
            }
            _ => {
                self.fail(&step.id, "aborted".into());
                Err(())
            }
        }
    }

    fn run_loop(&mut self, loop_id: &str, until: &str, max: u32, body: &[Step], gated: bool) -> Result<(), ()> {
        for n in 1..=max {
            if self.control.is_aborted() {
                return Err(());
            }
            let _ = self.events.send(Event::LoopIteration {
                loop_id: loop_id.into(),
                iteration: n,
            });
            for sub in body {
                self.run_step(sub, gated)?;
            }
            if self.eval_until(until, body) {
                let _ = self.events.send(Event::LoopConverged {
                    loop_id: loop_id.into(),
                    iterations: n,
                });
                return Ok(());
            }
        }
        let _ = self.events.send(Event::LoopMaxReached {
            loop_id: loop_id.into(),
            max,
        });
        Ok(())
    }

    /// 目前只支持 codex-clean:找 body 里最后一个 codex step 的 verdict。
    fn eval_until(&self, until: &str, body: &[Step]) -> bool {
        if until != "codex-clean" {
            return false;
        }
        for sub in body.iter().rev() {
            if matches!(sub.kind, StepKind::Codex { .. }) {
                if let Some(out) = self.ctx.get(&sub.id) {
                    return matches!(out.verdict, Some(Verdict::Clean));
                }
            }
        }
        false // 没找到 codex step → fail-closed 不收敛
    }

    /// 生成一个把 CLI 进度(行 + 可选模型轮次)转成 StepProgress 事件的回调
    /// (克隆 events,不借 self)。claude 传实际轮次,codex 传 None。
    fn progress_sink(&self, step_id: &str) -> impl FnMut(&str, Option<u32>) {
        let events = self.events.clone();
        let sid = step_id.to_string();
        move |line: &str, round: Option<u32>| {
            let _ = events.send(Event::StepProgress {
                step_id: sid.clone(),
                line: line.to_string(),
                round,
            });
        }
    }

    fn finish(&self, step_id: &str, summary: String, metrics: Option<StepMetrics>) {
        let _ = self.events.send(Event::StepFinished {
            step_id: step_id.to_string(),
            status: StepStatus::Done,
            summary,
            metrics,
        });
    }

    fn emit_skipped(&self, step_id: &str) {
        let _ = self.events.send(Event::StepFinished {
            step_id: step_id.to_string(),
            status: StepStatus::Skipped,
            summary: "skipped".into(),
            metrics: None,
        });
    }

    fn fail(&self, step_id: &str, error: String) {
        let _ = self.events.send(Event::StepFailed {
            step_id: step_id.to_string(),
            error,
        });
    }
}

/// 从 claude verifier 的回复里解析 `VERDICT: pass|fail`(末行优先)。
/// 找不到 / 非 pass 一律 fail-closed 为 ChangesRequested(绝不静默判过)。
fn parse_verdict(answer: &str) -> Verdict {
    for line in answer.lines().rev() {
        if let Some(rest) = line.trim().strip_prefix("VERDICT:") {
            return if rest.trim().eq_ignore_ascii_case("pass") {
                Verdict::Clean
            } else {
                Verdict::ChangesRequested
            };
        }
    }
    Verdict::ChangesRequested
}

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

#[cfg(test)]
mod tests {
    use super::{command_verdict, parse_verdict, tail};
    use crate::context::Verdict;
    use crate::control::Control;
    use std::path::Path;

    #[test]
    fn parse_verdict_reads_sentinel() {
        assert!(matches!(parse_verdict("分析…\nVERDICT: pass"), Verdict::Clean));
        assert!(matches!(parse_verdict("VERDICT: PASS"), Verdict::Clean));
        assert!(matches!(parse_verdict("VERDICT: fail\n收尾"), Verdict::ChangesRequested));
    }

    #[test]
    fn parse_verdict_fail_closed_without_sentinel() {
        assert!(matches!(parse_verdict("没有判定行"), Verdict::ChangesRequested));
        assert!(matches!(parse_verdict(""), Verdict::ChangesRequested));
    }

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
    fn command_verdict_spawn_failure_fail_closed() {
        let ctrl = Control::default();
        // 不存在的 cwd → spawn 失败 → run_command 返回 Err → 走 "校验执行失败" 分支
        let (v, f) = command_verdict("echo hi", Path::new("/nonexistent/agentpipe/xyz"), &ctrl, &mut |_l| {});
        assert!(matches!(v, Verdict::ChangesRequested));
        assert!(f.contains("校验执行失败"), "Err 分支文案不对: {f}");
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
}
