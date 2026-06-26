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
    /// Step 失败终态。metrics 携带失败前已累积的成本(verify-retry 中段 / OnUnmet::Fail /
    /// budget 触发等场景),让 audit::aggregate_cost 不再因失败路径漏统计。
    /// review §A finding #4:旧版无 metrics 字段时 audit 总成本 = $0,与 ctx.cost_so_far_usd
    /// 真实账目脱节,触发"budget 把 step 砍掉,UI 却显示一分钱没花"的反认知体验。
    /// `#[serde(default)]` 保持向后兼容老审计日志(legacy NDJSON 无此字段 → None,与
    /// 新无 cost 失败语义一致 = 该 step 不入 audit cost,避免漂移)。
    StepFailed {
        step_id: String,
        error: String,
        #[serde(default)]
        metrics: Option<StepMetrics>,
    },
    /// 隔离 worktree 创建成功:后续所有 step 在此 cwd 跑。RunStarted 之后立即发。
    WorktreeReady { path: String, branch: String },
    /// 隔离 worktree 创建失败:Run fail-closed 终止(不退回 target 原地跑)。
    WorktreeFailed { error: String },
    LoopIteration { loop_id: String, iteration: u32 },
    LoopConverged { loop_id: String, iterations: u32 },
    /// Loop 终止事件。reason 区分三种结束原因:自然 max(原义)、外部 Abort、sub-step 失败
    /// 透传 —— UI/CLI 渲染应按 reason 出不同文案,而非旧版统一的「hit max,still not clean」
    /// 误导文本(review §A finding #15)。`#[serde(default)]` 让老审计日志解析回退 MaxReached,
    /// 历史回放语义不变。
    LoopMaxReached {
        loop_id: String,
        max: u32,
        #[serde(default)]
        reason: LoopEndReason,
    },
    RunFinished { status: RunStatus },
}

/// Loop 终止原因。MaxReached 保留原义(自然耗尽 max 仍未收敛);Aborted/SubStepFailed
/// 是 review §A finding #15 加的语义化分流,前 PR 共用 MaxReached variant 导致渲染层
/// 把"外部中止"误读为"loop 跑到上限"。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopEndReason {
    /// 跑完 max 轮 until 仍未满足。
    #[default]
    MaxReached,
    /// 外部 Control::request_abort / 用户 Abort 决策门。
    Aborted,
    /// loop body 内 sub-step 失败 Err 透传(charge_and_check Err / decision gate Abort /
    /// runner 自身失败被升级 fail)。
    SubStepFailed,
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
