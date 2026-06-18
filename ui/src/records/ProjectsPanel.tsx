import { useState } from "react";
import type { RunSummary } from "../types";
import { groupByProject, projectName, type Project } from "../state/projects";

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

/** live run 信息(尚未稳定落盘);target 用于归入对应项目。 */
export interface LiveRunInfo {
  id: string;
  name: string;
  target: string;
}

export function ProjectsPanel({
  summaries,
  liveRun,
  activeTarget,
  selectedKey,
  onSelectProject,
  onSelectLive,
  onSelectHistory,
  compareIds,
  onToggleCompare,
  onCompare,
}: {
  /** 持久化历史(不含当前活跃 run) */
  summaries: RunSummary[];
  /** 当前活跃 live run;null 表示无运行 */
  liveRun: LiveRunInfo | null;
  /** 当前活跃项目 target(快跑栏选中的);用于高亮 */
  activeTarget: string | null;
  /** 当前选中键:"live" 或 run_id */
  selectedKey: string | null;
  /** 点项目头:设为活跃项目 */
  onSelectProject: (target: string) => void;
  onSelectLive: () => void;
  onSelectHistory: (runId: string) => void;
  /** 选中参与对比的 run_id 列表(最多 2 条) */
  compareIds: string[];
  onToggleCompare: (runId: string) => void;
  onCompare: () => void;
}) {
  // 折叠的项目 target 集合(默认全展开)
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  // 按 target 归类历史 run
  const projects: Project[] = groupByProject(summaries);

  // live run 合并进对应项目:无匹配项目则合成一个置顶的空项目(全新 target 首跑)
  if (liveRun) {
    const exists = projects.some((p) => p.target === liveRun.target);
    if (!exists) {
      projects.unshift({
        target: liveRun.target,
        name: projectName(liveRun.target),
        runs: [],
        totalCost: 0,
        latestRunId: "",
      });
    }
  }

  const totalRuns = (liveRun ? 1 : 0) + summaries.length;

  const toggleCollapse = (target: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(target)) next.delete(target);
      else next.add(target);
      return next;
    });

  return (
    <div className="pane pane-left">
      <div className="pane-header">
        <span className="ph-title">项目</span>
        <span className="ph-count">{totalRuns}</span>
        {compareIds.length === 2 && (
          <button className="btn btn-primary btn-sm" onClick={onCompare}>
            对比
          </button>
        )}
      </div>
      <div className="pane-inner">
        {projects.length === 0 ? (
          <div className="pane-empty">还没有运行记录，在下方输入 prompt 或右侧编排后点运行</div>
        ) : (
          <div className="project-list">
            {projects.map((p) => {
              const isCollapsed = collapsed.has(p.target);
              const isActive = activeTarget === p.target && p.target !== "";
              const showLive = liveRun?.target === p.target;
              return (
                <div key={p.target || "__ungrouped__"} className="project-group">
                  <div className={`project-head ${isActive ? "active" : ""}`}>
                    <button
                      className="project-chevron"
                      title={isCollapsed ? "展开" : "收起"}
                      onClick={() => toggleCollapse(p.target)}
                    >
                      {isCollapsed ? "▸" : "▾"}
                    </button>
                    <button
                      className="project-name-btn"
                      title={p.target || "未指定项目（旧记录或未选目录）"}
                      onClick={() => p.target && onSelectProject(p.target)}
                    >
                      <span className="project-folder">▢</span>
                      <span className="project-name">{p.name}</span>
                      <span className="project-count">{p.runs.length + (showLive ? 1 : 0)}</span>
                      {p.totalCost > 0 && (
                        <span className="project-cost">${p.totalCost.toFixed(2)}</span>
                      )}
                    </button>
                  </div>

                  {!isCollapsed && (
                    <div className="project-runs">
                      {/* 活跃 live run 置顶于所属项目 */}
                      {showLive && liveRun && (
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

                      {p.runs.map((r) => {
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
                              {r.status && <span> · {STATUS_LABEL[r.status] ?? r.status}</span>}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
