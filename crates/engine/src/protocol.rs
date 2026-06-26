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
    RunStarted {
        name: String,
        /// run 的 target(工作目录绝对路径);用于按项目归类。
        /// 旧审计日志无此字段 → serde default 空串,容损读取不破。
        #[serde(default)]
        target: String,
    },
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
    /// 隔离 worktree 创建成功:后续所有 step 在此 cwd 跑。RunStarted 之后立即发。
    WorktreeReady { path: String, branch: String },
    /// 隔离 worktree 创建失败:Run fail-closed 终止(不退回 target 原地跑)。
    WorktreeFailed { error: String },
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
    /// codex 本次 review 的成本/轮次/耗时。codex CLI 当前不输出 token usage,所以
    /// 实际填 None;字段先就位让 verify_once 把 verifier cost 上报给 budget,等
    /// codex CLI 升级输出 metrics 后直接填,无需再改 schema。
    pub metrics: Option<StepMetrics>,
}
