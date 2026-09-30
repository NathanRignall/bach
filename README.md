<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="128" height="128" alt="Bach app icon">
</p>

<h1 align="center">Bach</h1>

<p align="center">
  <strong>One desktop app for your coding agents, running on the machine where the code lives.</strong><br>
  Claude Code, Codex and opencode, driven headless and rendered as one clean chat.
</p>

<p align="center">
  <img src="docs/screenshots/chat.png" alt="A Claude Code session in Bach: the transcript with tool calls, the session list and the composer">
</p>

Bach is a Tauri + React app that drives each agent CLI in its headless JSON-streaming mode and
turns its output into one event stream. The agents run in `bach-server`, on your own computer or
on a remote machine over SSH, so sessions, terminals and dev servers keep running when your laptop
sleeps or disconnects.

## Features

- **Every agent in one place.** Sessions from Claude Code, Codex and opencode sit side by side,
  grouped by project. Tool calls, sub-agents, questions and approvals each get a proper card.
- **Agents on another machine.** Point the Mac app at a host over SSH (your keys, your
  `~/.ssh/config`). Nothing listens on the network, and ports your dev servers open are
  forwarded back to the Mac automatically.
- **Approvals you control.** Allow once, for the session, or always (saved to the project).
  Sessions waiting on you are flagged in the sidebar.
- **Worktrees per session.** Start a session on a fresh branch in its own git worktree, and
  review everything it changed in the **Changes** tab.
- **Real terminals.** A terminal panel per project, running on the agent's machine, that
  survives reloads and reconnects.
- **Background tasks that outlive the turn.** Satie, Bach's task launcher, lets agents start dev
  servers and long jobs that keep running, with live logs and ports.
- **Context and usage at a glance.** A ring shows how full the context is, plus your plan's
  5-hour and weekly limits.

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/changes.png" alt="The Changes tab showing a session's diff"></td>
    <td width="50%"><img src="docs/screenshots/terminal.png" alt="The terminal panel open under a chat"></td>
  </tr>
  <tr>
    <td align="center"><sub>Review a session's changes</sub></td>
    <td align="center"><sub>A terminal on the agent's machine</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/screenshots/new-session.png" alt="The new-session screen with folder, branch and worktree pickers"></td>
    <td width="50%"><img src="docs/screenshots/chat-dark.png" alt="A session in dark mode"></td>
  </tr>
  <tr>
    <td align="center"><sub>Start a session: folder, branch, worktree, agent, model</sub></td>
    <td align="center"><sub>Follows the system light/dark setting</sub></td>
  </tr>
</table>

## Getting started

Everything comes from the Nix flake's devShell.

```sh
direnv allow        # or: nix develop
pnpm install
cargo test
pnpm tauri dev      # the desktop app (needs a display)
```

You'll need at least one agent CLI installed and signed in (`claude`, `codex` or `opencode`).

---

## Using Bach

### Agents on a remote machine

The app runs agents either **on this computer** (inside the app) or **on another machine over
SSH**. Choose in the sidebar with the settings button next to "Agents on …". For SSH, give the
host as you'd pass it to `ssh` (`orion`, a `~/.ssh/config` alias, `user@host`).

- The app runs `ssh <host> bach-server attach` with your own ssh (keys, agent, `~/.ssh/config`,
  ProxyJump) and never prompts, so connect once with `ssh <host>` in a terminal first to trust its
  host key.
- `attach` talks to the server's private Unix socket (`~/.local/share/bach/bach.sock`, only
  accessible to you) and starts the server if it isn't running. Nothing listens on the network,
  and agents and tasks keep running while the Mac is away. The app reconnects and catches up on
  its own; its status and any SSH error show in the sidebar.
- `bach-server` must be on the remote's `PATH`, or set the full command in the connection
  settings (e.g. `~/dev/bach/target/release/bach-server`). The flake exports it as a package:

  ```nix
  # flake inputs:   bach.url = "github:NathanRignall/bach";   (private: nix needs a GitHub token)
  environment.systemPackages = [ inputs.bach.packages.${pkgs.system}.bach-server ];
  ```

