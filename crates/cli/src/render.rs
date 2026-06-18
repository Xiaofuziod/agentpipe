use agentpipe_engine::manifest::{Step, StepKind};
use agentpipe_engine::protocol::{Event, StepStatus};

/// dry-run:把一个 step 渲染成一行计划。纯函数。
pub fn render_plan_step(step: &Step) -> String {
    let detail = match &step.kind {
        StepKind::Claude { verify, skill, .. } => {
            let s = skill.as_deref().map(|s| format!(" skill={s}")).unwrap_or_default();
            let v = verify.as_ref().map(|_| " +verify").unwrap_or_default();
            format!("claude{s}{v}")
        }
        StepKind::Codex { action, .. } => format!("codex {action:?}"),
        StepKind::Human { .. } => "human".into(),
        StepKind::Loop { until, max, body } => {
            format!("loop until={until} max={max} ({} 步)", body.len())
        }
    };
    format!("  - {} [{detail}]", step.id)
}

/// StepMetrics 的人读片段:`N 轮 · X.Xs · $Y.YY`。render_event 与 cost 子命令共用,避免格式漂移。
pub fn format_metrics(num_turns: u32, duration_ms: u64, cost_usd: f64) -> String {
    format!("{} 轮 · {:.1}s · ${:.2}", num_turns, duration_ms as f64 / 1000.0, cost_usd)
}

/// 事件 → 人读一行。纯函数:无任何 I/O / stdin,view / dry-run / run 共用。
pub fn render_event(event: &Event) -> String {
    match event {
        Event::RunStarted { name, .. } => format!("▶ Run: {name}"),
        Event::StepStarted { step_id, kind } => format!("  ▷ [{kind}] {step_id}"),
        Event::StepProgress { line, .. } => format!("    {line}"),
        Event::StepFinished { step_id, status, summary, metrics } => {
            let mark = match status {
                StepStatus::Skipped => "⏭",
                StepStatus::Failed => "✗",
                _ => "✓",
            };
            let m = metrics
                .as_ref()
                .map(|m| format!(" · {}", format_metrics(m.num_turns, m.duration_ms, m.cost_usd)))
                .unwrap_or_default();
            format!("  {mark} {step_id}: {summary}{m}")
        }
        Event::StepFailed { step_id, error } => format!("  ✗ {step_id}: {error}"),
        Event::WorktreeReady { path, branch } => format!("  ⑂ worktree: {branch} @ {path}"),
        Event::WorktreeFailed { error } => format!("  ✗ worktree 创建失败: {error}"),
        Event::LoopIteration { loop_id, iteration } => format!("  ↻ {loop_id} 第 {iteration} 轮"),
        Event::LoopConverged { loop_id, iterations } => format!("  ✓ {loop_id} {iterations} 轮收敛"),
        Event::LoopMaxReached { loop_id, max } => format!("  ⚠ {loop_id} 到上限 {max} 仍未干净"),
        Event::StepAwaitingGate { step_id, suggestion, .. } => format!("  ⏸ {step_id}: {suggestion}"),
        Event::RunFinished { status } => format!("■ 结束: {status:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentpipe_engine::protocol::StepMetrics;

    #[test]
    fn renders_step_started() {
        let e = Event::StepStarted { step_id: "impl".into(), kind: "claude".into() };
        assert_eq!(render_event(&e), "  ▷ [claude] impl");
    }

    #[test]
    fn renders_finished_with_metrics() {
        let e = Event::StepFinished {
            step_id: "impl".into(),
            status: StepStatus::Done,
            summary: "done".into(),
            metrics: Some(StepMetrics { num_turns: 7, duration_ms: 41200, cost_usd: 0.83 }),
        };
        assert_eq!(render_event(&e), "  ✓ impl: done · 7 轮 · 41.2s · $0.83");
    }

    #[test]
    fn renders_worktree_events() {
        let ready = Event::WorktreeReady {
            path: "/tmp/.agentpipe-worktrees/repo-1-2".into(),
            branch: "agentpipe/fix-1-2".into(),
        };
        assert_eq!(render_event(&ready), "  ⑂ worktree: agentpipe/fix-1-2 @ /tmp/.agentpipe-worktrees/repo-1-2");
        let failed = Event::WorktreeFailed { error: "不是 git 仓库".into() };
        assert_eq!(render_event(&failed), "  ✗ worktree 创建失败: 不是 git 仓库");
    }

    #[test]
    fn renders_awaiting_gate_without_prompting() {
        let e = Event::StepAwaitingGate {
            step_id: "plan".into(),
            suggestion: "审批".into(),
            expects_artifact: false,
            gate_kind: agentpipe_engine::protocol::GateKind::Decision,
        };
        assert_eq!(render_event(&e), "  ⏸ plan: 审批");
    }

    #[test]
    fn renders_finished_failed_with_cross_mark() {
        let e = Event::StepFinished {
            step_id: "build".into(),
            status: StepStatus::Failed,
            summary: "编译失败".into(),
            metrics: None,
        };
        assert_eq!(render_event(&e), "  ✗ build: 编译失败");
    }

    #[test]
    fn renders_finished_skipped_without_metrics() {
        let e = Event::StepFinished {
            step_id: "lint".into(),
            status: StepStatus::Skipped,
            summary: "no changes".into(),
            metrics: None,
        };
        assert_eq!(render_event(&e), "  ⏭ lint: no changes");
    }
}
