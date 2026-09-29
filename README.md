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
- `src/` — React frontend (sessions sidebar, transcript, composer). Uses Tauri IPC inside the
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
