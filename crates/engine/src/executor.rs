use crate::context::{RunContext, StepOutput, Verdict};
use crate::manifest::{Manifest, RunMode, Step, StepKind};
use crate::protocol::{Command, Event, RunStatus, StepStatus};
use crate::runner::claude::ClaudeRunner;
use crate::runner::codex::CodexRunner;
use std::sync::mpsc::{Receiver, Sender};

pub struct RunnerBins {
    pub claude: String,
    pub codex: String,
}

pub struct Executor {
    manifest: Manifest,
    ctx: RunContext,
    claude: ClaudeRunner,
    codex: CodexRunner,
    events: Sender<Event>,
    commands: Receiver<Command>,
}

impl Executor {
    pub fn new(
        manifest: Manifest,
        bins: RunnerBins,
        events: Sender<Event>,
        commands: Receiver<Command>,
    ) -> Self {
        let ctx = RunContext::new(manifest.target.clone());
        Self {
            claude: ClaudeRunner::new(bins.claude),
            codex: CodexRunner::new(bins.codex),
            manifest,
            ctx,
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
                let _ = self.events.send(Event::RunFinished {
                    status: RunStatus::Failed,
                });
                return RunStatus::Failed;
            }
        }
        let _ = self.events.send(Event::RunFinished {
            status: RunStatus::Success,
        });
        RunStatus::Success
    }

    /// gated: step 模式下顶层步骤执行前需先批准(loop 与其 body 子步骤不重复门控)。
    fn run_step(&mut self, step: &Step, gated: bool) -> Result<(), ()> {
        if gated && !matches!(step.kind, StepKind::Loop { .. }) {
            let _ = self.events.send(Event::StepAwaitingGate {
                step_id: step.id.clone(),
                suggestion: format!("即将执行 step '{}'", step.id),
                expects_artifact: false,
            });
            match self.commands.recv() {
                Ok(Command::ApproveGate { .. }) => {}
                Ok(Command::SkipStep { .. }) => {
                    let _ = self.events.send(Event::StepFinished {
                        step_id: step.id.clone(),
                        status: StepStatus::Skipped,
                        summary: "skipped".into(),
                    });
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
            StepKind::Codex {
                action,
                path,
                base,
                prompt,
            } => {
                let out = self
                    .codex
                    .review(
                        action,
                        path.as_deref(),
                        base.as_deref(),
                        prompt.as_deref(),
                        &self.ctx.cwd,
                    )
                    .map_err(|e| self.fail(&step.id, e.to_string()))?;
                let summary = format!("verdict={:?}", out.verdict);
                self.ctx.record(
                    &step.id,
                    StepOutput {
                        findings: Some(out.findings),
                        verdict: Some(out.verdict),
                        ..Default::default()
                    },
                );
                self.finish(&step.id, summary);
                Ok(())
            }
            StepKind::Claude {
                prompt,
                skill,
                allow_writes,
                ..
            } => {
                let p = self.ctx.interpolate(prompt);
                let out = self
                    .claude
                    .run(&p, skill.as_deref(), *allow_writes, &self.ctx.cwd)
                    .map_err(|e| self.fail(&step.id, e.to_string()))?;
                self.ctx.record(
                    &step.id,
                    StepOutput {
                        artifact: Some(out.last_line),
                        ..Default::default()
                    },
                );
                self.finish(&step.id, "done".into());
                Ok(())
            }
            StepKind::Human {
                instruction,
                expects,
            } => self.run_human(step, instruction, expects.is_some()),
            StepKind::Loop {
                until, max, body, ..
            } => self.run_loop(&step.id, until, *max, body),
        }
    }

    fn run_human(
        &mut self,
        step: &Step,
        instruction: &str,
        expects_artifact: bool,
    ) -> Result<(), ()> {
        let _ = self.events.send(Event::StepAwaitingGate {
            step_id: step.id.clone(),
            suggestion: instruction.to_string(),
            expects_artifact,
        });
        match self.commands.recv() {
            Ok(Command::ApproveGate { artifact, .. }) => {
                self.ctx.record(
                    &step.id,
                    StepOutput {
                        artifact,
                        ..Default::default()
                    },
                );
                self.finish(&step.id, "approved".into());
                Ok(())
            }
            Ok(Command::SkipStep { .. }) => {
                let _ = self.events.send(Event::StepFinished {
                    step_id: step.id.clone(),
                    status: StepStatus::Skipped,
                    summary: "skipped".into(),
                });
                Ok(())
            }
            _ => {
                self.fail(&step.id, "aborted".into());
                Err(())
            }
        }
    }

    fn run_loop(&mut self, loop_id: &str, until: &str, max: u32, body: &[Step]) -> Result<(), ()> {
        for n in 1..=max {
            let _ = self.events.send(Event::LoopIteration {
                loop_id: loop_id.into(),
                iteration: n,
            });
            for sub in body {
                self.run_step(sub, false)?;
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

    fn fail(&self, step_id: &str, error: String) {
        let _ = self.events.send(Event::StepFailed {
            step_id: step_id.to_string(),
            error,
        });
    }
}
