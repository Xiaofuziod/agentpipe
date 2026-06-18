import { useState } from "react";
import type { Step, StepKind, CodexAction, Verify } from "../types";
import { KINDS, defaultsFor, StepRow } from "./StepCard";
import { setVerifyEnabled, patchVerify } from "./verifyEdit";

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
          <VerifySection step={step} onChange={onChange} />
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

type ClaudeStep = Extract<Step, { kind: "claude" }>;

function VerifySection({
  step,
  onChange,
}: {
  step: ClaudeStep;
  onChange: (s: Step) => void;
}) {
  const [open, setOpen] = useState(!!step.verify);
  const v = step.verify;
  const patch = (p: Partial<Verify>) => onChange(patchVerify(step, p));

  return (
    <div className="field" style={{ marginTop: 8 }}>
      <label className="label" style={{ cursor: "pointer", display: "flex", alignItems: "center", gap: 6 }}>
        <input
          type="checkbox"
          checked={!!v}
          onChange={(e) => {
            const next = setVerifyEnabled(step, e.target.checked);
            onChange(next);
            setOpen(e.target.checked);
          }}
        />
        校验门(verify)
        <button
          className="btn-icon"
          style={{ marginLeft: "auto", fontSize: 11 }}
          onClick={() => setOpen((o) => !o)}
          title={open ? "折叠" : "展开"}
        >
          {open ? "▲" : "▼"}
        </button>
      </label>

      {v && open && (
        <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 8, paddingLeft: 12, borderLeft: "2px solid var(--border)" }}>
          <div className="field">
            <label className="label">by</label>
            <select
              className="select"
              value={v.by}
              onChange={(e) => patch({ by: e.target.value as Verify["by"] })}
            >
              <option value="codex">codex</option>
              <option value="claude">claude</option>
              <option value="command">command</option>
            </select>
          </div>

          {v.by === "codex" && (
            <>
              <div className="field">
                <label className="label">action</label>
                <select
                  className="select"
                  value={v.action ?? "review-mr"}
                  onChange={(e) => patch({ action: e.target.value as CodexAction })}
                >
                  <option value="review-mr">review-mr</option>
                  <option value="review-doc">review-doc</option>
                  <option value="ask">ask</option>
                </select>
              </div>
              {(v.action ?? "review-mr") === "review-mr" && (
                <div className="field">
                  <label className="label">base 分支</label>
                  <input
                    className="input input-mono"
                    placeholder="dev"
                    value={v.base ?? ""}
                    onChange={(e) => patch({ base: e.target.value })}
                  />
                </div>
              )}
              {(v.action ?? "review-mr") === "review-doc" && (
                <div className="field">
                  <label className="label">文档路径</label>
                  <input
                    className="input input-mono"
                    placeholder="docs/spec.md"
                    value={v.path ?? ""}
                    onChange={(e) => patch({ path: e.target.value })}
                  />
                </div>
              )}
              {(v.action ?? "review-mr") === "ask" && (
                <div className="field">
                  <label className="label">prompt</label>
                  <textarea
                    className="textarea"
                    placeholder="判定问题…"
                    value={v.prompt ?? ""}
                    onChange={(e) => patch({ prompt: e.target.value })}
                  />
                </div>
              )}
            </>
          )}

          {v.by === "claude" && (
            <>
              <div className="field">
                <label className="label">prompt</label>
                <textarea
                  className="textarea"
                  placeholder="Claude 判定指令…"
                  value={v.prompt ?? ""}
                  onChange={(e) => patch({ prompt: e.target.value })}
                />
              </div>
              <div className="field">
                <label className="label">
                  skill<span className="hint">可选</span>
                </label>
                <input
                  className="input input-mono"
                  placeholder="code-review"
                  value={v.skill ?? ""}
                  onChange={(e) => patch({ skill: e.target.value || undefined })}
                />
              </div>
            </>
          )}

          {v.by === "command" && (
            <div className="field">
              <label className="label">command</label>
              <input
                className="input input-mono"
                placeholder="cargo test"
                value={v.command ?? ""}
                onChange={(e) => patch({ command: e.target.value || undefined })}
              />
              {!v.command && (
                <span style={{ color: "var(--red, #e53e3e)", fontSize: 12, marginTop: 2 }}>
                  command 不能为空
                </span>
              )}
            </div>
          )}

          <div className="field">
            <label className="label">
              max_retries<span className="hint">默认 2</span>
            </label>
            <input
              className="input"
              type="number"
              min={0}
              value={v.max_retries ?? 2}
              onChange={(e) => patch({ max_retries: Number(e.target.value) || 0 })}
            />
          </div>

          <div className="field">
            <label className="label">on_unmet</label>
            <select
              className="select"
              value={v.on_unmet ?? "gate"}
              onChange={(e) => patch({ on_unmet: e.target.value as Verify["on_unmet"] })}
            >
              <option value="gate">gate</option>
              <option value="fail">fail</option>
              <option value="continue">continue</option>
            </select>
          </div>

          <label className="checkbox">
            <input
              type="checkbox"
              checked={v.feedback ?? true}
              onChange={(e) => patch({ feedback: e.target.checked })}
            />
            feedback(将校验结论反馈给 Claude)
          </label>
        </div>
      )}
    </div>
  );
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
