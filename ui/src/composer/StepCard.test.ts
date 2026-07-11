import { describe, expect, it } from "vitest";
import { defaultsFor, stepSummary } from "./StepCard";
import type { Step } from "../types";

describe("StepCard", () => {
  it("creates acp steps without an inline command by default", () => {
    const step = defaultsFor("acp");
    expect(step).toEqual({ kind: "acp", agent: "", prompt: "" });
  });

  it("summarizes acp verify and optional command", () => {
    const step: Step = {
      id: "a",
      kind: "acp",
      agent: "gemini",
      prompt: "do",
      verify: { by: "command", command: "true" },
    };
    expect(stepSummary(step)).toContain("gemini");
    expect(stepSummary(step)).toContain("按 registry");
    expect(stepSummary(step)).toContain("+verify:command");
  });
});
