import { useCallback, useEffect, useState } from "react";
import { ipc } from "../ipc";
import type { EngineEvent, RunSummary } from "../types";
import { runReducer, initialRunState, type RunState } from "./runReducer";

/** 纯函数:把 view_run 的事件序列折叠成 RunState(与 live 共用 runReducer)。 */
export function replayToState(events: EngineEvent[]): RunState {
  return events.reduce(runReducer, initialRunState());
}

export interface HistoryController {
  summaries: RunSummary[];
  refresh: () => void;
  openState: (runId: string) => Promise<RunState>;
}

export function useHistory(): HistoryController {
  const [summaries, setSummaries] = useState<RunSummary[]>([]);
  const refresh = useCallback(() => {
    ipc.listRuns().then(setSummaries).catch(() => setSummaries([]));
  }, []);
  useEffect(() => {
    refresh();
  }, [refresh]);
  const openState = useCallback(async (runId: string): Promise<RunState> => {
    const events = await ipc.viewRun(runId);
    return replayToState(events);
  }, []);
  return { summaries, refresh, openState };
}
