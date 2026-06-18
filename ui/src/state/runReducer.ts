import type { EngineEvent, StepStatus, RunStatus, GateKind, StepMetrics } from "../types";

export interface StepView {
  status: StepStatus;
  summary?: string;
  error?: string;
  startedAt?: number; // 进入 Running 的 UI 时刻(实时秒表用)
  lastLine?: string; // 最近一条进度行(运行中展示)
  round?: number; // 当前模型请求轮次(claude stream-json)
  metrics?: StepMetrics; // 终态度量(轮次/耗时/成本)
}
export interface GateView {
  step_id: string;
  suggestion: string;
  expects_artifact: boolean;
  gate_kind: GateKind;
}
export interface RunState {
  name: string;
  target: string; // run 的 target(工作目录);取自 RunStarted,按项目归类用
  order: string[]; // 出现顺序
  steps: Record<string, StepView>;
  loops: Record<string, { iteration: number; result?: string }>;
  activeGate: GateView | null;
  runStatus: RunStatus | null;
  worktree?: { path: string; branch: string }; // 隔离 worktree 就绪(开启时)
  worktreeError?: string; // 隔离 worktree 创建失败 → Run fail-closed
  log: string[]; // 人读流水(封顶,见 LOG_CAP)
}

const LOG_CAP = 1000;

export const initialRunState = (): RunState => ({
  name: "",
  target: "",
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
      return { ...prev, name: e.name, target: e.target ?? "", log: pushLog(prev.log, `▶ ${e.name}`) };
    case "StepProgress": {
      const patch: Partial<StepView> = { lastLine: e.line };
      if (e.round != null) patch.round = e.round;
      // 有进度行 = 引擎已恢复执行,清掉可能残留的 gate(决策门批准后重试不发 StepStarted)
      return { ...prev, ...setStep(prev, e.step_id, patch), activeGate: null, log: pushLog(prev.log, `  ${e.line}`) };
    }
    case "StepStarted":
      return { ...prev, ...setStep(prev, e.step_id, { status: "Running", startedAt: Date.now() }), activeGate: null };
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
      return {
        ...prev,
        ...setStep(prev, e.step_id, { status: e.status, summary: e.summary, metrics: e.metrics ?? undefined }),
        activeGate: null,
      };
    case "StepFailed":
      return { ...prev, ...setStep(prev, e.step_id, { status: "Failed", error: e.error }) };
    case "WorktreeReady":
      return {
        ...prev,
        worktree: { path: e.path, branch: e.branch },
        log: pushLog(prev.log, `⑂ worktree ${e.branch} @ ${e.path}`),
      };
    case "WorktreeFailed":
      return { ...prev, worktreeError: e.error, log: pushLog(prev.log, `✗ worktree: ${e.error}`) };
    case "LoopIteration":
      return { ...prev, loops: { ...prev.loops, [e.loop_id]: { iteration: e.iteration } } };
    case "LoopConverged":
      return { ...prev, loops: { ...prev.loops, [e.loop_id]: { iteration: e.iterations, result: "收敛" } } };
    case "LoopMaxReached":
      return { ...prev, loops: { ...prev.loops, [e.loop_id]: { iteration: e.max, result: "到上限未干净" } } };
    case "RunFinished":
      return { ...prev, runStatus: e.status, activeGate: null, log: pushLog(prev.log, `■ ${e.status}`) };
    default:
      // 未知事件(版本错配 / 引擎新增类型):原样返回,绝不返回 undefined 让 state 损坏
      return prev;
  }
}
