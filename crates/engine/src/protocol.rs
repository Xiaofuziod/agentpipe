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

/// 一个 agent step 的终态度量(轮次 + 耗时 + 成本),来自 CLI 的结构化终态行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepMetrics {
    pub num_turns: u32,
    pub duration_ms: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    RunStarted { name: String },
    StepStarted { step_id: String, kind: String },
    StepProgress {
        step_id: String,
        line: String,
        /// agent 步骤的模型请求轮次(claude stream-json 解析);非 agent / 无轮次为 None。
        #[serde(default)]
        round: Option<u32>,
    },
    StepAwaitingGate {
        step_id: String,
        suggestion: String,
        expects_artifact: bool,
        gate_kind: GateKind,
    },
    StepFinished {
        step_id: String,
        status: StepStatus,
        summary: String,
        /// agent 成功步骤的度量(轮次/耗时/成本);其余步骤为 None。
        #[serde(default)]
        metrics: Option<StepMetrics>,
    },
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
