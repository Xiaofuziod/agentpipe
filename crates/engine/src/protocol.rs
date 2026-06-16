use crate::context::Verdict;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateKind {
    Step,
    Human,
    Decision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Running,
    AwaitingGate,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    RunStarted { name: String },
    StepStarted { step_id: String, kind: String },
    StepProgress { step_id: String, line: String },
    StepAwaitingGate {
        step_id: String,
        suggestion: String,
        expects_artifact: bool,
        gate_kind: GateKind,
    },
    StepFinished { step_id: String, status: StepStatus, summary: String },
    StepFailed { step_id: String, error: String },
    LoopIteration { loop_id: String, iteration: u32 },
    LoopConverged { loop_id: String, iterations: u32 },
    LoopMaxReached { loop_id: String, max: u32 },
    RunFinished { status: RunStatus },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RunStatus {
    Success,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    ApproveGate { step_id: String, artifact: Option<String> },
    SkipStep { step_id: String },
    Interrupt,
    #[deprecated(note = "用决策 gate 的 ApproveGate 表达重试")]
    Resume,
    Abort,
}

/// 供 codex runner 复用的结果类型
#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub verdict: Verdict,
    pub findings: String,
}
