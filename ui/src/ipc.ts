import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { Manifest, EngineEvent, EngineCommand, RunSummary, DiffRow } from "./types";

export const ipc = {
  startRun: (path: string) => invoke<void>("start_run", { path }),
  startRunInline: (manifest: Manifest) => invoke<void>("start_run_inline", { manifest }),
  sendCommand: (cmd: EngineCommand) => invoke<void>("send_command", { cmd }),
  saveManifest: (manifest: Manifest, path: string) => invoke<void>("save_manifest", { manifest, path }),
  listTemplates: () => invoke<string[]>("list_templates"),
  loadTemplate: (name: string) => invoke<Manifest>("load_template", { name }),
  pickDir: () => open({ directory: true }) as Promise<string | null>,
  onEngineEvent: (cb: (e: EngineEvent) => void): Promise<UnlistenFn> =>
    listen<EngineEvent>("engine://event", (e) => cb(e.payload)),
  listRuns: () => invoke<RunSummary[]>("list_runs"),
  viewRun: (runId: string) => invoke<EngineEvent[]>("view_run", { runId }),
  diffRuns: (a: string, b: string) => invoke<DiffRow[]>("diff_runs", { a, b }),
  onRunStartedId: (cb: (runId: string) => void): Promise<UnlistenFn> =>
    listen<{ run_id: string }>("engine://run-started-id", (e) => cb(e.payload.run_id)),
};
