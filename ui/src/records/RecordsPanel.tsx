import type { RunSummary } from "../types";

const STATUS_LABEL: Record<string, string> = {
  Success: "成功",
  Failed: "失败",
  Aborted: "已中止",
  running: "运行中",
};

/** 已完成 run 的状态点 CSS class */
function summaryDotClass(s: RunSummary): string {
  if (!s.complete) return "warn";
  return s.status ?? "";
}

export function RecordsPanel({
  summaries,
  liveRun,
  selectedKey,
  onSelectLive,
  onSelectHistory,
  compareIds,
  onToggleCompare,
  onCompare,
}: {
  /** 持久化历史(不含当前活跃 run) */
  summaries: RunSummary[];
  /** 当前活跃 live run;null 表示无运行 */
  liveRun: { id: string; name: string } | null;
  /** 当前选中键:"live" 或 run_id */
  selectedKey: string | null;
  onSelectLive: () => void;
  onSelectHistory: (runId: string) => void;
  /** 选中参与对比的 run_id 列表(最多 2 条) */
  compareIds: string[];
  onToggleCompare: (runId: string) => void;
  onCompare: () => void;
}) {
  const total = (liveRun ? 1 : 0) + summaries.length;

  return (
    <div className="pane pane-left">
      <div className="pane-header">
        <span className="ph-title">记录</span>
        <span className="ph-count">{total}</span>
        {compareIds.length === 2 && (
          <button className="btn btn-primary btn-sm" onClick={onCompare}>
            对比
          </button>
        )}
      </div>
      <div className="pane-inner">
        {total === 0 ? (
          <div className="pane-empty">还没有运行记录，在右侧编排后点运行</div>
        ) : (
          <div className="record-list">
            {/* 活跃 live run 置顶 */}
            {liveRun && (
              <div
                className={`record-item ${selectedKey === "live" ? "selected" : ""}`}
                onClick={onSelectLive}
              >
                <div className="rec-top">
                  <span className="status-dot running" />
                  <span className="rec-name">{liveRun.name || "未命名"}</span>
                </div>
                <div className="rec-meta">运行中…</div>
              </div>
            )}

            {/* 持久化历史列表 */}
            {summaries.map((r) => {
              const dotClass = summaryDotClass(r);
              const isSelected = selectedKey === r.run_id;
              const inCompare = compareIds.includes(r.run_id);
              const canAddCompare = compareIds.length < 2 || inCompare;
              return (
                <div
                  key={r.run_id}
                  className={`record-item ${isSelected ? "selected" : ""}`}
                  onClick={() => onSelectHistory(r.run_id)}
                >
                  <div className="rec-top">
                    {/* 对比 checkbox */}
                    <input
                      type="checkbox"
                      className="compare-checkbox"
                      checked={inCompare}
                      disabled={!canAddCompare}
                      title="加入对比(最多 2 条)"
                      onClick={(e) => e.stopPropagation()}
                      onChange={() => onToggleCompare(r.run_id)}
                    />
                    <span className={`status-dot ${dotClass}`} />
                    <span className="rec-name">{r.name || "未命名"}</span>
                    {!r.complete && <span className="rec-incomplete" title="未完整结束">⚠</span>}
                  </div>
                  <div className="rec-meta">
                    {r.step_count} 步
                    {r.total_cost_usd > 0 && (
                      <span className="rec-cost"> · ${r.total_cost_usd.toFixed(2)}</span>
                    )}
                    {r.status && (
                      <span> · {STATUS_LABEL[r.status] ?? r.status}</span>
                    )}
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
