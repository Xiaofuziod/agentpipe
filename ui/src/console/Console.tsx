import { useEffect, useReducer, useRef } from "react";
import { ipc } from "../ipc";
import { runReducer, initialRunState } from "../state/runReducer";
import type { StepStatus } from "../types";
import { GatePrompt } from "./GatePrompt";

const MARK: Record<StepStatus, string> = {
  Pending: "·",
  Running: "▷",
  AwaitingGate: "⏸",
  Done: "✓",
  Failed: "✗",
  Skipped: "⏭",
};

export function Console({ runPath }: { runPath: string | null }) {
  const [state, dispatch] = useReducer(runReducer, undefined, initialRunState);
  const started = useRef(false);

  useEffect(() => {
    const un = ipc.onEngineEvent(dispatch);
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (runPath && !started.current) {
      started.current = true;
      ipc.startRun(runPath).catch((e) => alert(String(e)));
    }
  }, [runPath]);

  return (
    <div style={{ padding: 12, fontFamily: "system-ui", maxWidth: 720 }}>
      <h3>控制台 {state.name && `— ${state.name}`}</h3>
      <div style={{ fontFamily: "ui-monospace, monospace", fontSize: 13 }}>
        {state.order.map((id) => {
          const st = state.steps[id];
          return (
            <div key={id}>
              {MARK[st.status]} {id} {st.summary ?? st.error ?? ""}
            </div>
          );
        })}
        {Object.entries(state.loops).map(([id, l]) => (
          <div key={id}>
            ↻ {id}:{l.iteration} 轮{l.result ? `(${l.result})` : ""}
          </div>
        ))}
      </div>

      {state.activeGate && <GatePrompt gate={state.activeGate} />}

      <div style={{ marginTop: 10 }}>
        {!state.runStatus && (
          <button onClick={() => ipc.sendCommand({ type: "Abort" })}>中止 Run</button>
        )}
        {state.runStatus && <div>结束:{state.runStatus}</div>}
      </div>
    </div>
  );
}
