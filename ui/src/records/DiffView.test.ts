import { describe, it, expect } from "vitest";
import { diffRowText } from "./DiffView";

describe("diffRowText", () => {
  it("only_a / only_b / changed 三类文案", () => {
    expect(diffRowText({ step_id: "x", kind: "only_a", a_status: "Done", a_cost: 0.1, b_status: null, b_cost: null })).toContain("仅 A");
    expect(diffRowText({ step_id: "y", kind: "only_b", a_status: null, a_cost: null, b_status: "Done", b_cost: 0.2 })).toContain("仅 B");
    expect(diffRowText({ step_id: "z", kind: "changed", a_status: "Done", a_cost: 0.1, b_status: "Failed", b_cost: 0.3 })).toContain("→");
  });

  it("only_a 含 step_id 与 a_status", () => {
    const text = diffRowText({ step_id: "build", kind: "only_a", a_status: "Done", a_cost: 0.5, b_status: null, b_cost: null });
    expect(text).toBe("- build: 仅 A (Done)");
  });

  it("only_b 含 step_id 与 b_status", () => {
    const text = diffRowText({ step_id: "lint", kind: "only_b", a_status: null, a_cost: null, b_status: "Failed", b_cost: 0.2 });
    expect(text).toBe("+ lint: 仅 B (Failed)");
  });

  it("changed 含双向 status 与成本(null cost 当 0)", () => {
    const text = diffRowText({ step_id: "test", kind: "changed", a_status: "Done", a_cost: null, b_status: "Failed", b_cost: 0.3 });
    expect(text).toBe("~ test: Done $0.00 → Failed $0.30");
  });
});
