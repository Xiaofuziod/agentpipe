import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { GatePrompt } from "./GatePrompt";
import type { GateView } from "../state/runReducer";

vi.mock("../ipc", () => ({
  ipc: {
    sendCommand: vi.fn(),
  },
}));

describe("GatePrompt", () => {
  it("renders permission gates as approve/reject/abort", () => {
    const gate: GateView = {
      step_id: "acp",
      suggestion: "acp step 权限请求: read file",
      expects_artifact: false,
      gate_kind: "permission",
    };

    const html = renderToStaticMarkup(<GatePrompt gate={gate} />);

    expect(html).toContain(">批准<");
    expect(html).toContain(">拒绝<");
    expect(html).toContain(">中止<");
    expect(html).not.toContain(">重试<");
    expect(html).not.toContain(">跳过<");
  });
});
