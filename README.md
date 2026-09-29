# Bach

A desktop-style UI (Tauri + React) that wraps Claude Code, Codex and opencode by driving each
CLI in headless JSON-streaming mode and normalizing their output into one event stream.

## Layout

- `src-tauri/src/adapters/` — one adapter per agent: builds the CLI args, parses its JSON lines
  into `AgentEvent`s. Claude Code is implemented and tested; Codex is best-effort; opencode is a stub.
- `src-tauri/src/runs.rs` — spawns/cancels processes and emits `agent-event` to the UI.
- `src/` — React frontend (sessions sidebar, transcript, composer).

## Develop

```sh
direnv allow        # or: nix develop
pnpm install
pnpm tauri dev      # needs a display; on orion use `pnpm dev` and open http://orion:3420
cd src-tauri && cargo test
```
