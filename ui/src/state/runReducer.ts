import type { EngineEvent, StepStatus, RunStatus, GateKind, StepMetrics } from "../types";

export interface StepView {
  status: StepStatus;
  summary?: string;
  error?: string;
  startedAt?: number; // 进入 Running 的 UI 时刻(实时秒表用)
  lastLine?: string; // 最近一条进度行(折叠态单行展示)
  lines?: string[]; // 全部进度行(展开态看实时/完整输出;封顶见 STEP_LINE_CAP)
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
const STEP_LINE_CAP = 500; // 每步保留的进度行上限(展开态滚动查看;封顶防长 run 撑爆内存)

function appendLine(lines: string[] | undefined, line: string): string[] {
  const next = lines ? [...lines, line] : [line];
  return next.length > STEP_LINE_CAP ? next.slice(-STEP_LINE_CAP) : next;
}

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
      const patch: Partial<StepView> = {
        lastLine: e.line,
        lines: appendLine(prev.steps[e.step_id]?.lines, e.line),
      };
      if (e.round != null) patch.round = e.round;
      // 有进度行 = 引擎已恢复执行,清掉可能残留的 gate(决策门批准后重试不发 StepStarted)
      return { ...prev, ...setStep(prev, e.step_id, patch), activeGate: null, log: pushLog(prev.log, `  ${e.line}`) };
    }
    case "StepStarted":
      // 重新开始(含 loop 内同 id 重跑):清掉上一次迭代的视图,否则展开输出会跨迭代
      // 累积、而 summary/metrics 只反映最后一次,二者串味不一致。
      return {
        ...prev,
        ...setStep(prev, e.step_id, {
          status: "Running",
          startedAt: Date.now(),
          lines: undefined,
          lastLine: undefined,
          summary: undefined,
          error: undefined,
          metrics: undefined,
          round: undefined,
        }),
        activeGate: null,
      };
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
