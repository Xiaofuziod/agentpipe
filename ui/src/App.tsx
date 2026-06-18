import { useEffect, useRef, useState } from "react";
import { Composer } from "./composer/Composer";
import { Console } from "./console/Console";
import { RecordsPanel } from "./records/RecordsPanel";
import { DiffView } from "./records/DiffView";
import { useRuns } from "./state/useRuns";
import { useHistory } from "./state/useHistory";
import { ipc } from "./ipc";
import type { Manifest, DiffRow } from "./types";
import type { RunState } from "./state/runReducer";

/** 选中态:live run | 持久化历史回看 | 无 */
type Selection =
  | { kind: "live" }
  | { kind: "history"; runId: string; state: RunState }
  | null;

export default function App() {
  const runs = useRuns();
  const hist = useHistory();
  const [composerOpen, setComposerOpen] = useState(true);
  const [target, setTarget] = useState<string | null>(null);

  // 选中态
  const [selection, setSelection] = useState<Selection>(null);

  // 对比
  const [compareIds, setCompareIds] = useState<string[]>([]);
  const [diffRows, setDiffRows] = useState<DiffRow[] | null>(null);
  const [diffNames, setDiffNames] = useState<[string, string]>(["A", "B"]);

  // 用于在 run 结束后将 liveRunId 关联到持久化记录,以便高亮
  const liveRunIdRef = useRef<string | null>(null);

  // 监听 engine://run-started-id 拿到持久化 run_id,存 ref 供结束后 refresh 时高亮
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    ipc.onRunStartedId((runId) => {
      liveRunIdRef.current = runId;
    }).then((un) => {
      unlisten = un;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // RunFinished 检测:activeId 由非 null 变 null → 刷新历史列表
  const prevActiveId = useRef<string | null>(null);
  useEffect(() => {
    const prev = prevActiveId.current;
    const cur = runs.activeId;
    if (prev !== null && cur === null) {
      // run 刚结束
      hist.refresh();
      // 结束后切换选中到 live 记录的最终快照(in-memory),保持用户能看到结果
      // live 条已包含终态,不需要额外切换; selectedId 还指向 live 记录
    }
    prevActiveId.current = cur;
  }, [runs.activeId, hist]);

  // 同步 useRuns 内部的 selectedId 变化 → 如果 live record 被 useRuns 自动选中,同步到 selection
  useEffect(() => {
    if (runs.selectedId && runs.selectedId === runs.activeId) {
      setSelection({ kind: "live" });
    }
  }, [runs.selectedId, runs.activeId]);

  const pickTarget = async () => {
    const d = await ipc.pickDir();
    if (d) setTarget(d);
  };

  const quickRun = (prompt: string) => {
    if (!target) return;
    const manifest: Manifest = {
      version: 1,
      name: prompt.split("\n")[0].slice(0, 40) || "快速运行",
      target,
      mode: "auto",
      steps: [{ id: "task-1", kind: "claude", prompt }],
    };
    runs.startInline(manifest);
    setSelection({ kind: "live" });
  };

  // 选择 live run
  const handleSelectLive = () => {
    if (runs.activeId) {
      runs.select(runs.activeId);
    } else if (runs.records.length > 0) {
      runs.select(runs.records[0].id);
    }
    setSelection({ kind: "live" });
  };

  // 选择历史 run → 加载 RunState 并切到只读回看
  const handleSelectHistory = async (runId: string) => {
    const state = await hist.openState(runId);
    setSelection({ kind: "history", runId, state });
  };

  // 对比 checkbox 切换
  const handleToggleCompare = (runId: string) => {
    setCompareIds((prev) => {
      if (prev.includes(runId)) return prev.filter((id) => id !== runId);
      if (prev.length >= 2) return prev; // 满了不再添加
      return [...prev, runId];
    });
  };

  // 触发 diff
  const handleCompare = async () => {
    if (compareIds.length !== 2) return;
    const [a, b] = compareIds;
    const rows = await ipc.diffRuns(a, b);
    const nameA = hist.summaries.find((s) => s.run_id === a)?.name ?? a;
    const nameB = hist.summaries.find((s) => s.run_id === b)?.name ?? b;
    setDiffNames([nameA, nameB]);
    setDiffRows(rows);
  };

  // 当前 live run(用于置顶显示)
  const liveRun = runs.activeId
    ? { id: runs.activeId, name: runs.records.find((r) => r.id === runs.activeId)?.name ?? "运行中" }
    : null;

  // 去重:live 运行中时 summaries 里可能已有同一个 run_id;但 summaries 来自持久化,
  // live 期间持久化条还不稳定,用 liveRunIdRef 排除。
  const filteredSummaries = liveRunIdRef.current
    ? hist.summaries.filter((s) => s.run_id !== liveRunIdRef.current)
    : hist.summaries;

  // 选中键:live → "live",历史 → run_id
  const selectedKey = selection?.kind === "live" ? "live" : selection?.kind === "history" ? selection.runId : null;

  // Console 的 props
  // live 路径:selection.kind==="live" 时用 useRuns 的 live record
  // 回看路径:selection.kind==="history" 时传 replayState
  const liveRecord = selection?.kind === "live" ? runs.selected : null;
  const replayState = selection?.kind === "history" ? selection.state : undefined;
  const isLive = selection?.kind === "live" && !!runs.activeId;

  return (
    <div className="app">
      <div className="workspace">
        {/* 左:运行记录(历史 + live 置顶) */}
        <RecordsPanel
          summaries={filteredSummaries}
          liveRun={liveRun}
          selectedKey={selectedKey}
          onSelectLive={handleSelectLive}
          onSelectHistory={handleSelectHistory}
          compareIds={compareIds}
          onToggleCompare={handleToggleCompare}
          onCompare={handleCompare}
        />

        {/* 中:控制台 */}
        <div className="pane pane-center">
          <Console
            record={liveRecord}
            isLive={isLive}
            busy={runs.activeId !== null}
            quickTarget={target}
            onPickTarget={pickTarget}
            onQuickRun={quickRun}
            replayState={replayState}
          />
        </div>

        {/* 右:编排任务 */}
        <div className={`pane pane-right ${composerOpen ? "" : "collapsed"}`}>
          <div className="pane-header">
            <span className="ph-title">编排任务</span>
            <button className="btn-icon collapse-toggle" title="收起" onClick={() => setComposerOpen(false)}>
              ▸
            </button>
          </div>
          <div className="pane-inner">
            <Composer target={target} onRun={(p) => { runs.start(p); setSelection({ kind: "live" }); }} />
          </div>
        </div>

        {/* 收起后右上角展开手柄 */}
        {!composerOpen && (
          <button className="expand-handle" title="展开编排面板" onClick={() => setComposerOpen(true)}>
            ◂ 编排
          </button>
        )}
      </div>

      {/* Diff 浮层 */}
      {diffRows !== null && (
        <DiffView
          rows={diffRows}
          nameA={diffNames[0]}
          nameB={diffNames[1]}
          onClose={() => setDiffRows(null)}
        />
      )}
    </div>
  );
}
