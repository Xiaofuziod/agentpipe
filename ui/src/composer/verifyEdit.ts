import type { Step, Verify } from "../types";
type Claude = Extract<Step, { kind: "claude" }>;

const DEFAULT_VERIFY: Verify = { by: "codex", action: "review-mr", base: "dev", max_retries: 2, on_unmet: "gate", feedback: true };

export function setVerifyEnabled(step: Claude, on: boolean): Claude {
  if (on) return { ...step, verify: step.verify ?? { ...DEFAULT_VERIFY } };
  const { verify: _drop, ...rest } = step;
  return rest as Claude;
}

export function patchVerify(step: Claude, patch: Partial<Verify>): Claude {
  const base = step.verify ?? { ...DEFAULT_VERIFY };
  return { ...step, verify: { ...base, ...patch } };
}

export function verifySummary(v: Verify | undefined): string {
  return v ? `+verify:${v.by}` : "";
}
