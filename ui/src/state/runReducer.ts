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
  log: string[]; // 人读流水(封顶,见 LOG_CAP)
}

const LOG_CAP = 1000;

export const initialRunState = (): RunState => ({
  name: "",
  order: [],
  steps: {},
  loops: {},
  activeGate: null,
  runStatus: null,
  log: [],
});

function pushLog(log: string[], line: string): string[] {
  const next = [...log, line];
  return next.length > LOG_CAP ? next.slice(-LOG_CAP) : next;
}

// 只复制被改的 steps/order 切片,其余切片按引用复用(避免每事件全量复制 → O(n^2))。
function setStep(prev: RunState, id: string, patch: Partial<StepView>): Pick<RunState, "steps" | "order"> {
  const existing: StepView = prev.steps[id] ?? { status: "Pending" };
  return {
    steps: { ...prev.steps, [id]: { ...existing, ...patch } },
    order: prev.order.includes(id) ? prev.order : [...prev.order, id],
  };
}

export function runReducer(prev: RunState, e: EngineEvent): RunState {
  switch (e.type) {
    case "RunStarted":
      return { ...prev, name: e.name, log: pushLog(prev.log, `▶ ${e.name}`) };
    case "StepProgress":
      return { ...prev, log: pushLog(prev.log, `  ${e.line}`) };
    case "StepStarted":
      return { ...prev, ...setStep(prev, e.step_id, { status: "Running" }), activeGate: null };
    case "StepAwaitingGate":
      return {
        ...prev,
        ...setStep(prev, e.step_id, { status: "AwaitingGate" }),
        activeGate: {
          step_id: e.step_id,
          suggestion: e.suggestion,
          expects_artifact: e.expects_artifact,
          gate_kind: e.gate_kind,
        },
      };
    case "StepFinished":
      return { ...prev, ...setStep(prev, e.step_id, { status: e.status, summary: e.summary }), activeGate: null };
    case "StepFailed":
      return { ...prev, ...setStep(prev, e.step_id, { status: "Failed", error: e.error }) };
    case "LoopIteration":
      return { ...prev, loops: { ...prev.loops, [e.loop_id]: { iteration: e.iteration } } };
    case "LoopConverged":
      return { ...prev, loops: { ...prev.loops, [e.loop_id]: { iteration: e.iterations, result: "收敛" } } };
    case "LoopMaxReached":
      return { ...prev, loops: { ...prev.loops, [e.loop_id]: { iteration: e.max, result: "到上限未干净" } } };
    case "RunFinished":
      return { ...prev, runStatus: e.status, activeGate: null, log: pushLog(prev.log, `■ ${e.status}`) };
  }
}
