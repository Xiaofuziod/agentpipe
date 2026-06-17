import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { ipc } from "../ipc";
import type { EngineEvent, Manifest } from "../types";
import { runReducer, initialRunState, type RunState } from "./runReducer";

export interface RunRecord {
  id: string;
  name: string;
  path: string;
  startedAt: number;
  state: RunState;
}

function deriveName(path: string): string {
  const base = path.split(/[\\/]/).pop() ?? path;
  return base.replace(/\.ya?ml$/i, "") || base;
}

/**
 * 把一条引擎事件路由到活跃记录:只更新 activeId 对应的那条 state,其余按引用复用;
 * RunFinished 后把 activeId 置空(该 run 不再接收事件)。无活跃 run 时原样返回。
 * 纯函数,单一来源——hook 与测试共用,避免路由语义漂移。
 */
export function routeEvent(
  records: RunRecord[],
  activeId: string | null,
  e: EngineEvent,
): { records: RunRecord[]; activeId: string | null } {
  if (!activeId) return { records, activeId };
  const next = records.map((r) => (r.id === activeId ? { ...r, state: runReducer(r.state, e) } : r));
  return { records: next, activeId: e.type === "RunFinished" ? null : activeId };
}

export interface RunsController {
  records: RunRecord[];
  selectedId: string | null;
  activeId: string | null;
  selected: RunRecord | null;
  select: (id: string) => void;
  start: (path: string) => Promise<void>;
  startInline: (manifest: Manifest) => Promise<void>;
}

/**
 * 多 run 记录控制器:引擎一次只跑一个 run(串行),所以全局 engine://event 事件流
 * 全部归属当前活跃记录;run 结束后该记录的最终快照留在 records 里供回看。
 * 监听只在 mount 时建立一次,start 前 await readyRef 保证不丢早期事件(RunStarted/首步)。
 */
export function useRuns(): RunsController {
  const [records, setRecords] = useState<RunRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);

  const activeIdRef = useRef<string | null>(null);
  const counter = useRef(0);
  const readyRef = useRef<Promise<void> | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;
    let resolveReady!: () => void;
    readyRef.current = new Promise<void>((r) => (resolveReady = r));

    ipc
      .onEngineEvent((e) => {
        const id = activeIdRef.current;
        if (!id) return;
        setRecords((rs) => routeEvent(rs, id, e).records);
        if (e.type === "RunFinished") {
          activeIdRef.current = null;
          setActiveId(null);
        }
      })
      .then((un) => {
        if (cancelled) {
          un();
          return;
        }
        unlisten = un;
        resolveReady();
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const select = useCallback((id: string) => setSelectedId(id), []);

  // 新建并登记一条记录,设为活跃 + 选中;返回前等监听就绪(不丢早期事件)。
  const beginRecord = useCallback(async (name: string, path: string): Promise<void> => {
    await readyRef.current;
    const id = `run-${++counter.current}-${Date.now()}`;
    const rec: RunRecord = { id, name, path, startedAt: Date.now(), state: initialRunState() };
    setRecords((rs) => [rec, ...rs]); // 最新在最上
    activeIdRef.current = id;
    setActiveId(id);
    setSelectedId(id);
  }, []);

  const start = useCallback(
    async (path: string) => {
      await beginRecord(deriveName(path), path);
      ipc.startRun(path).catch((e) => alert(String(e)));
    },
    [beginRecord],
  );

  const startInline = useCallback(
    async (manifest: Manifest) => {
      await beginRecord(manifest.name || "快速运行", "");
      ipc.startRunInline(manifest).catch((e) => alert(String(e)));
    },
    [beginRecord],
  );

  const selected = records.find((r) => r.id === selectedId) ?? null;

  return { records, selectedId, activeId, selected, select, start, startInline };
}
