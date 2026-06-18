import { describe, it, expect } from "vitest";
import { replayToState } from "./useHistory";
import type { EngineEvent } from "../types";

describe("replayToState", () => {
  it("折叠事件序列重建终态", () => {
    const events: EngineEvent[] = [
      { type: "RunStarted", name: "demo" },
      { type: "StepStarted", step_id: "a", kind: "claude" },
      { type: "StepFinished", step_id: "a", status: "Done", summary: "ok", metrics: { num_turns: 2, duration_ms: 2000, cost_usd: 0.4 } },
      { type: "RunFinished", status: "Success" },
    ];
    const st = replayToState(events);
    expect(st.name).toBe("demo");
    expect(st.order).toEqual(["a"]);
    expect(st.steps["a"].status).toBe("Done");
    expect(st.runStatus).toBe("Success");
  });

  it("空序列返回初始态", () => {
    expect(replayToState([]).order).toEqual([]);
  });
});
