import { useEffect, useMemo, useRef, useState } from "react";
import { Composer } from "./composer/Composer";
import { Console } from "./console/Console";
import { ProjectsPanel } from "./records/ProjectsPanel";
import { DiffView } from "./records/DiffView";
import { useRuns } from "./state/useRuns";
import { useHistory } from "./state/useHistory";
import { groupByProject, mostRecentTarget } from "./state/projects";
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
    }
    prevActiveId.current = cur;
  }, [runs.activeId, hist]);

  // Run 结束后:当 hist.summaries 刷新时,若仍在 live 选中态且 run 已结束,
  // 尝试切换到对应持久化记录(高亮左侧行,保持内容一致)
  useEffect(() => {
    if (
      selection?.kind !== "live" ||
      runs.activeId !== null ||
      liveRunIdRef.current === null
    ) {
      return;
    }
    const liveId = liveRunIdRef.current;
    const matched = hist.summaries.find((s) => s.run_id === liveId);
    if (!matched) return;
    // 找到持久化记录:切换选中并清除 ref(只触发一次)
    liveRunIdRef.current = null;
    hist.openState(liveId).then((state) => {
      setSelection({ kind: "history", runId: liveId, state });
    });
  }, [hist.summaries, selection, runs.activeId, hist]);

  // 同步 useRuns 内部的 selectedId 变化 → 如果 live record 被 useRuns 自动选中,同步到 selection
  useEffect(() => {
    if (runs.selectedId && runs.selectedId === runs.activeId) {
      setSelection({ kind: "live" });
    }
  }, [runs.selectedId, runs.activeId]);

  // 项目化:按 target 把历史 run 归类(快跑栏下拉 + 默认 target 用;memo 化避免每渲染
  // 重算新数组拖累下方 effect 依赖。左列 ProjectsPanel 走 filteredSummaries 自行归类)。
  const projects = useMemo(() => groupByProject(hist.summaries), [hist.summaries]);

  // 默认活跃 target = 最近用过的项目(首次有历史且尚未选 target 时)
  useEffect(() => {
    if (target !== null) return;
    const recent = mostRecentTarget(projects);
    if (recent) setTarget(recent);
  }, [projects, target]);

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

  // 删除一条历史 run:落盘文件删掉 → 刷新列表;清理选中态 / 对比态。
  const handleDeleteRun = async (runId: string) => {
    await ipc.deleteRun(runId);
    hist.refresh();
    setCompareIds((prev) => prev.filter((id) => id !== runId));
    setSelection((prev) =>
      prev?.kind === "history" && prev.runId === runId ? null : prev,
    );
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

  // 当前 live run(用于置顶显示)。target 取自 run state(RunStarted 注入);
  // RunStarted 到达前回退到活跃 target(quick/编排 启动时用的就是它)。
  const liveRecord0 = runs.activeId ? runs.records.find((r) => r.id === runs.activeId) : null;
  const liveRun = runs.activeId
    ? {
        id: runs.activeId,
        name: liveRecord0?.name ?? "运行中",
        target: liveRecord0?.state.target || target || "",
      }
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
        {/* 左:项目(按 target 归类的 run) */}
        <ProjectsPanel
          summaries={filteredSummaries}
          liveRun={liveRun}
          activeTarget={target}
          selectedKey={selectedKey}
          onSelectProject={setTarget}
          onSelectLive={handleSelectLive}
          onSelectHistory={handleSelectHistory}
          onDeleteRun={handleDeleteRun}
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
            knownTargets={projects.filter((p) => p.target !== "").map((p) => ({ target: p.target, name: p.name }))}
            onPickTarget={pickTarget}
            onSelectTarget={setTarget}
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
            <Composer target={target} onLaunch={(man) => { runs.startInline(man); setSelection({ kind: "live" }); }} />
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
