import { describe, it, expect } from "vitest";
import { setVerifyEnabled, patchVerify, verifySummary } from "./verifyEdit";
import type { Step } from "../types";

const claude = (): Extract<Step, { kind: "claude" }> => ({ id: "s", kind: "claude", prompt: "do" });

describe("verifyEdit", () => {
  it("启用产出默认 verify(by codex),停用则删除", () => {
    const on = setVerifyEnabled(claude(), true);
    expect(on.verify?.by).toBe("codex");
    const off = setVerifyEnabled(on, false);
    expect(off.verify).toBeUndefined();
  });
  it("patchVerify 合并字段", () => {
    const s = setVerifyEnabled(claude(), true);
    const s2 = patchVerify(s, { by: "command", command: "cargo test" });
    expect(s2.verify?.by).toBe("command");
    expect(s2.verify?.command).toBe("cargo test");
  });
  it("verifySummary 反映 by", () => {
    expect(verifySummary({ by: "command", command: "x" })).toContain("command");
    expect(verifySummary(undefined)).toBe("");
  });
});
