import { useEffect, useMemo, useState } from "react";
import type { Manifest, Step } from "../types";
import { ipc } from "../ipc";
import { StepRow } from "./StepCard";
import { StepDrawer } from "./StepDrawer";

const emptyStep = (id: string): Step => ({ id, kind: "claude", prompt: "" });
// GUI 一律 auto(逐步批准模式已下线,引擎仍兼容 step YAML)。
const emptyManifest = (): Manifest => ({ version: 1, name: "", target: "", mode: "auto", worktree: false, steps: [] });

/** 顶层带 expects 的 human 步骤 = 启动任务的「必要条件」,启动表单逐条预填。 */
function humanInputs(steps: Step[]): Array<{ id: string; instruction: string; expects: string }> {
  return steps.flatMap((s) =>
    s.kind === "human" && s.expects
      ? [{ id: s.id, instruction: s.instruction, expects: s.expects }]
      : [],
  );
}

export function Composer({
  target,
  onLaunch,
}: {
  target: string | null;
  /** 启动任务:把内联 manifest(已注入 target / 预置条件)交给宿主 inline 跑。 */
  onLaunch: (manifest: Manifest) => void;
}) {
  const [m, setM] = useState<Manifest>(emptyManifest);
  const [savePath, setSavePath] = useState("");
  const [templates, setTemplates] = useState<string[]>([]);
  // 启动条件值:human 步骤 id → 用户填的值(仅用于本次启动,不写进模版)
  const [launchValues, setLaunchValues] = useState<Record<string, string>>({});
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
      const loaded = await ipc.loadTemplate(name);
      setM({ ...loaded, mode: "auto" }); // GUI 不暴露模式,一律 auto
      setLaunchValues({}); // 换模板,旧条件作废
      setErr(null);
      setOk(null);
    } catch (e) {
      setErr(String(e));
    }
  };

  // 必要条件(随步骤变)
  const inputs = useMemo(() => humanInputs(m.steps), [m.steps]);

  // 保存模版:当前编辑(注入 target、恒 auto)落盘,不含本次启动填的条件。
  const save = async () => {
    setErr(null);
    setOk(null);
    if (!savePath) {
      setErr("请填模版保存路径");
      return;
    }
    if (!target) {
      setErr("请先在控制台下方选择 target 仓库");
      return;
    }
    try {
      await ipc.saveManifest({ ...m, mode: "auto", target }, savePath);
      setOk("已保存模版");
    } catch (e) {
      setErr(String(e));
    }
  };

  // 启动任务:把必要条件注入对应 human 步骤的 value,inline 跑(无需先存盘)。
  const launch = () => {
    setErr(null);
    setOk(null);
    if (m.steps.length === 0) {
      setErr("还没有步骤,先编排或从模板新建");
      return;
    }
    if (!target) {
      setErr("请先在控制台下方选择 target 仓库");
      return;
    }
    const steps = m.steps.map((s) =>
      s.kind === "human" && s.expects && launchValues[s.id]?.trim()
        ? { ...s, value: launchValues[s.id].trim() }
        : s,
    );
    onLaunch({ ...m, mode: "auto", target, steps, name: m.name || "编排任务" });
  };

  return (
    <div className="pane-body">
      {/* 任务级设置 */}
      <div className="card">
        <div className="field">
          <label className="label">从模板新建</label>
          <select className="select" value="" onChange={(e) => loadTemplate(e.target.value)}>
            <option value="">选择模板…</option>
            {templates.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        </div>
        <label className="checkbox" style={{ marginTop: 12 }} title="在 target 仓库的一个新 git worktree(新分支)里跑,不改动当前工作区;跑完保留供 review/合并">
          <input
            type="checkbox"
            checked={!!m.worktree}
            onChange={(e) => update({ worktree: e.target.checked })}
          />
          在隔离 git worktree 中运行(不改动 target 工作区)
        </label>
        {hasClaude(m.steps) && (
          <div className="banner banner-warn">
            <span>⚠</span>
            <span>
              claude 步骤将以 bypassPermissions 在 target 仓库完全自主写码,确认 target 可信。
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

      {/* 保存为模版(纯编排产出,可复用) */}
      <div className="card" style={{ marginTop: 24 }}>
        <div className="field">
          <label className="label">
            保存为模版
            <span className="hint">存成可复用的 task.yaml,不含本次启动填的条件</span>
          </label>
          <div className="row" style={{ flexWrap: "nowrap" }}>
            <input
              className="input input-mono"
              placeholder="my-task(默认存到 ~/.agentpipe/tasks/)或绝对路径 ~/tasks/x.yaml"
              value={savePath}
              onChange={(e) => setSavePath(e.target.value)}
            />
            <button className="btn" onClick={save} disabled={!savePath}>
              保存模版
            </button>
          </div>
        </div>
      </div>

      {/* 启动任务(填必要条件直接跑,无需先存盘) */}
      <div className="card" style={{ marginTop: 12 }}>
        <label className="label">
          启动任务
          <span className="hint">填好必要条件直接运行,无需先保存</span>
        </label>
        {inputs.length > 0 ? (
          <div style={{ marginTop: 4 }}>
            {inputs.map((it) => (
              <div className="field" key={it.id} style={{ marginTop: 10 }}>
                <label className="label" style={{ fontWeight: 500 }} title={it.instruction}>
                  {it.id}
                  <span className="hint">{it.expects}</span>
                </label>
                <input
                  className="input"
                  placeholder={it.instruction}
                  value={launchValues[it.id] ?? ""}
                  onChange={(e) => setLaunchValues((v) => ({ ...v, [it.id]: e.target.value }))}
                />
              </div>
            ))}
          </div>
        ) : (
          <div style={{ marginTop: 6, fontSize: 12, color: "var(--text-muted)" }}>
            该任务无需预填条件,直接运行即可。
          </div>
        )}
        <button
          className="btn btn-primary"
          style={{ marginTop: 14 }}
          onClick={launch}
          disabled={m.steps.length === 0}
        >
          ▷ 运行
        </button>
      </div>

      {err && <div className="banner banner-error" style={{ marginTop: 12 }}>{err}</div>}
      {ok && <div className="banner banner-ok" style={{ marginTop: 12 }}>{ok}</div>}

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
