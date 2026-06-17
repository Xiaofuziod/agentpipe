import { useEffect, useRef, useState } from "react";
import { ipc } from "../ipc";
import type { StepStatus } from "../types";
import type { RunRecord } from "../state/useRuns";
import { GatePrompt } from "./GatePrompt";

const MARK: Record<StepStatus, string> = {
  Pending: "·",
  Running: "▷",
  AwaitingGate: "⏸",
  Done: "✓",
  Failed: "✗",
  Skipped: "⏭",
};

/** 活跃时每秒返回当前时刻,用于驱动运行中 step 的实时秒表;非活跃不 tick(省渲染)。 */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

function fmtElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const ss = String(s % 60).padStart(2, "0");
  return `${Math.floor(s / 60)}:${ss}`;
}

function fmtMetrics(m: { num_turns: number; duration_ms: number; cost_usd: number }): string {
  const secs = (m.duration_ms / 1000).toFixed(1);
  const cost = m.cost_usd > 0 ? ` · $${m.cost_usd.toFixed(2)}` : "";
  return `${m.num_turns} 轮 · ${secs}s${cost}`;
}

export function Console({
  record,
  isLive,
  busy,
  quickTarget,
  onPickTarget,
  onQuickRun,
}: {
  record: RunRecord | null;
  isLive: boolean;
  busy: boolean;
  quickTarget: string | null;
  onPickTarget: () => void;
  onQuickRun: (prompt: string) => void;
}) {
  const state = record?.state ?? null;
  const live = isLive && !!state && !state.runStatus;
  const now = useNow(live);

  return (
    <>
      <div className="pane-header">
        <span className="ph-title">控制台</span>
        {record && <span className="ph-count" title={record.name}>{record.name}</span>}
        <div className="ph-spacer" />
        {live && (
          <button className="btn btn-danger btn-sm" onClick={() => ipc.sendCommand({ type: "Abort" })}>
            中止 Run
          </button>
        )}
        {state?.runStatus && (
          <span className="run-result">
            <span className={state.runStatus}>{state.runStatus}</span>
          </span>
        )}
      </div>

      <div className="pane-inner pane-body">
        {state ? (
          <>
            <div className="console-feed">
              {state.order.map((id) => {
                const st = state.steps[id];
                const running = st.status === "Running";
                // 运行中:显示最近进度行(尚无 summary);终态:显示 summary/error
                const main = running ? st.lastLine ?? "" : st.summary ?? st.error ?? "";
                return (
                  <div key={id} className={`console-line st-${st.status}`}>
                    <span className="mark">{MARK[st.status]}</span>
                    <span className="cid">{id}</span>
                    {running && st.round != null && <span className="cline-round">第 {st.round} 轮</span>}
                    <span className="cline-main">{main}</span>
                    {st.metrics && <span className="cline-metrics">{fmtMetrics(st.metrics)}</span>}
                    {running && st.startedAt != null && (
                      <span className="cline-timer">{fmtElapsed(now - st.startedAt)}</span>
                    )}
                  </div>
                );
              })}
              {Object.entries(state.loops).map(([id, l]) => (
                <div key={id} className="console-line">
                  <span className="mark loop-tick">↻</span>
                  <span className="cid">{id}</span>
                  <span>
                    {l.iteration} 轮{l.result ? `(${l.result})` : ""}
                  </span>
                </div>
              ))}
              {state.order.length === 0 && (
                <div className="console-line" style={{ color: "var(--text-faint)" }}>
                  {live ? "等待引擎事件…" : "无执行记录"}
                </div>
              )}
            </div>

            {state.activeGate && live && (
              <GatePrompt
                key={`${state.activeGate.step_id}:${state.activeGate.gate_kind}`}
                gate={state.activeGate}
              />
            )}
          </>
        ) : (
          <div className="pane-empty">选一条记录查看执行过程,或在下方输入 prompt 快速运行</div>
        )}
      </div>

      <ConsolePromptBar
        busy={busy}
        quickTarget={quickTarget}
        onPickTarget={onPickTarget}
        onQuickRun={onQuickRun}
      />
    </>
  );
}

/** 控制台底部常驻 prompt 栏:输入一段 prompt → 直接跑一个单 claude step 的 run。 */
function ConsolePromptBar({
  busy,
  quickTarget,
  onPickTarget,
  onQuickRun,
}: {
  busy: boolean;
  quickTarget: string | null;
  onPickTarget: () => void;
  onQuickRun: (prompt: string) => void;
}) {
  const [prompt, setPrompt] = useState("");
  const taRef = useRef<HTMLTextAreaElement>(null);

  const targetName = quickTarget ? quickTarget.split(/[\\/]/).pop() : null;
  const canRun = !busy && prompt.trim().length > 0 && !!quickTarget;

  // 随内容自增高(上限交给 CSS max-height + 滚动)
  const autoGrow = (el: HTMLTextAreaElement | null) => {
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  };

  const run = () => {
    if (!canRun) return;
    onQuickRun(prompt.trim());
    setPrompt("");
    if (taRef.current) taRef.current.style.height = "auto";
  };

  return (
    <div className="console-promptbar">
      <div className={`composer-box ${busy ? "is-disabled" : ""}`}>
        <textarea
          ref={taRef}
          className="composer-input"
          placeholder={busy ? "有运行中的 Run,结束后可再运行…" : "输入 prompt,直接运行一个 claude 任务…"}
          value={prompt}
          disabled={busy}
          rows={1}
          onChange={(e) => {
            setPrompt(e.target.value);
            autoGrow(e.target);
          }}
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
              e.preventDefault();
              run();
            }
          }}
        />
        <div className="composer-toolbar">
          <div className="composer-tools">
            <button className="pb-target" onClick={onPickTarget} title="选择 target 仓库(claude 的工作目录)">
              <span className="pb-target-icon">◇</span>
              {targetName ? targetName : "选择 target"}
            </button>
            <span className="pb-warn" title="claude 步骤一律以 bypassPermissions 运行">⚠ 自主写码</span>
          </div>
          <button className="composer-send" onClick={run} disabled={!canRun} title="运行(⌘/Ctrl+Enter)">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 19V5M5 12l7-7 7 7" />
            </svg>
          </button>
        </div>
      </div>
    </div>
  );
}
