import { useEffect, useState } from "react";
import type { GateView } from "../state/runReducer";
import { ipc } from "../ipc";
import type { EngineCommand } from "../types";

export function GatePrompt({ gate }: { gate: GateView }) {
  const [artifact, setArtifact] = useState("");
  const [sent, setSent] = useState(false); // 发后禁用,防双击把多余指令灌给下一个 gate

  // 每来一个新 gate(reducer 每次 StepAwaitingGate 都新建 activeGate 对象)就重置。
  // 不能只靠 Console 的 key remount:同 step 同 kind 的连续 decision gate key 相同、不 remount,
  // 否则二次失败时按钮会一直禁用,用户卡死。
  useEffect(() => {
    setSent(false);
    setArtifact("");
  }, [gate]);

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
    <div className="gate">
      <div className="gate-head">
        <span>⏸</span>
        <span className="step-id">{gate.step_id}</span>
        <span>{gate.suggestion}</span>
      </div>
      {gate.expects_artifact && (
        <input
          className="input input-mono"
          placeholder="产物(路径 / URL)"
          value={artifact}
          onChange={(e) => setArtifact(e.target.value)}
          disabled={sent}
          style={{ marginBottom: 8 }}
        />
      )}
      <div className="row">
        <button className="btn btn-primary btn-sm" onClick={approve} disabled={sent}>
          {gate.gate_kind === "decision" ? "重试" : "批准"}
        </button>
        <button className="btn btn-sm" onClick={skip} disabled={sent}>
          跳过
        </button>
        {gate.gate_kind === "decision" && (
          <button className="btn btn-danger btn-sm" onClick={abort} disabled={sent}>
            中止
          </button>
        )}
      </div>
    </div>
  );
}
