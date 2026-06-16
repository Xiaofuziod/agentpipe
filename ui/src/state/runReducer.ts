import type { EngineEvent, StepStatus, RunStatus, GateKind } from "../types";

export interface StepView {
  status: StepStatus;
  summary?: string;
  error?: string;
}
export interface GateView {
  step_id: string;
  suggestion: string;
  expects_artifact: boolean;
  gate_kind: GateKind;
}
export interface RunState {
  name: string;
  order: string[]; // 出现顺序
  steps: Record<string, StepView>;
  loops: Record<string, { iteration: number; result?: string }>;
  activeGate: GateView | null;
  runStatus: RunStatus | null;
  log: string[]; // 人读流水
}

export const initialRunState = (): RunState => ({
  name: "",
  order: [],
  steps: {},
  loops: {},
  activeGate: null,
  runStatus: null,
  log: [],
});

function touch(s: RunState, id: string, patch: Partial<StepView>) {
  if (!s.order.includes(id)) s.order.push(id);
  const existing: StepView = s.steps[id] ?? { status: "Pending" };
  s.steps[id] = { ...existing, ...patch };
}

export function runReducer(prev: RunState, e: EngineEvent): RunState {
  const s: RunState = {
    ...prev,
    steps: { ...prev.steps },
    loops: { ...prev.loops },
    order: [...prev.order],
    log: [...prev.log],
  };
  switch (e.type) {
    case "RunStarted":
      s.name = e.name;
      s.log.push(`▶ ${e.name}`);
      break;
    case "StepStarted":
      touch(s, e.step_id, { status: "Running" });
      s.activeGate = null;
      break;
    case "StepProgress":
      s.log.push(`  ${e.line}`);
      break;
    case "StepAwaitingGate":
      touch(s, e.step_id, { status: "AwaitingGate" });
      s.activeGate = {
        step_id: e.step_id,
        suggestion: e.suggestion,
        expects_artifact: e.expects_artifact,
        gate_kind: e.gate_kind,
      };
      break;
    case "StepFinished":
      touch(s, e.step_id, { status: e.status, summary: e.summary });
      s.activeGate = null;
      break;
    case "StepFailed":
      touch(s, e.step_id, { status: "Failed", error: e.error });
      break;
    case "LoopIteration":
      s.loops[e.loop_id] = { iteration: e.iteration };
      break;
    case "LoopConverged":
      s.loops[e.loop_id] = { iteration: e.iterations, result: "收敛" };
      break;
    case "LoopMaxReached":
      s.loops[e.loop_id] = { iteration: e.max, result: "到上限未干净" };
      break;
    case "RunFinished":
      s.runStatus = e.status;
      s.activeGate = null;
      s.log.push(`■ ${e.status}`);
      break;
  }
  return s;
}
