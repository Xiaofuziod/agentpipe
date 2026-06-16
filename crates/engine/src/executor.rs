use crate::context::{RunContext, StepOutput, Verdict};
use crate::control::Control;
use crate::manifest::{Manifest, RunMode, Step, StepKind};
use crate::protocol::{Command, Event, GateKind, RunStatus, StepStatus};
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
                loop {
                    let res = self.codex.review(
                        action,
                        path_i.as_deref(),
                        base_i.as_deref(),
                        prompt_i.as_deref(),
                        Some(self.control.as_ref()),
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
                            self.finish(&step.id, summary);
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
            StepKind::Claude { prompt, skill, allow_writes, timeout } => {
                let p = self.ctx.interpolate(prompt);
                loop {
                    let res = self.claude.run(
                        &p,
                        skill.as_deref(),
                        *allow_writes,
                        *timeout,
                        Some(self.control.as_ref()),
                        &self.ctx.cwd,
                    );
                    match res {
                        Ok(out) => {
                            self.ctx.record(&step.id, StepOutput {
                                artifact: Some(out.last_line),
                                ..Default::default()
                            });
                            self.finish(&step.id, "done".into());
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
            StepKind::Human { instruction, expects } => {
                let instr = self.ctx.interpolate(instruction);
                self.run_human(step, &instr, expects.is_some())
            }
            StepKind::Loop { until, max, body } => self.run_loop(&step.id, until, *max, body, gated),
        }
    }

    /// step 失败的处理决策:已中止则直接 Abort(不弹 gate);否则发决策 gate 等指令。
    fn handle_failure(&mut self, step_id: &str, err: String) -> StepDecision {
        if self.control.is_aborted() {
            return StepDecision::Abort;
        }
        let _ = self.events.send(Event::StepFailed {
            step_id: step_id.to_string(),
            error: err,
        });
        let _ = self.events.send(Event::StepAwaitingGate {
            step_id: step_id.to_string(),
            suggestion: "step 失败,选择 重试 / 跳过 / 中止".into(),
            expects_artifact: false,
            gate_kind: GateKind::Decision,
        });
        match self.commands.recv() {
            Ok(Command::ApproveGate { .. }) => StepDecision::Retry,
            Ok(Command::SkipStep { .. }) => StepDecision::Skip,
            _ => StepDecision::Abort,
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
                self.finish(&step.id, "approved".into());
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

    fn finish(&self, step_id: &str, summary: String) {
        let _ = self.events.send(Event::StepFinished {
            step_id: step_id.to_string(),
            status: StepStatus::Done,
            summary,
        });
    }

    fn emit_skipped(&self, step_id: &str) {
        let _ = self.events.send(Event::StepFinished {
            step_id: step_id.to_string(),
            status: StepStatus::Skipped,
            summary: "skipped".into(),
        });
    }

    fn fail(&self, step_id: &str, error: String) {
        let _ = self.events.send(Event::StepFailed {
            step_id: step_id.to_string(),
            error,
        });
    }
}
