import { useState } from "react";
import type { GateView } from "../state/runReducer";
import { ipc } from "../ipc";

export function GatePrompt({ gate }: { gate: GateView }) {
  const [artifact, setArtifact] = useState("");
  const approve = () =>
    ipc.sendCommand({ type: "ApproveGate", step_id: gate.step_id, artifact: artifact || null });
  const skip = () => ipc.sendCommand({ type: "SkipStep", step_id: gate.step_id });
  const abort = () => ipc.sendCommand({ type: "Abort" });
  return (
    <div style={{ border: "1px solid #888", borderRadius: 6, padding: 8, margin: "8px 0" }}>
      <div>
        ⏸ {gate.step_id}:{gate.suggestion}
      </div>
      {gate.expects_artifact && (
        <input
          placeholder="产物(路径 / URL)"
          value={artifact}
          onChange={(e) => setArtifact(e.target.value)}
          style={{ margin: "6px 0", width: "100%" }}
        />
      )}
      <div style={{ display: "flex", gap: 6 }}>
        <button onClick={approve}>{gate.gate_kind === "decision" ? "重试" : "批准"}</button>
        <button onClick={skip}>跳过</button>
        {gate.gate_kind === "decision" && <button onClick={abort}>中止</button>}
      </div>
    </div>
  );
}
