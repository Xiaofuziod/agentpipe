import { useEffect, useState } from "react";
import type { Manifest, Step } from "../types";
import { ipc } from "../ipc";
import { StepCard } from "./StepCard";

const emptyStep = (id: string): Step => ({ id, kind: "claude", prompt: "" });
const emptyManifest = (): Manifest => ({ version: 1, name: "", target: "", mode: "step", steps: [] });

export function Composer({ onRun }: { onRun: (path: string) => void }) {
  const [m, setM] = useState<Manifest>(emptyManifest);
  const [savePath, setSavePath] = useState("");
  const [templates, setTemplates] = useState<string[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);

  useEffect(() => {
    ipc.listTemplates().then(setTemplates).catch(() => setTemplates([]));
  }, []);

  const update = (patch: Partial<Manifest>) => setM({ ...m, ...patch });
  const setStep = (i: number, s: Step) => update({ steps: m.steps.map((x, j) => (j === i ? s : x)) });
  const addStep = () => update({ steps: [...m.steps, emptyStep(`step-${m.steps.length + 1}`)] });
  const delStep = (i: number) => update({ steps: m.steps.filter((_, j) => j !== i) });
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

  const hasAutoWrites = m.mode === "auto" && hasAllowWrites(m.steps);

  const save = async () => {
    setErr(null);
    setOk(null);
    if (!savePath) {
      setErr("请填保存路径");
      return;
    }
    try {
      await ipc.saveManifest(m, savePath);
      setOk("已保存");
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <div style={{ padding: 12, fontFamily: "system-ui", maxWidth: 720 }}>
      <h3>编排</h3>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
        <input placeholder="任务名" value={m.name} onChange={(e) => update({ name: e.target.value })} />
        <button
          onClick={async () => {
            const d = await ipc.pickDir();
            if (d) update({ target: d });
          }}
        >
          target:{m.target || "(未选)"}
        </button>
        <select value={m.mode} onChange={(e) => update({ mode: e.target.value as Manifest["mode"] })}>
          <option value="step">step(逐步批准)</option>
          <option value="auto">auto(自动跑)</option>
        </select>
        <select defaultValue="" onChange={(e) => loadTemplate(e.target.value)}>
          <option value="">从模板新建…</option>
          {templates.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
      </div>

      {hasAutoWrites && (
        <div style={{ color: "#b35900", marginTop: 6 }}>
          ⚠ auto 模式 + allow_writes:claude 将以 bypassPermissions 在 target 仓库完全自主写码,确认 target 可信。
        </div>
      )}

      <div style={{ marginTop: 8 }}>
        {m.steps.map((s, i) => (
          <StepCard
            key={i}
            step={s}
            onChange={(s2) => setStep(i, s2)}
            onUp={() => move(i, -1)}
            onDown={() => move(i, 1)}
            onDelete={() => delStep(i)}
          />
        ))}
        <button onClick={addStep}>+ 加步骤</button>
      </div>

      <div style={{ display: "flex", gap: 8, marginTop: 12, alignItems: "center" }}>
        <input
          placeholder="保存到 task.yaml 路径"
          value={savePath}
          onChange={(e) => setSavePath(e.target.value)}
          style={{ flex: 1 }}
        />
        <button onClick={save}>保存</button>
        <button onClick={() => onRun(savePath)} disabled={!savePath}>
          运行
        </button>
      </div>
      {err && <div style={{ color: "red", marginTop: 6 }}>{err}</div>}
      {ok && <div style={{ color: "green", marginTop: 6 }}>{ok}</div>}
    </div>
  );
}

function hasAllowWrites(steps: Step[]): boolean {
  return steps.some((s) => {
    if (s.kind === "claude") return !!s.allow_writes;
    if (s.kind === "loop") return hasAllowWrites(s.body);
    return false;
  });
}
