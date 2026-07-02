import type { DiffRow } from "../types";

/** 纯函数:把一行 DiffRow 转成可读文案(单测覆盖)。
 *
 * 应用内不展示 USD 计费(用户裁决,2026-06-26):只 diff 状态变化,cost 字段
 * 仍由后端提供但 UI 不渲染。后端 audit / CLI cost 子命令仍保留账目。 */
export function diffRowText(r: DiffRow): string {
  if (r.kind === "only_a") return `- ${r.step_id}: 仅 A (${r.a_status})`;
  if (r.kind === "only_b") return `+ ${r.step_id}: 仅 B (${r.b_status})`;
  return `~ ${r.step_id}: ${r.a_status} → ${r.b_status}`;
}

/**
 * DiffView — 以浮层抽屉展示两次 run 的 step 对比结果。
 * 空 rows 时显示「无差异」。
 */
export function DiffView({
  rows,
  nameA,
  nameB,
  onClose,
}: {
  rows: DiffRow[];
  nameA: string;
  nameB: string;
  onClose: () => void;
}) {
  return (
    <>
      <div className="drawer-overlay" onClick={onClose} />
      <div className="drawer diff-drawer">
        <div className="drawer-header">
          <span className="title">对比结果</span>
          <button className="btn btn-ghost btn-sm" onClick={onClose}>✕</button>
        </div>
        <div className="diff-run-labels">
          <span className="diff-label diff-label-a">A: {nameA}</span>
          <span className="diff-label diff-label-b">B: {nameB}</span>
        </div>
        <div className="drawer-body">
          {rows.length === 0 ? (
            <div className="pane-empty">无差异</div>
          ) : (
            <div className="diff-list">
              {rows.map((r) => (
                <div
                  key={r.step_id}
                  className={`diff-row diff-row-${r.kind}`}
                >
                  {diffRowText(r)}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </>
  );
}
