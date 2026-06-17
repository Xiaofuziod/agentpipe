import type { RunRecord } from "../state/useRuns";

const STATUS_LABEL: Record<string, string> = {
  Success: "成功",
  Failed: "失败",
  Aborted: "已中止",
  running: "运行中",
};

function fmtTime(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export function RecordsPanel({
  records,
  selectedId,
  activeId,
  onSelect,
}: {
  records: RunRecord[];
  selectedId: string | null;
  activeId: string | null;
  onSelect: (id: string) => void;
}) {
  return (
    <div className="pane pane-left">
      <div className="pane-header">
        <span className="ph-title">记录</span>
        <span className="ph-count">{records.length}</span>
      </div>
      <div className="pane-inner">
        {records.length === 0 ? (
          <div className="pane-empty">还没有运行记录,在右侧编排后点运行</div>
        ) : (
          <div className="record-list">
            {records.map((r) => {
              const status = r.state.runStatus ?? (r.id === activeId ? "running" : null);
              const dotClass = status === "running" ? "running" : status ?? "";
              const stepCount = r.state.order.length;
              return (
                <div
                  key={r.id}
                  className={`record-item ${r.id === selectedId ? "selected" : ""}`}
                  onClick={() => onSelect(r.id)}
                >
                  <div className="rec-top">
                    <span className={`status-dot ${dotClass}`} />
                    <span className="rec-name">{r.name || "未命名"}</span>
                  </div>
                  <div className="rec-meta">
                    {fmtTime(r.startedAt)} · {stepCount} 步{status ? ` · ${STATUS_LABEL[status] ?? status}` : ""}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
