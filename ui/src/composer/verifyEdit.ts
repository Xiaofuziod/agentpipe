import type { Step, Verify } from "../types";
type Claude = Extract<Step, { kind: "claude" }>;
type Acp = Extract<Step, { kind: "acp" }>;
type VerifiableStep = Claude | Acp;

const DEFAULT_VERIFY: Verify = { by: "codex", action: "review-mr", base: "dev", max_retries: 2, on_unmet: "gate", feedback: true };

export function setVerifyEnabled<T extends VerifiableStep>(step: T, on: boolean): T {
  if (on) return { ...step, verify: step.verify ?? { ...DEFAULT_VERIFY } };
  const { verify: _drop, ...rest } = step;
  return rest as T;
}

export function patchVerify<T extends VerifiableStep>(step: T, patch: Partial<Verify>): T {
  const base = step.verify ?? { ...DEFAULT_VERIFY };
  return { ...step, verify: { ...base, ...patch } };
}

export function verifySummary(v: Verify | undefined): string {
  return v ? `+verify:${v.by}` : "";
}
