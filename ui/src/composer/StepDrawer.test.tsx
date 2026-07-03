import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { StepDrawer } from "./StepDrawer";
import type { Step } from "../types";

describe("StepDrawer", () => {
  it("renders acp command/on_permission/verify controls", () => {
    const step: Step = {
      id: "a",
      kind: "acp",
      agent: "gemini",
      prompt: "do",
    };

    const html = renderToStaticMarkup(
      <StepDrawer step={step} onChange={() => {}} onClose={() => {}} />
    );

    expect(html).toContain("留空则按 agent 名查");
    expect(html).toContain("on_permission");
    expect(html).toContain("校验门(verify)");
  });
});
