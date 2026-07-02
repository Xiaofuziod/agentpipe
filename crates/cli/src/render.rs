use agentpipe_engine::manifest::{Step, StepKind};
use agentpipe_engine::protocol::{Event, LoopEndReason, StepStatus};

/// dry-run:把一个 step 渲染成计划行;loop 递归展开 body(缩进两格)。纯函数。
/// vet / allow_residual 必须可见(review finding #9):前者意味着每个非 clean review
/// 轮多一次 codex 调用的钱,后者放宽收敛严格度,dry-run 正是给用户执行前核对这些的。
pub fn render_plan_step(step: &Step) -> String {
    let detail = match &step.kind {
        StepKind::Claude { verify, skill, .. } => {
            let s = skill.as_deref().map(|s| format!(" skill={s}")).unwrap_or_default();
            let v = verify.as_ref().map(|_| " +verify").unwrap_or_default();
            format!("claude{s}{v}")
        }
        StepKind::Codex { action, vet, .. } => {
            let v = if *vet { " +vet" } else { "" };
            format!("codex {action:?}{v}")
        }
        StepKind::Human { .. } => "human".into(),
        StepKind::Acp { agent, .. } => format!("acp {agent}"),
        StepKind::Loop { until, max, allow_residual, body } => {
            let ar = allow_residual
                .as_ref()
                .map(|s| format!(" allow_residual={}", format!("{s:?}").to_lowercase()))
                .unwrap_or_default();
            format!("loop until={until} max={max}{ar} ({} steps)", body.len())
        }
    };
    let line = format!("  - {} [{detail}]", step.id);
    if let StepKind::Loop { body, .. } = &step.kind {
        let inner = body
            .iter()
            .map(|s| {
                render_plan_step(s)
                    .lines()
                    .map(|l| format!("  {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n");
        return format!("{line}\n{inner}");
    }
    line
}

/// StepMetrics 的人读片段:`N turns · X.Xs · $Y.YY`。render_event 与 cost 子命令共用,避免格式漂移。
pub fn format_metrics(num_turns: u32, duration_ms: u64, cost_usd: f64) -> String {
    format!("{} turns · {:.1}s · ${:.2}", num_turns, duration_ms as f64 / 1000.0, cost_usd)
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
        Event::StepFailed { step_id, error, metrics } => {
            // 失败也展示已烧掉的花费:budget 触发 / OnUnmet::Fail 的 StepFailed 携带
            // cumulative metrics(审计已入账),只报 error 不报 spend 会让用户看不到
            // "花了多少钱之后才死的"(codex review P3),与上方 StepFinished 同形。
            let m = metrics
                .as_ref()
                .map(|m| format!(" · {}", format_metrics(m.num_turns, m.duration_ms, m.cost_usd)))
                .unwrap_or_default();
            format!("  ✗ {step_id}: {error}{m}")
        }
        Event::WorktreeReady { path, branch } => format!("  ⑂ worktree: {branch} @ {path}"),
        Event::WorktreeFailed { error } => format!("  ✗ worktree failed: {error}"),
        Event::LoopIteration { loop_id, iteration } => format!("  ↻ {loop_id} round {iteration}"),
        Event::LoopConverged { loop_id, iterations, residual } => {
            if *residual > 0 {
                format!("  ✓ {loop_id} converged in {iterations} round(s), {residual} residual finding(s) allowed")
            } else {
                format!("  ✓ {loop_id} converged in {iterations} round(s)")
            }
        }
        Event::LoopMaxReached { loop_id, max, reason } => match reason {
            LoopEndReason::MaxReached => {
                // P1 Retry 续号后 `max` 是累计已跑轮数,可能 > 配置的 max(2 次 Retry
                // 后如 15 > 5)。"hit max {max}" 读起来像"配置的上限是 {max}",改成
                // "ran {max} round(s)" 避免暗示配置值。
                format!("  ⚠ {loop_id} ran {max} round(s), still not clean")
            }
            LoopEndReason::Aborted => format!("  ⏹ {loop_id} aborted at round {max}"),
            LoopEndReason::SubStepFailed => {
                format!("  ✗ {loop_id} stopped at round {max} (sub-step failed)")
            }
        },
        Event::StepAwaitingGate { step_id, suggestion, .. } => format!("  ⏸ {step_id}: {suggestion}"),
        Event::RunFinished { status } => format!("■ Done: {status:?}"),
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
        assert_eq!(render_event(&e), "  ✓ impl: done · 7 turns · 41.2s · $0.83");
    }

    #[test]
    fn renders_failed_with_metrics() {
        // codex review P3 回归:budget / verifier 重试耗尽的失败带 cumulative metrics,
        // 渲染必须展示已烧掉的花费;无 metrics 的失败保持旧文案。
        let e = Event::StepFailed {
            step_id: "impl".into(),
            error: "超出 USD budget".into(),
            metrics: Some(StepMetrics { num_turns: 7, duration_ms: 41200, cost_usd: 0.83 }),
        };
        assert_eq!(render_event(&e), "  ✗ impl: 超出 USD budget · 7 turns · 41.2s · $0.83");
        let bare = Event::StepFailed { step_id: "impl".into(), error: "boom".into(), metrics: None };
        assert_eq!(render_event(&bare), "  ✗ impl: boom");
    }

    #[test]
    fn renders_worktree_events() {
        let ready = Event::WorktreeReady {
            path: "/tmp/.agentpipe-worktrees/repo-1-2".into(),
            branch: "agentpipe/fix-1-2".into(),
        };
        assert_eq!(render_event(&ready), "  ⑂ worktree: agentpipe/fix-1-2 @ /tmp/.agentpipe-worktrees/repo-1-2");
        let failed = Event::WorktreeFailed { error: "not a git repo".into() };
        assert_eq!(render_event(&failed), "  ✗ worktree failed: not a git repo");
    }

    #[test]
    fn renders_awaiting_gate_without_prompting() {
        let e = Event::StepAwaitingGate {
            step_id: "plan".into(),
            suggestion: "approve?".into(),
            expects_artifact: false,
            gate_kind: agentpipe_engine::protocol::GateKind::Decision,
        };
        assert_eq!(render_event(&e), "  ⏸ plan: approve?");
    }

    #[test]
    fn renders_finished_failed_with_cross_mark() {
        let e = Event::StepFinished {
            step_id: "build".into(),
            status: StepStatus::Failed,
            summary: "build failed".into(),
            metrics: None,
        };
        assert_eq!(render_event(&e), "  ✗ build: build failed");
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

    #[test]
    fn renders_loop_converged_with_residual() {
        let e = Event::LoopConverged { loop_id: "l".into(), iterations: 2, residual: 3 };
        assert_eq!(render_event(&e), "  ✓ l converged in 2 round(s), 3 residual finding(s) allowed");
        let e0 = Event::LoopConverged { loop_id: "l".into(), iterations: 2, residual: 0 };
        assert_eq!(render_event(&e0), "  ✓ l converged in 2 round(s)");
    }

    #[test]
    fn renders_loop_max_reached_without_implying_configured_max() {
        // 收尾自查(四维度「字面 vs 语义」):P1 Retry 续号后 max 字段是累计已跑轮数,
        // 可能 > manifest 配置的 max(如 Retry 一次后 10 > 配置的 5)。文案不能用
        // "hit max {max}"(读起来像"配置上限是 {max}"),必须是中性的"跑了几轮"。
        let e = Event::LoopMaxReached {
            loop_id: "review-fix".into(),
            max: 10,
            reason: LoopEndReason::MaxReached,
        };
        assert_eq!(render_event(&e), "  ⚠ review-fix ran 10 round(s), still not clean");
    }
}
