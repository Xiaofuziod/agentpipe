import { useState } from "react";
import type { GateView } from "../state/runReducer";
import { ipc } from "../ipc";
import type { EngineCommand } from "../types";

export function GatePrompt({ gate }: { gate: GateView }) {
  const [artifact, setArtifact] = useState("");
  const [sent, setSent] = useState(false); // 发后禁用,防双击把多余指令灌给下一个 gate

  const send = (cmd: EngineCommand) => {
    if (sent) return;
    setSent(true);
    ipc.sendCommand(cmd).catch((e) => {
      setSent(false);
      alert(String(e));
    });
  };

  const approve = () => send({ type: "ApproveGate", step_id: gate.step_id, artifact: artifact || null });
  const skip = () => send({ type: "SkipStep", step_id: gate.step_id });
  const abort = () => send({ type: "Abort" });

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
          disabled={sent}
          style={{ margin: "6px 0", width: "100%" }}
        />
      )}
      <div style={{ display: "flex", gap: 6 }}>
        <button onClick={approve} disabled={sent}>
          {gate.gate_kind === "decision" ? "重试" : "批准"}
        </button>
        <button onClick={skip} disabled={sent}>
          跳过
        </button>
        {gate.gate_kind === "decision" && (
          <button onClick={abort} disabled={sent}>
            中止
          </button>
        )}
      </div>
    </div>
  );
}
