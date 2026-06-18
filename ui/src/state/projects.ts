import type { RunSummary } from "../types";

/** 一个项目 = 一个 target(工作目录)。其下挂该 target 产生的全部 run("多轮回合")。 */
export interface Project {
  /** target 绝对路径;"" 表示未指定项目分组 */
  target: string;
  /** 显示名:target 的 basename;"" → UNGROUPED_NAME */
  name: string;
  /** 该项目的 run,保持入参顺序(summaries 已按 run_id 倒序) */
  runs: RunSummary[];
  /** 累计成本 */
  totalCost: number;
  /** 该项目最新 run 的 run_id(项目间排序键) */
  latestRunId: string;
}

export const UNGROUPED_NAME = "(未指定项目)";

/** target 绝对路径 → 显示名(basename);空 target → UNGROUPED_NAME。 */
export function projectName(target: string): string {
  if (!target) return UNGROUPED_NAME;
  const base = target.split(/[\\/]/).filter(Boolean).pop();
  return base || target;
}

/**
 * 按 target 把 run 归类成项目。组内保持入参顺序(summaries 已最新在前);
 * 项目按"最新 run"倒序,未指定项目("")永远沉底。纯函数,单一来源。
 */
export function groupByProject(summaries: RunSummary[]): Project[] {
  const map = new Map<string, RunSummary[]>();
  for (const s of summaries) {
    const key = s.target ?? "";
    const arr = map.get(key);
    if (arr) arr.push(s);
    else map.set(key, [s]);
  }
  const projects: Project[] = [];
  for (const [target, runs] of map) {
    const latestRunId = runs.reduce((m, r) => (r.run_id > m ? r.run_id : m), "");
    const totalCost = runs.reduce((sum, r) => sum + r.total_cost_usd, 0);
    projects.push({ target, name: projectName(target), runs, totalCost, latestRunId });
  }
  projects.sort((a, b) => {
    // 未指定项目沉底
    if (a.target === "" && b.target !== "") return 1;
    if (b.target === "" && a.target !== "") return -1;
    // run_id 含 UTC 紧凑时间戳前缀,字典序倒序 = 最新在前
    if (a.latestRunId < b.latestRunId) return 1;
    if (a.latestRunId > b.latestRunId) return -1;
    return 0;
  });
  return projects;
}

/** 最近用过的项目 target(供快跑栏默认选中);无非空项目时返回 null。 */
export function mostRecentTarget(projects: Project[]): string | null {
  const p = projects.find((x) => x.target !== "");
  return p ? p.target : null;
}
