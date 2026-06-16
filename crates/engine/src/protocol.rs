use crate::context::Verdict;

#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    AwaitingGate,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub enum Event {
    RunStarted { name: String },
    StepStarted { step_id: String, kind: String },
    StepProgress { step_id: String, line: String },
    StepAwaitingGate { step_id: String, suggestion: String, expects_artifact: bool },
    StepFinished { step_id: String, status: StepStatus, summary: String },
    StepFailed { step_id: String, error: String },
    LoopIteration { loop_id: String, iteration: u32 },
    LoopConverged { loop_id: String, iterations: u32 },
    LoopMaxReached { loop_id: String, max: u32 },
    RunFinished { status: RunStatus },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunStatus {
    Success,
    Failed,
    Aborted,
}

#[derive(Debug, Clone)]
pub enum Command {
    ApproveGate { step_id: String, artifact: Option<String> },
    SkipStep { step_id: String },
    Interrupt,
    Resume,
    Abort,
}

/// 供 codex runner 复用的结果类型
#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub verdict: Verdict,
    pub findings: String,
}
