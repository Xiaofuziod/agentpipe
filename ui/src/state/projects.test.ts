import { describe, expect, it } from "vitest";
import { groupByProject, mostRecentTarget, projectName, UNGROUPED_NAME } from "./projects";
import type { RunSummary } from "../types";

function sum(run_id: string, target: string, cost = 0): RunSummary {
  return {
    run_id,
    name: run_id,
    target,
    status: "Success",
    total_cost_usd: cost,
    total_turns: 1,
    step_count: 1,
    complete: true,
  };
}

describe("projectName", () => {
  it("取 target 的 basename", () => {
    expect(projectName("/Users/x/code/repo")).toBe("repo");
    expect(projectName("/Users/x/code/repo/")).toBe("repo"); // 尾斜杠
    expect(projectName("C:\\work\\proj")).toBe("proj"); // windows 分隔符
  });
  it("空 target → 未指定项目", () => {
    expect(projectName("")).toBe(UNGROUPED_NAME);
  });
});

describe("groupByProject", () => {
  it("按 target 归类,组内保持入参顺序", () => {
    // 入参按 run_id 倒序(最新在前)
    const projects = groupByProject([
      sum("20260618T120000Z-c", "/a", 0.3),
      sum("20260618T110000Z-b", "/a", 0.2),
      sum("20260618T100000Z-a", "/b", 0.5),
    ]);
    expect(projects.map((p) => p.target)).toEqual(["/a", "/b"]);
    const a = projects[0];
    expect(a.runs.map((r) => r.run_id)).toEqual([
      "20260618T120000Z-c",
      "20260618T110000Z-b",
    ]);
    expect(a.totalCost).toBeCloseTo(0.5);
    expect(a.latestRunId).toBe("20260618T120000Z-c");
  });

  it("项目按最新 run 倒序", () => {
    const projects = groupByProject([
      sum("20260618T090000Z-old", "/old"),
      sum("20260618T200000Z-new", "/new"),
    ]);
    expect(projects.map((p) => p.target)).toEqual(["/new", "/old"]);
  });

  it("空 target 归未指定项目并沉底", () => {
    const projects = groupByProject([
      sum("20260618T200000Z-x", ""),
      sum("20260618T100000Z-y", "/repo"),
    ]);
    expect(projects.map((p) => p.target)).toEqual(["/repo", ""]);
    expect(projects[1].name).toBe(UNGROUPED_NAME);
  });
});

describe("mostRecentTarget", () => {
  it("返回最近的非空 target", () => {
    const projects = groupByProject([
      sum("20260618T200000Z-x", "/new"),
      sum("20260618T100000Z-y", "/old"),
    ]);
    expect(mostRecentTarget(projects)).toBe("/new");
  });
  it("只有未指定项目时返回 null", () => {
    expect(mostRecentTarget(groupByProject([sum("a", "")]))).toBeNull();
    expect(mostRecentTarget([])).toBeNull();
  });
});
