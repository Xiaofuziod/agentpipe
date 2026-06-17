import { describe, it, expect } from "vitest";
import { routeEvent, type RunRecord } from "./useRuns";
import { initialRunState } from "./runReducer";

const rec = (id: string): RunRecord => ({
  id,
  name: id,
  path: `/tmp/${id}.yaml`,
  startedAt: 0,
  state: initialRunState(),
});

describe("routeEvent", () => {
  it("只更新活跃记录,其余记录按引用原样保留", () => {
    const a = rec("a");
    const b = rec("b");
    const { records } = routeEvent([a, b], "b", { type: "StepStarted", step_id: "s1", kind: "claude" });
    expect(records[0]).toBe(a); // 非活跃,引用不变
    expect(records[1]).not.toBe(b); // 活跃,新对象
    expect(records[1].state.steps.s1.status).toBe("Running");
  });

  it("无活跃 run 时原样返回(事件被丢弃,不误写任何记录)", () => {
    const a = rec("a");
    const input = [a];
    const out = routeEvent(input, null, { type: "StepStarted", step_id: "s1", kind: "claude" });
    expect(out.records).toBe(input);
    expect(out.activeId).toBeNull();
  });

  it("RunFinished 后把 activeId 置空,但仍写入终态", () => {
    const a = rec("a");
    const { records, activeId } = routeEvent([a], "a", { type: "RunFinished", status: "Success" });
    expect(activeId).toBeNull();
    expect(records[0].state.runStatus).toBe("Success");
  });
});