- The app and the server must come from the same commit. They compare protocol fingerprints
  (`bach-server --version`) on connect; if they differ, **Restart server** replaces the remote
  one over SSH.

**Port forwarding.** Ports that background tasks listen on are forwarded to the Mac as they come
and go (like VS Code), so `http://localhost:5173` on the Mac reaches the dev server on the remote.
The cable icon next to the connection lists the forwards with the task listening on each, turns
automatic forwarding off, and forwards any other port by hand. Clicking a port anywhere in the app
opens it, forwarding it first if needed.

Forwards reuse the app's SSH connection (a `ControlMaster`, each forward an
`ssh -O forward -L 127.0.0.1:<local>:localhost:<port>`): no second login, loopback only on the Mac,
and restored after a reconnect. If a port is taken on the Mac (or is below 1024), another is used
and shown next to it.

### Sessions, branches and worktrees

For a git project, a new session shows a branch picker and a **Worktree** switch:

- **Off** (default): the agent runs in the folder itself. Picking another branch switches the
  folder to it first (git refuses if uncommitted changes conflict).
- **On**: a new branch `bach/<prompt words>-<id>` is created from the picked branch in its own
  worktree under `<data dir>/worktrees/`, and the agent runs there. Your checkout is untouched.
  Worktrees aren't deleted with their session; **Clean up worktrees…** in the sidebar removes them.

The folder, branch and worktree are fixed once the first message is sent.

The **Changes** tab (Ctrl/Cmd+Shift+D) shows the diff where the session runs, untracked files
included. For a worktree session it defaults to everything since the base branch; **Uncommitted**
narrows it to what isn't committed yet. It refreshes when a run ends, and every few seconds while
the agent works.

### The transcript

- Claude's sub-agent (`Agent`/`Task`) calls render as a card with the sub-agent's title, type,
  live tool and token counts, and duration, with its own steps nested inside.
- Running tool calls show a ticking elapsed time; calls cut off by Stop or a restart show as
  interrupted.
- The transcript follows new output only while you're at the bottom. Scroll up to stay put;
  **Jump to latest** re-pins.
- Hover one of your messages for **Retry** (resends it as a new message). Errors have one too.
- Claude Code sessions have a model picker (Default/Opus/Sonnet/Haiku, passed as `--model`); the
  model each run reports is shown in the header.
- The ring next to the model picker shows how full the context is. Its popover shows the account's
  5-hour and weekly limits, as of the latest run on that backend.

### Approvals

Claude Code runs with `--permission-prompt-tool stdio`. When it wants a tool that isn't
pre-approved, Bach shows an approval card and the run waits for you:

| Button | What it does |
| --- | --- |
| **Allow** | This call only. |
| **Allow for this session** | Also remembers the agent's suggested rule (e.g. `Bash(touch inside-b.txt)`) and passes it with `--allowedTools` on later messages. |
| **Always allow** | Saves the rule to the project's `.claude/settings.local.json`. |
| **Deny** | The agent is told you said no. |

Only the rule itself is ever granted: suggestions to widen directory access or switch to
accept-edits mode are dropped, and a request reaching outside the project says so on the card.
Sessions waiting on you show a shield in the sidebar and change the tab title. A reload or another
client can still answer; stopping the run (or restarting the backend) closes the request.

Codex runs over `codex app-server`, and its approval and question requests use the same cards.

### Terminal

