import { describe, it, expect } from "vitest";
import { runReducer, initialRunState } from "./runReducer";

describe("runReducer", () => {
  it("tracks step lifecycle", () => {
    let s = initialRunState();
    s = runReducer(s, { type: "RunStarted", name: "t" });
    s = runReducer(s, { type: "StepStarted", step_id: "rev", kind: "codex" });
    expect(s.steps.rev.status).toBe("Running");
    s = runReducer(s, { type: "StepFinished", step_id: "rev", status: "Done", summary: "verdict=Clean" });
    expect(s.steps.rev.status).toBe("Done");
    expect(s.steps.rev.summary).toContain("Clean");
  });

  it("captures the active gate and clears it on next event", () => {
    let s = initialRunState();
    s = runReducer(s, {
      type: "StepAwaitingGate",
      step_id: "fix",
      suggestion: "go?",
      expects_artifact: false,
      gate_kind: "step",
    });
    expect(s.activeGate?.step_id).toBe("fix");
    s = runReducer(s, { type: "StepStarted", step_id: "fix", kind: "claude" });
    expect(s.activeGate).toBeNull();
  });

  it("captures worktree ready and failure", () => {
    let s = initialRunState();
    s = runReducer(s, { type: "WorktreeReady", path: "/tmp/wt/repo-1", branch: "agentpipe/x-1" });
    expect(s.worktree).toEqual({ path: "/tmp/wt/repo-1", branch: "agentpipe/x-1" });

    let f = initialRunState();
    f = runReducer(f, { type: "WorktreeFailed", error: "不是 git 仓库" });
    expect(f.worktreeError).toBe("不是 git 仓库");
  });

  it("records loop convergence and run terminal status", () => {
    let s = initialRunState();
    s = runReducer(s, { type: "LoopConverged", loop_id: "l", iterations: 2 });
    expect(s.loops.l.result).toBe("收敛");
    s = runReducer(s, { type: "RunFinished", status: "Aborted" });
    expect(s.runStatus).toBe("Aborted");
  });

  it("appends progress lines to log", () => {
    let s = initialRunState();
    s = runReducer(s, { type: "StepProgress", step_id: "x", line: "hello" });
    expect(s.log.some((l) => l.includes("hello"))).toBe(true);
  });

  it("stamps startedAt on StepStarted and tracks lastLine on StepProgress", () => {
    let s = initialRunState();
    s = runReducer(s, { type: "StepStarted", step_id: "impl", kind: "claude" });
    expect(typeof s.steps.impl.startedAt).toBe("number");
    s = runReducer(s, { type: "StepProgress", step_id: "impl", line: "第 2 轮 · 调用 Bash" });
    expect(s.steps.impl.lastLine).toBe("第 2 轮 · 调用 Bash");
    expect(s.steps.impl.status).toBe("Running"); // 进度行不改状态
  });

  it("tracks round from StepProgress and metrics from StepFinished", () => {
    let s = initialRunState();
    s = runReducer(s, { type: "StepStarted", step_id: "impl", kind: "claude" });
    s = runReducer(s, { type: "StepProgress", step_id: "impl", line: "调用 Bash", round: 2 });
    expect(s.steps.impl.round).toBe(2);
    s = runReducer(s, {
      type: "StepFinished",
      step_id: "impl",
      status: "Done",
      summary: "done",
      metrics: { num_turns: 3, duration_ms: 3266, cost_usd: 0.49 },
    });
    expect(s.steps.impl.metrics).toEqual({ num_turns: 3, duration_ms: 3266, cost_usd: 0.49 });
  });

  it("clears a lingering gate when progress resumes (gate-approved retry)", () => {
    let s = initialRunState();
    s = runReducer(s, {
      type: "StepAwaitingGate",
      step_id: "impl",
      suggestion: "校验未通过,重试?",
      expects_artifact: false,
      gate_kind: "decision",
    });
    expect(s.activeGate).not.toBeNull();
    // 批准后引擎重跑(不发 StepStarted),首个进度行应清掉残留的 gate
    s = runReducer(s, { type: "StepProgress", step_id: "impl", line: "重试中" });
    expect(s.activeGate).toBeNull();
  });

  it("leaves round unset when StepProgress carries no round (codex path)", () => {
    let s = initialRunState();
    s = runReducer(s, { type: "StepProgress", step_id: "rev", line: "codex 输出" });
    expect(s.steps.rev.round).toBeUndefined();
    expect(s.steps.rev.lastLine).toBe("codex 输出");
  });

  it("returns prev state (never undefined) for an unknown event type", () => {
    const s = initialRunState();
    // 模拟版本错配:引擎发来 UI 类型里没有的事件
    const next = runReducer(s, { type: "FutureEvent" } as unknown as Parameters<typeof runReducer>[1]);
    expect(next).toBe(s);
  });

  it("replays the full-pipeline event stream to a correct terminal console state", () => {
    // 序列对照 `agentpipe run templates/full-pipeline.yaml`(stub, verdict=clean)实跑事件流:
    // human 步骤 StepStarted→Gate(Human)→Finished;gated 步骤 Gate(Step)→Started→Finished;
    // loop 体子步骤逐个门控;最终收敛 Success。
    const evs: Parameters<typeof runReducer>[1][] = [
      { type: "RunStarted", name: "full-test" },
      { type: "StepStarted", step_id: "brainstorm", kind: "human" },
      { type: "StepAwaitingGate", step_id: "brainstorm", suggestion: "", expects_artifact: true, gate_kind: "human" },
      { type: "StepFinished", step_id: "brainstorm", status: "Done", summary: "approved" },
      { type: "StepAwaitingGate", step_id: "design-review-claude", suggestion: "", expects_artifact: false, gate_kind: "step" },
      { type: "StepStarted", step_id: "design-review-claude", kind: "claude" },
      { type: "StepFinished", step_id: "design-review-claude", status: "Done", summary: "done" },
      { type: "StepAwaitingGate", step_id: "design-review-codex", suggestion: "", expects_artifact: false, gate_kind: "step" },
      { type: "StepStarted", step_id: "design-review-codex", kind: "codex" },
      { type: "StepFinished", step_id: "design-review-codex", status: "Done", summary: "verdict=Clean" },
      { type: "StepStarted", step_id: "plan", kind: "human" },
      { type: "StepAwaitingGate", step_id: "plan", suggestion: "", expects_artifact: true, gate_kind: "human" },
      { type: "StepFinished", step_id: "plan", status: "Done", summary: "approved" },
      { type: "StepAwaitingGate", step_id: "implement", suggestion: "", expects_artifact: false, gate_kind: "step" },
      { type: "StepStarted", step_id: "implement", kind: "claude" },
      { type: "StepFinished", step_id: "implement", status: "Done", summary: "done" },
      { type: "StepStarted", step_id: "self-review", kind: "human" },
      { type: "StepAwaitingGate", step_id: "self-review", suggestion: "", expects_artifact: false, gate_kind: "human" },
      { type: "StepFinished", step_id: "self-review", status: "Done", summary: "approved" },
      { type: "LoopIteration", loop_id: "codex-loop", iteration: 1 },
      { type: "StepAwaitingGate", step_id: "codex-review-mr", suggestion: "", expects_artifact: false, gate_kind: "step" },
      { type: "StepStarted", step_id: "codex-review-mr", kind: "codex" },
      { type: "StepFinished", step_id: "codex-review-mr", status: "Done", summary: "verdict=Clean" },
      { type: "StepAwaitingGate", step_id: "apply-feedback", suggestion: "", expects_artifact: false, gate_kind: "step" },
      { type: "StepStarted", step_id: "apply-feedback", kind: "claude" },
      { type: "StepFinished", step_id: "apply-feedback", status: "Done", summary: "done" },
      { type: "LoopConverged", loop_id: "codex-loop", iterations: 1 },
      { type: "RunFinished", status: "Success" },
    ];
    let s = initialRunState();
    for (const e of evs) s = runReducer(s, e);

    expect(s.name).toBe("full-test");
    expect(s.runStatus).toBe("Success");
    expect(s.activeGate).toBeNull(); // 终态不残留 gate
    expect(s.loops["codex-loop"].result).toBe("收敛");
    // 全部 8 个步骤都出现且终态 Done(顺序即出现顺序)
    expect(s.order).toEqual([
      "brainstorm",
      "design-review-claude",
      "design-review-codex",
      "plan",
      "implement",
      "self-review",
      "codex-review-mr",
      "apply-feedback",
    ]);
    expect(Object.values(s.steps).every((st) => st.status === "Done")).toBe(true);
  });

  it("caps the log so it cannot grow unbounded", () => {
    let s = initialRunState();
    for (let i = 0; i < 1200; i++) {
      s = runReducer(s, { type: "StepProgress", step_id: "x", line: `line-${i}` });
    }
    expect(s.log.length).toBeLessThanOrEqual(1000);
    expect(s.log[s.log.length - 1]).toContain("line-1199");
  });
});
