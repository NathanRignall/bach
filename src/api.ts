import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type AgentKind = "claude" | "codex" | "opencode";

export interface AgentInfo {
  kind: AgentKind;
  name: string;
  installed: boolean;
}

export type AgentEvent =
  | { type: "session"; id: string }
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; id: string; output: string; is_error: boolean }
  | { type: "done"; cost_usd: number | null; is_error: boolean }
  | { type: "error"; message: string }
  | { type: "raw"; line: string };

export type RunEvent = AgentEvent & { run_id: string };

const inTauri = "__TAURI_INTERNALS__" in window;

export const listAgents = (): Promise<AgentInfo[]> =>
  inTauri
    ? invoke("list_agents")
    : Promise.resolve([
        { kind: "claude", name: "Claude Code", installed: true },
        { kind: "codex", name: "Codex", installed: false },
        { kind: "opencode", name: "opencode", installed: false },
      ]);

export const startRun = (args: {
  agent: AgentKind;
  prompt: string;
  cwd?: string;
  sessionId?: string;
}): Promise<string> => invoke("start_run", args);

export const cancelRun = (runId: string): Promise<void> => invoke("cancel_run", { runId });

export const onAgentEvent = (cb: (e: RunEvent) => void) =>
  inTauri ? listen<RunEvent>("agent-event", (e) => cb(e.payload)) : Promise.resolve(() => {});
