import { useState } from "react";
import { Composer } from "./composer/Composer";
import { Console } from "./console/Console";
import { RecordsPanel } from "./records/RecordsPanel";
import { useRuns } from "./state/useRuns";
import { ipc } from "./ipc";
import type { Manifest } from "./types";

export default function App() {
  const runs = useRuns();
  const [composerOpen, setComposerOpen] = useState(true);
  // 全应用单一 target 仓库(claude/codex 的工作目录),编排与快捷运行共用
  const [target, setTarget] = useState<string | null>(null);

  const pickTarget = async () => {
    const d = await ipc.pickDir();
    if (d) setTarget(d);
  };

  // 控制台底部 prompt 栏:拼一个单 claude step 的 manifest 直接跑(auto 模式,无 gate)
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
  };

  return (
    <div className="app">
      <div className="workspace">
        {/* 左:运行记录 */}
        <RecordsPanel
          records={runs.records}
          selectedId={runs.selectedId}
          activeId={runs.activeId}
          onSelect={runs.select}
        />

        {/* 中:控制台 — 显示选中记录的执行过程 */}
        <div className="pane pane-center">
          <Console
            record={runs.selected}
            isLive={!!runs.selected && runs.selected.id === runs.activeId}
            busy={runs.activeId !== null}
            quickTarget={target}
            onPickTarget={pickTarget}
            onQuickRun={quickRun}
          />
        </div>

        {/* 右:编排任务 — 可收起 */}
        <div className={`pane pane-right ${composerOpen ? "" : "collapsed"}`}>
          <div className="pane-header">
            <span className="ph-title">编排任务</span>
            <button className="btn-icon collapse-toggle" title="收起" onClick={() => setComposerOpen(false)}>
              ▸
            </button>
          </div>
          <div className="pane-inner">
            <Composer target={target} onRun={(p) => runs.start(p)} />
          </div>
        </div>

        {/* 收起后右上角的展开手柄 */}
        {!composerOpen && (
          <button className="expand-handle" title="展开编排面板" onClick={() => setComposerOpen(true)}>
            ◂ 编排
          </button>
        )}
      </div>
    </div>
  );
}
