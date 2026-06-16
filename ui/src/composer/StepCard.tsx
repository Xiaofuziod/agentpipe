import type { Step, StepKind, CodexAction } from "../types";

const KINDS: StepKind["kind"][] = ["claude", "codex", "human", "loop"];

function defaultsFor(kind: StepKind["kind"]): StepKind {
  switch (kind) {
    case "claude":
      return { kind: "claude", prompt: "" };
    case "codex":
      return { kind: "codex", action: "review-mr", base: "dev" };
    case "human":
      return { kind: "human", instruction: "" };
    case "loop":
      return { kind: "loop", until: "codex-clean", max: 5, body: [] };
  }
}

function Fields({ step, onChange }: { step: Step; onChange: (s: Step) => void }) {
  switch (step.kind) {
    case "claude":
      return (
        <>
          <textarea
            placeholder="prompt"
            value={step.prompt}
            onChange={(e) => onChange({ ...step, prompt: e.target.value })}
          />
          <input
            placeholder="skill(可选)"
            value={step.skill ?? ""}
            onChange={(e) => onChange({ ...step, skill: e.target.value || undefined })}
          />
          <label>
            <input
              type="checkbox"
              checked={!!step.allow_writes}
              onChange={(e) => onChange({ ...step, allow_writes: e.target.checked })}
            />
            allow_writes(自主写码,bypassPermissions)
          </label>
          <input
            type="number"
            placeholder="timeout 秒(可选)"
            value={step.timeout ?? ""}
            onChange={(e) => onChange({ ...step, timeout: e.target.value ? Number(e.target.value) : undefined })}
          />
        </>
      );
    case "codex":
      return (
        <>
          <select
            value={step.action}
            onChange={(e) => onChange({ ...step, action: e.target.value as CodexAction })}
          >
            <option value="review-mr">review-mr</option>
            <option value="review-doc">review-doc</option>
            <option value="ask">ask</option>
          </select>
          {step.action === "review-mr" && (
            <input
              placeholder="base 分支"
              value={step.base ?? ""}
              onChange={(e) => onChange({ ...step, base: e.target.value })}
            />
          )}
          {step.action === "review-doc" && (
            <input
              placeholder="文档路径"
              value={step.path ?? ""}
              onChange={(e) => onChange({ ...step, path: e.target.value })}
            />
          )}
          {step.action === "ask" && (
            <input
              placeholder="问题"
              value={step.prompt ?? ""}
              onChange={(e) => onChange({ ...step, prompt: e.target.value })}
            />
          )}
        </>
      );
    case "human":
      return (
        <>
          <input
            placeholder="instruction"
            value={step.instruction}
            onChange={(e) => onChange({ ...step, instruction: e.target.value })}
          />
          <label>
            <input
              type="checkbox"
              checked={step.expects !== undefined}
              onChange={(e) => onChange({ ...step, expects: e.target.checked ? "artifact" : undefined })}
            />
            需要产物
          </label>
        </>
      );
    case "loop":
      return <LoopBody step={step} onChange={onChange} />;
  }
}

function LoopBody({
  step,
  onChange,
}: {
  step: Extract<Step, { kind: "loop" }>;
  onChange: (s: Step) => void;
}) {
  const setBody = (body: Step[]) => onChange({ ...step, body });
  const setSub = (i: number, s: Step) => setBody(step.body.map((x, j) => (j === i ? s : x)));
  const add = () =>
    setBody([...step.body, { id: `${step.id}-body-${step.body.length + 1}`, kind: "codex", action: "review-mr", base: "dev" }]);
  const del = (i: number) => setBody(step.body.filter((_, j) => j !== i));
  const move = (i: number, d: -1 | 1) => {
    const j = i + d;
    if (j < 0 || j >= step.body.length) return;
    const next = [...step.body];
    [next[i], next[j]] = [next[j], next[i]];
    setBody(next);
  };
  return (
    <div style={{ borderLeft: "2px solid #ccc", paddingLeft: 8, marginTop: 6 }}>
      <input
        type="number"
        placeholder="max 轮数"
        value={step.max}
        onChange={(e) => onChange({ ...step, max: Number(e.target.value) || 1 })}
      />
      <span> until: codex-clean(固定)</span>
      {step.body.map((s, i) => (
        <StepCard
          key={i}
          step={s}
          onChange={(s2) => setSub(i, s2)}
          onUp={() => move(i, -1)}
          onDown={() => move(i, 1)}
          onDelete={() => del(i)}
        />
      ))}
      <button onClick={add}>+ 加循环内步骤</button>
    </div>
  );
}

export function StepCard({
  step,
  onChange,
  onUp,
  onDown,
  onDelete,
}: {
  step: Step;
  onChange: (s: Step) => void;
  onUp: () => void;
  onDown: () => void;
  onDelete: () => void;
}) {
  const setKind = (kind: StepKind["kind"]) => onChange({ id: step.id, ...defaultsFor(kind) });
  return (
    <div style={{ border: "1px solid #ddd", borderRadius: 6, padding: 8, margin: "6px 0" }}>
      <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
        <input
          placeholder="id"
          value={step.id}
          onChange={(e) => onChange({ ...step, id: e.target.value })}
          style={{ width: 140 }}
        />
        <select value={step.kind} onChange={(e) => setKind(e.target.value as StepKind["kind"])}>
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {k}
            </option>
          ))}
        </select>
        <button onClick={onUp}>↑</button>
        <button onClick={onDown}>↓</button>
        <button onClick={onDelete}>删</button>
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 6 }}>
        <Fields step={step} onChange={onChange} />
      </div>
    </div>
  );
}
