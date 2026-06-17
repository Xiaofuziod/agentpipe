import { useState } from "react";
import type { Step, StepKind, CodexAction } from "../types";
import { KINDS, defaultsFor, StepRow } from "./StepCard";

/** 单个 step 的编辑抽屉,从右侧滑入。loop 的子步骤递归用嵌套抽屉(depth+1 叠在更上层)。 */
export function StepDrawer({
  step,
  depth = 0,
  onChange,
  onClose,
}: {
  step: Step;
  depth?: number;
  onChange: (s: Step) => void;
  onClose: () => void;
}) {
  const setKind = (kind: StepKind["kind"]) => onChange({ id: step.id, ...defaultsFor(kind) });

  return (
    <>
      <div className="drawer-overlay" style={{ zIndex: 100 + depth * 2 }} onClick={onClose} />
      <div className="drawer" style={{ zIndex: 101 + depth * 2 }}>
        <div className="drawer-header">
          <span className={`badge badge-${step.kind}`}>{step.kind}</span>
          <span className="title">编辑步骤</span>
          <button className="btn-icon" title="关闭" onClick={onClose}>
            ✕
          </button>
        </div>

        <div className="drawer-body">
          <div className="row" style={{ gap: 8 }}>
            <div className="field" style={{ flex: 1 }}>
              <label className="label">id</label>
              <input
                className="input input-mono"
                placeholder="step-1"
                value={step.id}
                onChange={(e) => onChange({ ...step, id: e.target.value })}
              />
            </div>
            <div className="field" style={{ width: 140 }}>
              <label className="label">类型</label>
              <select
                className="select"
                value={step.kind}
                onChange={(e) => setKind(e.target.value as StepKind["kind"])}
              >
                {KINDS.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <Fields step={step} depth={depth} onChange={onChange} />
        </div>

        <div className="drawer-footer">
          <button className="btn btn-primary" onClick={onClose}>
            完成
          </button>
        </div>
      </div>
    </>
  );
}

function Fields({
  step,
  depth,
  onChange,
}: {
  step: Step;
  depth: number;
  onChange: (s: Step) => void;
}) {
  switch (step.kind) {
    case "claude":
      return (
        <>
          <div className="field">
            <label className="label">prompt</label>
            <textarea
              className="textarea"
              placeholder="给 Claude 的指令…"
              value={step.prompt}
              onChange={(e) => onChange({ ...step, prompt: e.target.value })}
            />
          </div>
          <div className="field">
            <label className="label">
              skill<span className="hint">可选</span>
            </label>
            <input
              className="input input-mono"
              placeholder="brainstorming"
              value={step.skill ?? ""}
              onChange={(e) => onChange({ ...step, skill: e.target.value || undefined })}
            />
          </div>
          <div className="hint" style={{ marginTop: 4 }}>
            claude 步骤一律以 CLI 最高权限(bypassPermissions)运行,可自主写码 / 提交。
          </div>
        </>
      );
    case "codex":
      return (
        <>
          <div className="field">
            <label className="label">action</label>
            <select
              className="select"
              value={step.action}
              onChange={(e) => onChange({ ...step, action: e.target.value as CodexAction })}
            >
              <option value="review-mr">review-mr</option>
              <option value="review-doc">review-doc</option>
              <option value="ask">ask</option>
            </select>
          </div>
          {step.action === "review-mr" && (
            <div className="field">
              <label className="label">base 分支</label>
              <input
                className="input input-mono"
                placeholder="dev"
                value={step.base ?? ""}
                onChange={(e) => onChange({ ...step, base: e.target.value })}
              />
            </div>
          )}
          {step.action === "review-doc" && (
            <div className="field">
              <label className="label">文档路径</label>
              <input
                className="input input-mono"
                placeholder="docs/spec.md"
                value={step.path ?? ""}
                onChange={(e) => onChange({ ...step, path: e.target.value })}
              />
            </div>
          )}
          {step.action === "ask" && (
            <div className="field">
              <label className="label">问题</label>
              <textarea
                className="textarea"
                placeholder="想问 Codex 的问题…"
                value={step.prompt ?? ""}
                onChange={(e) => onChange({ ...step, prompt: e.target.value })}
              />
            </div>
          )}
        </>
      );
    case "human":
      return (
        <>
          <div className="field">
            <label className="label">instruction</label>
            <textarea
              className="textarea"
              placeholder="给人的操作说明…"
              value={step.instruction}
              onChange={(e) => onChange({ ...step, instruction: e.target.value })}
            />
          </div>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={step.expects !== undefined}
              onChange={(e) => onChange({ ...step, expects: e.target.checked ? "artifact" : undefined })}
            />
            需要产物(artifact)
          </label>
        </>
      );
    case "loop":
      return <LoopBody step={step} depth={depth} onChange={onChange} />;
  }
}

function LoopBody({
  step,
  depth,
  onChange,
}: {
  step: Extract<Step, { kind: "loop" }>;
  depth: number;
  onChange: (s: Step) => void;
}) {
  const [editing, setEditing] = useState<number | null>(null);

  const setBody = (body: Step[]) => onChange({ ...step, body });
  const setSub = (i: number, s: Step) => setBody(step.body.map((x, j) => (j === i ? s : x)));
  const add = () => {
    setBody([
      ...step.body,
      { id: `${step.id}-body-${step.body.length + 1}`, kind: "codex", action: "review-mr", base: "dev" },
    ]);
    setEditing(step.body.length);
  };
  const del = (i: number) => {
    setBody(step.body.filter((_, j) => j !== i));
    setEditing(null);
  };
  const move = (i: number, d: -1 | 1) => {
    const j = i + d;
    if (j < 0 || j >= step.body.length) return;
    const next = [...step.body];
    [next[i], next[j]] = [next[j], next[i]];
    setBody(next);
  };

  return (
    <>
      <div className="field">
        <label className="label">
          max<span className="hint">最多循环轮数</span>
        </label>
        <input
          className="input"
          type="number"
          value={step.max}
          onChange={(e) => onChange({ ...step, max: Number(e.target.value) || 1 })}
        />
      </div>
      <div className="field">
        <label className="label">until</label>
        <div className="badge badge-loop" style={{ alignSelf: "flex-start" }}>
          codex-clean(固定)
        </div>
      </div>

      <div className="field">
        <label className="label">
          循环体步骤<span className="hint">{step.body.length} 步</span>
        </label>
        <div className="step-list" style={{ margin: 0 }}>
          {step.body.map((s, i) => (
            <StepRow
              key={i}
              step={s}
              seq={i + 1}
              nested
              onEdit={() => setEditing(i)}
              onUp={() => move(i, -1)}
              onDown={() => move(i, 1)}
              onDelete={() => del(i)}
            />
          ))}
        </div>
        <div className="add-step">
          <button className="btn btn-sm" onClick={add}>
            + 加循环内步骤
          </button>
        </div>
      </div>

      {editing !== null && step.body[editing] && (
        <StepDrawer
          step={step.body[editing]}
          depth={depth + 1}
          onChange={(s2) => setSub(editing, s2)}
          onClose={() => setEditing(null)}
        />
      )}
    </>
  );
}
