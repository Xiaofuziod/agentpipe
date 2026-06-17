export type RunMode = "step" | "auto";
export type CodexAction = "review-doc" | "review-mr" | "ask";

export type StepKind =
  | { kind: "claude"; prompt: string; skill?: string }
  | { kind: "codex"; action: CodexAction; path?: string; base?: string; prompt?: string }
  | { kind: "human"; instruction: string; expects?: string }
  | { kind: "loop"; until: "codex-clean"; max: number; body: Step[] };

export type Step = { id: string } & StepKind;
export type Manifest = { version: 1; name: string; target: string; mode: RunMode; steps: Step[] };

export type GateKind = "step" | "human" | "decision";
export type StepStatus = "Pending" | "Running" | "AwaitingGate" | "Done" | "Failed" | "Skipped";
export type RunStatus = "Success" | "Failed" | "Aborted";

export type EngineEvent =
  | { type: "RunStarted"; name: string }
  | { type: "StepStarted"; step_id: string; kind: string }
  | { type: "StepProgress"; step_id: string; line: string }
  | { type: "StepAwaitingGate"; step_id: string; suggestion: string; expects_artifact: boolean; gate_kind: GateKind }
  | { type: "StepFinished"; step_id: string; status: StepStatus; summary: string }
  | { type: "StepFailed"; step_id: string; error: string }
  | { type: "LoopIteration"; loop_id: string; iteration: number }
  | { type: "LoopConverged"; loop_id: string; iterations: number }
  | { type: "LoopMaxReached"; loop_id: string; max: number }
  | { type: "RunFinished"; status: RunStatus };

export type EngineCommand =
  | { type: "ApproveGate"; step_id: string; artifact: string | null }
  | { type: "SkipStep"; step_id: string }
  | { type: "Abort" };