Drag up the **Terminal** bar under the chat (or click it, or press Ctrl+\`) to open the project's
terminals: your login shell on the agent's machine, so direnv and dev shells load as usual,
starting in the session's folder. Tabs hold several; drag the bar to resize or put it away.

Terminals belong to the backend (a PTY each), not the window. Hiding the panel, reloading or
reconnecting leaves them running, and any client can pick them up from the last 512 KB of output.
Closing a tab ends its shell. The terminal font (JetBrains Mono) is bundled.

### Background tasks (Satie)

Bach runs one agent process per message, so anything an agent backgrounds with its own tools dies
when the turn ends. **Satie** is Bach's launcher for things that must keep running:

- Tasks start *detached* (own session, output to a log file, exit code to a file) and are recorded
  in a database, so they survive turns, agents and a `bach-server` restart. Starting the same
  command in the same folder again replaces its earlier finished runs.
- Agents reach it as an MCP server (`satie`, loopback only, one bearer token per run):
  `task_start` (optionally waiting for ports or a URL), `task_list`, `task_logs`, `task_stop`, plus
  process-compose projects. Agents only see their own project's tasks. Claude Code also gets a
  system-prompt note and a hook that refuses Bash `run_in_background`, pointing it at
  `task_start`.
- Tasks the agent only needs itself (a server for its own tests) can be marked non-interactive:
  their ports aren't forwarded and the panel hides them behind a toggle.
- The **Background tasks** panel lists tasks with status, uptime, clickable ports, live logs, Stop
  and Remove (or **Clear finished**), updated live from `task` events.

Satie keeps its database and task files in `tasks/` next to the session database. Unix only
(`setsid`, `/proc` for ports). Codex and opencode don't get the MCP server yet.

### From a browser

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
| `BACH_DB` | Session database (default `~/.local/share/bach/bach.db`); the socket, lock and `server.log` move with it. |

---

## Development

### Layout

| Path | What's there |
| --- | --- |
| `crates/bach-protocol/` | The wire protocol: the command table (`commands.rs`), `ServerEvent`s, `ApiError`, and WebSocket frames. `cargo test -p bach-protocol` regenerates `src/api/generated/protocol.ts` (the test fails once when it changes; commit the result). |
| `crates/bach-core/` | The engine, with no UI or transport. `api.rs` implements every command; `adapters/` turns each CLI's output into `AgentEvent`s; `runs.rs` spawns and cancels agents; `sessions.rs` records every run event into the session's transcript; `store.rs` is SQLite storage; `terminals.rs` the PTYs. |
| `crates/bach-server/` | The server binary: `serve` (Unix socket for the app, WebSocket for browsers), `attach` (stdin/stdout to that socket, starting the server if needed) and `restart`. |
| `crates/bach-client/` | The app's side of a remote connection: runs `ssh <host> bach-server attach`, checks the protocol, reconnects; `forward.rs` adds port forwards. |
| `crates/satie/` | Satie, the background-task launcher. Knows nothing about Bach: callers get an MCP token by granting a `Scope`. |
| `crates/satie-protocol/` | Satie's wire types (tasks, `TaskEvent`s, log chunks). |
| `src-tauri/` | Tauri shell: one `rpc` command, routed to an in-app backend or to `bach-client`. |
| `src/` | React frontend (Tailwind v4, shadcn/ui on Base UI). `src/api/` is the typed client, `session.ts` folds transcript entries into rendered blocks. Theme tokens are in `src/index.css`. |

Adapter status: Claude Code is implemented and tested against real output, Codex runs over
`codex app-server`, and opencode is still a stub that shows raw JSON.

### Adding a command

1. Add an `...Args` struct and a line to the `commands!` table in `crates/bach-protocol/src/commands.rs`.
2. Implement the new `Handler` method in `crates/bach-core/src/api.rs` (it won't compile until you do).
3. Run `cargo test -p bach-protocol` to regenerate the TypeScript, then `call("your_command", {...})`.

Commands and enum values are `snake_case`; fields are `camelCase`. UI components live in
`src/components/ui`; add more with `pnpm dlx shadcn@latest add <name>`.

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

This builds `target/release/bundle/macos/Bach.app` and a DMG, signed with hardened runtime and
notarized, and checks both the way Gatekeeper will.
