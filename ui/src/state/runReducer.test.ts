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
});
