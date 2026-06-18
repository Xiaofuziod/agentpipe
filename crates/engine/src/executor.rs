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
        });
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

        let kind_name = match &step.kind {
            StepKind::Claude { .. } => "claude",
            StepKind::Codex { .. } => "codex",
            StepKind::Human { .. } => "human",
            StepKind::Loop { .. } => "loop",
        };
        let _ = self.events.send(Event::StepStarted {
            step_id: step.id.clone(),
            kind: kind_name.into(),
        });

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
            StepKind::Human { instruction, expects } => {
                let instr = self.ctx.interpolate(instruction);
                self.run_human(step, &instr, expects.is_some())
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
                // Task 2: command verifier runtime wiring (not in scope for Task 1)
                (Verdict::ChangesRequested, "command verifier not yet implemented".into())
            }
        }
    }

    fn run_human(&mut self, step: &Step, instruction: &str, expects_artifact: bool) -> Result<(), ()> {
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

#[cfg(test)]
mod tests {
    use super::parse_verdict;
    use crate::context::Verdict;

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
}
