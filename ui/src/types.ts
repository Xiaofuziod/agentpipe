export type RunMode = "step" | "auto";
export type CodexAction = "review-doc" | "review-mr" | "ask";

// 与 crates/engine/src/manifest.rs 的 Verify 手工镜像同步(挂在 claude 步骤上)
export type Verify = {
  by: "codex" | "claude" | "command";
  action?: CodexAction; // codex verifier 用
  base?: string;
  path?: string;
  prompt?: string; // codex(ask)或 claude(判定指令)
  skill?: string; // claude verifier 用
  command?: string; // command verifier(exit 0 = 达成)
  max_retries?: number;
  on_unmet?: "gate" | "fail" | "continue";
  feedback?: boolean;
};

export type StepKind =
  | { kind: "claude"; prompt: string; skill?: string; verify?: Verify }
  | { kind: "codex"; action: CodexAction; path?: string; base?: string; prompt?: string }
  | { kind: "human"; instruction: string; expects?: string }
  | { kind: "loop"; until: "codex-clean"; max: number; body: Step[] };

export type Step = { id: string } & StepKind;
export type Manifest = { version: 1; name: string; target: string; mode: RunMode; worktree?: boolean; steps: Step[] };

export type GateKind = "step" | "human" | "decision";
export type StepStatus = "Pending" | "Running" | "AwaitingGate" | "Done" | "Failed" | "Skipped";
export type RunStatus = "Success" | "Failed" | "Aborted";

// 与 crates/engine/src/protocol.rs 的 StepMetrics 手工镜像同步
export type StepMetrics = { num_turns: number; duration_ms: number; cost_usd: number };

export type EngineEvent =
  | { type: "RunStarted"; name: string }
  | { type: "StepStarted"; step_id: string; kind: string }
  | { type: "StepProgress"; step_id: string; line: string; round?: number | null }
  | { type: "StepAwaitingGate"; step_id: string; suggestion: string; expects_artifact: boolean; gate_kind: GateKind }
  | { type: "StepFinished"; step_id: string; status: StepStatus; summary: string; metrics?: StepMetrics | null }
  | { type: "StepFailed"; step_id: string; error: string }
  | { type: "WorktreeReady"; path: string; branch: string }
  | { type: "WorktreeFailed"; error: string }
  | { type: "LoopIteration"; loop_id: string; iteration: number }
  | { type: "LoopConverged"; loop_id: string; iterations: number }
  | { type: "LoopMaxReached"; loop_id: string; max: number }
  | { type: "RunFinished"; status: RunStatus };

export type EngineCommand =
  | { type: "ApproveGate"; step_id: string; artifact: string | null }
  | { type: "SkipStep"; step_id: string }
  | { type: "Abort" };

export type RunSummary = {
  run_id: string;
  name: string;
  status: string | null;
  total_cost_usd: number;
  total_turns: number;
  step_count: number;
  complete: boolean;
};

export type DiffRow = {
  step_id: string;
  kind: "only_a" | "only_b" | "changed";
  a_status: string | null;
  a_cost: number | null;
  b_status: string | null;
  b_cost: number | null;
};
