# Bach

A desktop-style UI (Tauri + React) that wraps Claude Code, Codex and opencode by driving each
CLI in headless JSON-streaming mode and normalizing their output into one event stream.

## Layout

- `crates/bach-protocol/` — the wire protocol: the command table (`commands.rs`: each command's
  name, arguments and result), `ServerEvent`s, `ApiError { code, message }`, and the frames the
  WebSocket carries. `cargo test -p bach-protocol` regenerates `src/api/generated/protocol.ts`
  from it (the test fails once when the file changes; commit the result).
- `crates/satie/` — Satie, the background-task launcher (see below). Knows nothing about Bach:
  callers get an MCP token by granting a `Scope` (a project, and an owner recorded on its tasks).
  `lib.rs` is the launcher, `mcp.rs` the agents' MCP server, `diagnose.rs` / `probe.rs` / `process.rs`
  look at the machine, `store.rs` is its own database.
- `crates/satie-protocol/` — Satie's wire types (tasks, `TaskEvent`s, log chunks, command arguments).
- `crates/bach-core/` — the engine, with no UI or transport.
  - `api.rs` — `Api`: implements every command (`bach_protocol::Handler`) and broadcasts events.
  - `adapters/` — one adapter per agent: builds the CLI args, parses its JSON lines into
    `AgentEvent`s. Claude Code is implemented and tested against real output; Codex is
    best-effort; opencode is a stub.
  - `runs.rs` — spawns/cancels agent processes and emits their events.
  - `sessions.rs` — the backend owns sessions: it records every run event into the session's
    transcript as it happens (whether or not a client is watching), tracks what the session is
    waiting on, and pushes `session` events (`changed` / `entry` / `deleted`) to clients.
  - `store.rs` — SQLite storage (`~/.local/share/bach/bach.db`, or `$BACH_DB`): a row per session
    and its transcript as numbered entries. Sessions live wherever the backend runs: the Tauri
    app's data dir, or the bach-server host. Sessions saved by older versions (the frontend's own
    JSON) are converted on startup, their transcript kept as one `imported` entry.
  - `adapters/claude.rs` also steers Claude Code to Satie (system prompt, a hook refusing Bash
    `run_in_background`, the MCP config with the run's token).
- `crates/bach-server/` — the WebSocket bridge for browser use (see below).
- `src-tauri/` — Tauri shell: one `rpc` command into `Api`, events on the `bach` channel.
- `src/api/` — the typed client: `call("send_message", {...})` is checked against the generated
  `Commands` map; `transport.ts` carries it over Tauri IPC or the WebSocket.
- `src/session.ts` folds a session's transcript entries into the blocks the UI renders; `App.tsx`
  keeps the session list and opened transcripts current from `session` events, refetching after
  a gap or a reconnect.
- `src/` — React frontend styled with Tailwind v4 and shadcn/ui (Base UI primitives, `base-nova`
  style; components live in `src/components/ui`, add more with `pnpm dlx shadcn@latest add <name>`).
  Theme tokens are in `src/index.css` and follow the system light/dark setting.

### Adding a command

1. Add an `...Args` struct and a line to the `commands!` table in `crates/bach-protocol/src/commands.rs`.
2. Implement the new `Handler` method in `crates/bach-core/src/api.rs` (it won't compile until you do).
3. `cargo test -p bach-protocol` to regenerate the TypeScript, then `call("your_command", {...})`.

Commands and enum values are `snake_case`; fields are `camelCase`.

## Develop

```sh
direnv allow        # or: nix develop
pnpm install
cargo test
pnpm tauri dev      # needs a display
```

## Using it from a browser (e.g. Mac -> orion)

Agents run wherever `bach-server` runs. It listens on `127.0.0.1:3421` only and rejects
WebSocket connections from origins other than the Vite dev page, because it can run agents
(and so shell commands) as your user.

```sh
# on orion
cargo run --bin bach-server &
pnpm dev                                   # UI on http://orion:3420

# on the Mac
ssh -L 3421:localhost:3421 orion           # then open http://orion:3420
```

`BACH_PORT` changes the port; `BACH_ALLOWED_ORIGINS` (comma-separated) replaces the allowed
origins. Point the UI elsewhere with `VITE_BACH_WS`.

## Native app on the Mac, agents on orion

Run the tunnel as above, then start the Tauri app on the Mac (`pnpm tauri dev`) and, in the
sidebar, set "Agents run" to "on a remote bach-server" (`ws://localhost:3421`) and Apply. The
choice is saved in the app. Working directories are then paths on orion. The server accepts
the Tauri webview origins (`tauri://localhost`, `http://tauri.localhost`) by default.

## Branches and worktrees

For a git project folder, a new session shows a branch picker and a Worktree switch:

- **Off** (default): the agent runs in the folder itself; picking a different branch switches the
  folder to it first (git refuses if uncommitted changes conflict).
- **On**: a new branch `bach/<prompt words>-<id>` is created from the picked branch in an isolated
  worktree under `<data dir>/worktrees/` (next to the session database), and the agent runs there.
  Your own checkout is untouched. Worktrees are not deleted when a session is.

This happens when the first message is sent; the folder, branch and worktree are then fixed.

## Sub-agents, long-running calls and models

- Claude's sub-agent (`Agent`/`Task`) calls render as a card with the sub-agent's title, type,
  live tool/token counts and duration; its own steps and summary nest inside. Running tool
  calls show a ticking elapsed time; calls cut off by Stop or a restart show as interrupted.
- The transcript follows new output only while you're at the bottom. Scroll up to stay put;
  "Jump to latest" re-pins.
- Claude Code sessions have a model picker (Default/Opus/Sonnet/Haiku, passed as `--model`);
  the model each run actually reports using is shown in the session header.

## Approvals and retry

Claude Code runs with `--permission-prompt-tool stdio`: when it wants to use a tool that isn't
pre-approved (e.g. `nix develop`, `tmux`, file writes outside the safe list) it sends a request,
Bach shows an approval card, and the run waits for your answer.

- **Allow** - this call only. **Allow for this session** - also remembers the agent's suggested rule
  (e.g. `Bash(touch inside-b.txt)`) and passes it with `--allowedTools` on every later message in the
  session. **Always allow** - saves that rule to the project's `.claude/settings.local.json`.
  **Deny** - the agent is told you said no.
- Only the rule itself is ever granted. The agent also offers to widen directory access or switch to
  accept-edits mode; those suggestions are dropped. A request that reaches outside the project
  folder says so on the card.
- Sessions waiting on you show a shield in the sidebar and the tab title changes. A reload, or another
  client, can still answer an open request; stopping the run (or restarting the backend) closes it.
- Hover one of your messages for a **Retry** button (resends it as a new message; the agent still
  remembers the earlier one). Error messages have a Retry too.

## Background tasks (Satie)

Anything an agent starts in the background with its own tools dies when the turn ends, because Bach
runs one agent process per message. **Satie** is Bach's own launcher for things that must keep running:

- Processes start *detached* (own session, output in a log file, exit code in a file) and are recorded
  in the database, so they survive turns, agents and even a `bach-server` restart; on startup Bach
  finds them again, notices ones that ended, and can still stop them.
- Agents reach it as an MCP server (`satie`, loopback only, one bearer token per run): `task_start`
  (optionally waits for a `port`), `task_list`, `task_logs`, `task_stop`. Agents only see their own
  project's tasks. Claude Code runs also get a system-prompt note and a hook that refuses Bash
  `run_in_background` and points at `task_start`.
- The **Background tasks** panel (sidebar) lists them with status, uptime, listening ports (clickable),
  live logs, Stop and Remove, and can start a command by hand. It follows `task` events the backend
  pushes (Satie checks tasks every second while anyone is listening) instead of polling.
- Satie keeps its own database and task files in a `tasks/` folder next to the session database
  (`~/.local/share/bach/tasks/satie.db`). Tasks from older versions, kept in `bach.db`, are moved
  there on startup.

Unix only (`setsid`, `/proc` for ports). Codex and opencode don't get the MCP server yet.
