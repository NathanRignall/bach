# Architecture

How Bach works under the hood. For what it is and how to get it running, see the
[README](../README.md).

- [Overview](#overview)
- [Code layout](#code-layout)
- [The protocol](#the-protocol)
- [Remote machines over SSH](#remote-machines-over-ssh)
- [From a browser](#from-a-browser)
- [Sessions, branches and worktrees](#sessions-branches-and-worktrees)
- [The transcript](#the-transcript)
- [Approvals](#approvals)
- [Terminals](#terminals)
- [Background tasks (Satie)](#background-tasks-satie)

## Overview

Bach drives each agent CLI (Claude Code, Codex, opencode) in its headless JSON-streaming mode and
normalizes the output into one event stream. The backend owns everything: sessions, transcripts,
terminals and background tasks. The UI is a client that can come and go.

```
 Mac app (Tauri + React)                      remote machine
┌──────────────────────┐   ssh <host>       ┌────────────────────────────────────┐
│ UI ── rpc ── bach-   │   bach-server      │ bach-server                        │
│              client ─┼── attach ─────────►│  ├─ bach-core   sessions, runs,    │
│                      │   (stdin/stdout)   │  │              terminals, SQLite │
│  port forwards ◄─────┼── ControlMaster ───┤  ├─ adapters ─► claude / codex /   │
└──────────────────────┘                    │  │              opencode processes │
                                            │  └─ satie ────► detached tasks     │
 Browser (Vite dev page)                    │                 (MCP for agents)   │
┌──────────────────────┐   WebSocket        │                                    │
│ UI                   │── 127.0.0.1:3421 ─►│                                    │
└──────────────────────┘                    └────────────────────────────────────┘
```

When agents run on the Mac itself, the same `bach-core` backend runs inside the app instead.

## Code layout

| Path | What's there |
| --- | --- |
| `crates/bach-protocol/` | The wire protocol: the command table (`commands.rs`: each command's name, arguments and result), `ServerEvent`s, `ApiError { code, message }`, and the frames the WebSocket carries. |
| `crates/bach-core/` | The engine, with no UI or transport. `api.rs` implements every command (`bach_protocol::Handler`) and broadcasts events; `adapters/` turns each CLI's output into `AgentEvent`s; `runs.rs` spawns and cancels agent processes; `sessions.rs` records every run event into the session's transcript; `store.rs` is SQLite storage; `terminals.rs` the PTYs. |
| `crates/bach-server/` | The server binary: `serve` (a private Unix socket for the app, a WebSocket for browsers; one server per database), `attach` (stdin/stdout to that socket, starting the server if needed) and `restart` (replace a running server with this binary's). |
| `crates/bach-client/` | The app's side of a remote connection: runs `ssh <host> bach-server attach`, checks the protocol, matches replies, reconnects; `forward.rs` adds port forwards. |
| `crates/satie/` | Satie, the background-task launcher. Knows nothing about Bach: callers get an MCP token by granting a `Scope` (a project, and an owner recorded on its tasks). `lib.rs` is the launcher, `mcp.rs` the agents' MCP server, `store.rs` its own database. |
| `crates/satie-protocol/` | Satie's wire types (tasks, `TaskEvent`s, log chunks, command arguments). |
| `src-tauri/` | Tauri shell: one `rpc` command, routed to a backend inside the app or to `bach-client`; events on the `bach` channel, connection status on `bach-connection`. |
| `src/` | React frontend (Tailwind v4, shadcn/ui on Base UI, `base-nova` style). `src/api/` is the typed client and `transport.ts` carries it over Tauri IPC or the WebSocket; `session.ts` folds transcript entries into rendered blocks; theme tokens are in `index.css`. |

Adapter status: Claude Code is implemented and tested against real output, Codex runs over
`codex app-server`, and opencode is still a stub.

## The protocol

Every UI action is a command in `crates/bach-protocol/src/commands.rs`. `cargo test -p
bach-protocol` regenerates `src/api/generated/protocol.ts` from it (the test fails once when the
file changes; commit the result), so `call("send_message", {...})` in the frontend is type-checked
against the Rust definitions. Commands and enum values are `snake_case`; fields are `camelCase`.

To add a command:

1. Add an `...Args` struct and a line to the `commands!` table in `commands.rs`.
2. Implement the new `Handler` method in `crates/bach-core/src/api.rs` (it won't compile until you do).
3. Run `cargo test -p bach-protocol` to regenerate the TypeScript, then `call("your_command", {...})`.

The backend pushes `session` events (`changed` / `entry` / `deleted`), `task` events and
`terminal` events. `App.tsx` keeps the session list and open transcripts current from them,
refetching after a gap or a reconnect.

Sessions live in SQLite (`~/.local/share/bach/bach.db`, or `$BACH_DB`): a row per session and its
transcript as numbered entries, recorded as events happen whether or not a client is watching.

## Remote machines over SSH

The app runs `ssh <host> bach-server attach` with your own ssh (keys, agent, `~/.ssh/config`,
ProxyJump) and never prompts, so connect once with `ssh <host>` in a terminal first to trust the
host key. `attach` talks to the server's private Unix socket (`~/.local/share/bach/bach.sock`,
next to the database, only accessible to you) and starts the server if it isn't running. Nothing
listens on the network, and agents and tasks keep running while the Mac is away. The app
reconnects and catches up on its own. `bach-server` must be on the remote's `PATH`, or the full
command set in the connection settings (e.g. `~/dev/bach/target/release/bach-server`).

The app and the server must come from the same commit. They compare protocol fingerprints
(`bach-server --version`) on connect; if they differ, **Restart server** replaces the remote one
over SSH.

**Port forwarding.** Ports that background tasks listen on are forwarded to the Mac as they come
and go, so `http://localhost:5173` on the Mac reaches the dev server on the remote. Forwards reuse
the app's SSH connection (it runs as a `ControlMaster`; each forward is
`ssh -O forward -L 127.0.0.1:<local>:localhost:<port>`): no second login, loopback only on the
Mac, and restored after a reconnect. If a port is taken on the Mac (or is below 1024), another is
used and shown next to it.

## From a browser

`bach-server` also serves browsers, on a WebSocket at `127.0.0.1:3421` that rejects origins other
than the Vite dev page, because it can run agents (and so shell commands) as your user.

```sh
# on the remote
cargo run --bin bach-server &
pnpm dev                                   # UI on http://<remote>:3420

# on your computer
ssh -L 3421:localhost:3421 <remote>        # then open http://<remote>:3420
```

| Variable | Effect |
| --- | --- |
| `BACH_PORT` | WebSocket port (default `3421`). |
| `BACH_ALLOWED_ORIGINS` | Comma-separated browser origins, replacing the defaults. |
| `VITE_BACH_WS` | Where the UI connects (default `ws://localhost:3421`). |
| `BACH_AGENT_WRAPPER` | A command to start the agent CLIs through (`sandbox` runs `sandbox claude …`), fixed when the server starts; `restart` keeps it. Needs bach-server built with `--features agent-wrapper`, which otherwise refuses to start. |
| `BACH_AGENT_WRAPPER_CODEX_SANDBOX` | `1` keeps Codex's own sandbox inside the wrapper's. Otherwise Codex runs anything the wrapper allows, asking only before commands it thinks are dangerous (Manual asks before any change), and the app's mode picker says so. |
| `VITE_BACH_AGENT_WRAPPER` | Set when building the UI to show "Start agents through" in the SSH settings, passed to the server when the app starts or restarts it. |
| `BACH_DB` | Session database (default `~/.local/share/bach/bach.db`); the socket, lock and `server.log` move with it. |

## Sessions, branches and worktrees

For a git project, a new session shows a branch picker and a **Worktree** switch:

- **Off** (default): the agent runs in the folder itself. Picking another branch switches the
  folder to it first (git refuses if uncommitted changes conflict).
- **On**: a new branch `bach/<prompt words>-<id>` is created from the picked branch in its own
  worktree under `<data dir>/worktrees/`, and the agent runs there. Worktrees aren't deleted with
  their session; **Clean up worktrees…** in the sidebar removes them.

The folder, branch and worktree are fixed once the first message is sent. The **Changes** tab
diffs where the session runs, untracked files included; for a worktree session it defaults to
everything since the base branch.

## The transcript

Bach runs one agent process per message, resuming the CLI's own session each time. Each run's
events are appended to the transcript, which the frontend folds into blocks:

- Claude's sub-agent (`Agent`/`Task`) calls render as a card with live tool and token counts, with
  the sub-agent's own steps nested inside.
- Running tool calls show a ticking elapsed time; calls cut off by Stop or a restart show as
  interrupted.
- The model each run reports is shown in the session header. Context usage and the account's
  5-hour and weekly limits come from Claude Code's `rate_limit_event` and message usage, so they
  are as of the latest run on that backend.

## Approvals

Claude Code runs with `--permission-prompt-tool stdio`. When it wants a tool that isn't
pre-approved, Bach shows an approval card and the run waits for you:

| Button | What it does |
| --- | --- |
| **Allow** | This call only. |
| **Allow for this session** | Also remembers the agent's suggested rule (e.g. `Bash(touch inside-b.txt)`) and passes it with `--allowedTools` on later messages. |
| **Always allow** | Saves the rule to the project's `.claude/settings.local.json`. |
| **Deny** | The agent is told you said no. |

Only the rule itself is ever granted: suggestions to widen directory access or switch to
accept-edits mode are dropped, and a request reaching outside the project says so on the card. A
reload or another client can still answer; stopping the run (or restarting the backend) closes the
request. Codex runs over `codex app-server`, and its approval and question requests use the same
cards.

## Terminals

Terminals belong to the backend (`crates/bach-core/src/terminals.rs`, a PTY each), not the window.
Each is your login shell on the agent's machine, so direnv and dev shells load as usual. Hiding the
panel, reloading or reconnecting leaves them running. The backend keeps each one's last 512 KB of
output to redraw from and streams the rest as numbered `terminal` events; a client that missed
some redraws from that snapshot.

## Background tasks (Satie)

Because each message is its own agent process, anything an agent backgrounds with its own tools
dies when the turn ends. **Satie** is Bach's launcher for things that must keep running:

- Tasks start *detached* (own session, output to a log file, exit code to a file) and are recorded
  in a database, so they survive turns, agents and a `bach-server` restart. On startup Satie finds
  them again and notices ones that ended. Starting the same command in the same folder again
  replaces its earlier finished runs.
- Agents reach it as an MCP server (`satie`, loopback only, one bearer token per run):
  `task_start` (optionally waiting for ports or a URL), `task_list`, `task_logs`, `task_stop`, plus
  process-compose projects. Agents only see their own project's tasks. Claude Code also gets a
  system-prompt note and a hook that refuses Bash `run_in_background`, pointing it at
  `task_start`.
- Tasks only the agent needs (a server for its own tests) can be marked non-interactive: their
  ports aren't forwarded and the panel hides them behind a toggle.
- The **Background tasks** panel follows `task` events the backend pushes (Satie checks tasks every
  second while anyone is listening) instead of polling.

Satie keeps its database and task files in `tasks/` next to the session database
(`~/.local/share/bach/tasks/satie.db`). Unix only (`setsid`, `/proc` for ports).
