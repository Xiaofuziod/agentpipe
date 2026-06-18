import { useEffect, useState } from "react";
import type { Manifest, Step } from "../types";
import { ipc } from "../ipc";
import { StepRow } from "./StepCard";
import { StepDrawer } from "./StepDrawer";

const emptyStep = (id: string): Step => ({ id, kind: "claude", prompt: "" });
const emptyManifest = (): Manifest => ({ version: 1, name: "", target: "", mode: "step", worktree: false, steps: [] });

export function Composer({
  target,
  onRun,
}: {
  target: string | null;
  onRun: (path: string) => void;
}) {
  const [m, setM] = useState<Manifest>(emptyManifest);
  const [savePath, setSavePath] = useState("");
  const [templates, setTemplates] = useState<string[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const [editing, setEditing] = useState<number | null>(null);

  useEffect(() => {
    ipc.listTemplates().then(setTemplates).catch(() => setTemplates([]));
  }, []);

  const update = (patch: Partial<Manifest>) => setM({ ...m, ...patch });
  const setStep = (i: number, s: Step) => update({ steps: m.steps.map((x, j) => (j === i ? s : x)) });
  const addStep = () => {
    update({ steps: [...m.steps, emptyStep(`step-${m.steps.length + 1}`)] });
    setEditing(m.steps.length);
  };
  const delStep = (i: number) => {
    update({ steps: m.steps.filter((_, j) => j !== i) });
    setEditing(null);
  };
  const move = (i: number, d: -1 | 1) => {
    const j = i + d;
    if (j < 0 || j >= m.steps.length) return;
    const next = [...m.steps];
    [next[i], next[j]] = [next[j], next[i]];
    update({ steps: next });
  };

  const loadTemplate = async (name: string) => {
    if (!name) return;
    try {
      setM(await ipc.loadTemplate(name));
      setErr(null);
    } catch (e) {
      setErr(String(e));
    }
  };

  // 全应用单一 target(在控制台下方选择)注入 manifest 后再落盘
  const persist = async (): Promise<boolean> => {
    setErr(null);
    setOk(null);
    if (!savePath) {
      setErr("请填保存路径");
      return false;
    }
    if (!target) {
      setErr("请先在控制台下方选择 target 仓库");
      return false;
    }
    try {
      await ipc.saveManifest({ ...m, target }, savePath);
      return true;
    } catch (e) {
      setErr(String(e));
      return false;
    }
  };

  const save = async () => {
    if (await persist()) setOk("已保存");
  };

  // 运行 = 先按当前编辑 + target 落盘,再跑该文件,保证跑的就是屏幕上的内容
  const run = async () => {
    if (await persist()) onRun(savePath);
  };

  return (
    <div className="pane-body">
      {/* 任务级设置 */}
      <div className="card">
        <div className="row" style={{ flexWrap: "nowrap" }}>
          <div className="field" style={{ flex: 1, minWidth: 0 }}>
            <label className="label">模式</label>
            <select
              className="select"
              value={m.mode}
              onChange={(e) => update({ mode: e.target.value as Manifest["mode"] })}
            >
              <option value="step">step(逐步批准)</option>
              <option value="auto">auto(自动跑)</option>
            </select>
          </div>
          <div className="field" style={{ flex: 1, minWidth: 0 }}>
            <label className="label">从模板新建</label>
            <select className="select" defaultValue="" onChange={(e) => loadTemplate(e.target.value)}>
              <option value="">选择模板…</option>
              {templates.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </div>
        </div>
        <label className="checkbox" style={{ marginTop: 12 }} title="在 target 仓库的一个新 git worktree(新分支)里跑,不改动当前工作区;跑完保留供 review/合并">
          <input
            type="checkbox"
            checked={!!m.worktree}
            onChange={(e) => update({ worktree: e.target.checked })}
          />
          在隔离 git worktree 中运行(不改动 target 工作区)
        </label>
        {m.mode === "auto" && hasClaude(m.steps) && (
          <div className="banner banner-warn">
            <span>⚠</span>
            <span>
              auto 模式:claude 步骤将以 bypassPermissions 在 target 仓库完全自主写码,确认 target 可信。
            </span>
          </div>
        )}
      </div>

      {/* 步骤列表 — 紧凑行,点编辑进抽屉 */}
      <h2 className="page-title" style={{ marginTop: 24, fontSize: 15 }}>
        步骤 <span className="dim">{m.steps.length}</span>
      </h2>
      <div className="step-list">
        {m.steps.map((s, i) => (
          <StepRow
            key={i}
            step={s}
            seq={i + 1}
            onEdit={() => setEditing(i)}
            onUp={() => move(i, -1)}
            onDown={() => move(i, 1)}
            onDelete={() => delStep(i)}
          />
        ))}
        {m.steps.length === 0 && (
          <div className="step-row" style={{ color: "var(--text-faint)", justifyContent: "center" }}>
            还没有步骤,点下方添加
          </div>
        )}
      </div>
      <div className="add-step">
        <button className="btn" onClick={addStep}>
          + 加步骤
        </button>
      </div>

      {/* 保存 / 运行 */}
      <div className="card" style={{ marginTop: 24 }}>
        <div className="field">
          <label className="label">保存到 task.yaml</label>
          <div className="row" style={{ flexWrap: "nowrap" }}>
            <input
              className="input input-mono"
              placeholder="~/tasks/my-task.yaml"
              value={savePath}
              onChange={(e) => setSavePath(e.target.value)}
            />
            <button className="btn" onClick={save}>
              保存
            </button>
            <button className="btn btn-primary" onClick={run} disabled={!savePath}>
              ▷ 运行
            </button>
          </div>
        </div>
        {err && <div className="banner banner-error">{err}</div>}
        {ok && <div className="banner banner-ok">{ok}</div>}
      </div>

      {editing !== null && m.steps[editing] && (
        <StepDrawer
          step={m.steps[editing]}
          onChange={(s) => setStep(editing, s)}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  );
}

function hasClaude(steps: Step[]): boolean {
  return steps.some((s) => {
    if (s.kind === "claude") return true;
    if (s.kind === "loop") return hasClaude(s.body);
    return false;
  });
}
