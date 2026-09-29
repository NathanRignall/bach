# Bach

A desktop-style UI (Tauri + React) that wraps Claude Code, Codex and opencode by driving each
CLI in headless JSON-streaming mode and normalizing their output into one event stream.

## Layout

- `bach-core/` — UI-free Rust crate.
  - `adapters/` — one adapter per agent: builds the CLI args, parses its JSON lines into
    `AgentEvent`s. Claude Code is implemented and tested against real output; Codex is
    best-effort; opencode is a stub.
  - `runs.rs` — spawns/cancels agent processes and emits events.
  - `store.rs` — SQLite session storage (`~/.local/share/bach/bach.db`, or `$BACH_DB`). Sessions
    are saved by whichever backend is in use: the Tauri app's data dir, or the bach-server host.
  - `server.rs`, `bin/bach-server.rs` — WebSocket bridge for browser use (see below).
- `src-tauri/` — Tauri shell exposing the same commands to the window.
- `src/` — React frontend styled with Tailwind v4 and shadcn/ui (Base UI primitives, `base-nova`
  style; components live in `src/components/ui`, add more with `pnpm dlx shadcn@latest add <name>`).
  Theme tokens are in `src/index.css` and follow the system light/dark setting. Uses Tauri IPC inside the
  app and the WebSocket bridge in a plain browser (`src/api.ts`).

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
