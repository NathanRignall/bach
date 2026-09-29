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
- `crates/bach-server/` — the server binary: `serve` (a private Unix socket for the desktop app,
  plus a WebSocket for browsers; one server per database) and `attach` (stdin/stdout to that
  socket, starting the server if needed).
- `crates/bach-client/` — the desktop app's side of a remote connection: runs
  `ssh <host> bach-server attach`, checks the protocol, matches replies, reconnects; `forward.rs`
  adds port forwards to that SSH connection.
- `src-tauri/` — Tauri shell: one `rpc` command, routed to a backend inside the app or to
  `bach-client`; events on the `bach` channel, connection status on `bach-connection`.
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
pnpm tauri dev      # the desktop app (needs a display)
```

## The Mac app, agents on orion

The desktop app talks to agents either **on this computer** (inside the app) or **on another
machine over SSH**. Pick in the sidebar (the settings button next to "Agents on …"): for SSH, give
the host as you'd pass it to `ssh` (`orion`, a `~/.ssh/config` alias, `user@host`). The choice is
saved in the app.

Over SSH the app runs `ssh <host> bach-server attach` with your own ssh (keys, agent,
`~/.ssh/config`, ProxyJump), never prompting: connect once with `ssh orion` in a terminal first
so its host key is trusted. `attach` connects to the server's private Unix socket on that machine
(`~/.local/share/bach/bach.sock`, next to the database, only accessible to you), and starts the
server there if it isn't running. So nothing listens on the network for the app, and agents and
background tasks keep running when the Mac disconnects. The app reconnects by itself and catches
up. Its status and any SSH error are shown in the sidebar.

**Port forwarding.** Over SSH, the ports background tasks listen on are forwarded to the Mac as
they come and go (like VS Code), so `http://localhost:5173` on the Mac reaches the dev server on
orion; switch it off in the tasks panel's "Forwarded ports", which also forwards any other port by
hand. Clicking a port anywhere opens it in the browser, forwarding it first if needed. Forwards
use the app's own SSH connection (it runs as a `ControlMaster`; each forward is
`ssh -O forward -L 127.0.0.1:<local>:localhost:<port>`), so there's no second login, they listen
on the Mac's loopback only, and they come back after a reconnect. When a port is taken on the Mac
(or below 1024) another one is used, and shown next to the port.

`bach-server` must be on the remote's PATH (or set the command in the connection settings, e.g.
`~/dev/bach/target/release/bach-server`). The flake exports it as a package; on NixOS, add it to
the host, for example:

```nix
# flake inputs:   bach.url = "github:NathanRignall/bach";   (private: nix needs a GitHub token)
environment.systemPackages = [ inputs.bach.packages.${pkgs.system}.bach-server ];
```

The app and the server must come from the same commit: they check each other's protocol
fingerprint (`bach-server --version`) when connecting and say so if they differ.

### Building and signing the Mac app

On the Mac, in the devShell:

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
# notarization: an App Store Connect API key ...
export APPLE_API_ISSUER=... APPLE_API_KEY=... APPLE_API_KEY_PATH=~/keys/AuthKey_XXXX.p8
# ... or an Apple ID with an app-specific password
# export APPLE_ID=... APPLE_PASSWORD=... APPLE_TEAM_ID=...
nix develop -c scripts/build-mac.sh
```

It builds `target/release/bundle/macos/Bach.app` and a DMG, signed with hardened runtime and
notarized (the app by Tauri, the DMG by the script), and checks both the way Gatekeeper will.

## Using it from a browser (e.g. Mac -> orion)

`bach-server` also serves browsers, on a WebSocket at `127.0.0.1:3421` that rejects origins other
than the Vite dev page, because it can run agents (and so shell commands) as your user.

```sh
# on orion
cargo run --bin bach-server &
pnpm dev                                   # UI on http://orion:3420

# on the Mac
ssh -L 3421:localhost:3421 orion           # then open http://orion:3420
```

`BACH_PORT` changes the port; `BACH_ALLOWED_ORIGINS` (comma-separated) replaces the allowed
origins. Point the UI elsewhere with `VITE_BACH_WS`. `BACH_DB` moves the database (and with it
the socket, lock and `server.log`).

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
- The ring next to the model picker shows how full the session's context is; its popover also shows
  the account's plan usage limits (5-hour, weekly). Claude Code reports both while it runs
  (`rate_limit_event`, message usage), so the limits are as of the latest run on that backend.

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
