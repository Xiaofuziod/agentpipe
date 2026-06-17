import type { Step, StepKind } from "../types";

export const KINDS: StepKind["kind"][] = ["claude", "codex", "human", "loop"];

export function defaultsFor(kind: StepKind["kind"]): StepKind {
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

/** 一行可读摘要(主列表只显示概要,详情进抽屉编辑)。 */
export function stepSummary(step: Step): string {
  switch (step.kind) {
    case "claude": {
      const tags = [step.skill && `@${step.skill}`].filter(Boolean);
      const head = step.prompt.split("\n")[0].trim();
      return [head || "(空 prompt)", tags.length ? `· ${tags.join(" ")}` : ""].join(" ");
    }
    case "codex":
      if (step.action === "review-mr") return `review-mr · base:${step.base || "dev"}`;
      if (step.action === "review-doc") return `review-doc · ${step.path || "(未指定文档)"}`;
      return `ask · ${step.prompt || "(空问题)"}`;
    case "human":
      return step.instruction || "(空指令)";
    case "loop":
      return `until:codex-clean · 最多 ${step.max} 轮 · ${step.body.length} 步`;
  }
}

const Icon = {
  pencil: (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
    </svg>
  ),
  up: (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 19V5M5 12l7-7 7 7" />
    </svg>
  ),
  down: (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 5v14M19 12l-7 7-7-7" />
    </svg>
  ),
  trash: (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2m2 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
    </svg>
  ),
};

const badgeClass: Record<StepKind["kind"], string> = {
  claude: "badge-claude",
  codex: "badge-codex",
  human: "badge-human",
  loop: "badge-loop",
};

export function StepRow({
  step,
  seq,
  nested,
  onEdit,
  onUp,
  onDown,
  onDelete,
}: {
  step: Step;
  seq: number;
  nested?: boolean;
  onEdit: () => void;
  onUp: () => void;
  onDown: () => void;
  onDelete: () => void;
}) {
  return (
    <div className={`step-row ${nested ? "nested" : ""}`}>
      <span className="seq">{seq}</span>
      <span className={`badge ${badgeClass[step.kind]}`}>{step.kind}</span>
      <span className="step-id">{step.id}</span>
      <span className="step-summary">{stepSummary(step)}</span>
      <div className="actions">
        <button className="btn-icon" title="编辑" onClick={onEdit}>
          {Icon.pencil}
        </button>
        <button className="btn-icon" title="上移" onClick={onUp}>
          {Icon.up}
        </button>
        <button className="btn-icon" title="下移" onClick={onDown}>
          {Icon.down}
        </button>
        <button className="btn-icon btn-danger" title="删除" onClick={onDelete}>
          {Icon.trash}
        </button>
      </div>
    </div>
  );
}
