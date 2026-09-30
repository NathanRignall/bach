# Roadmap

Planned features and known issues, roughly grouped by area.

## Diff view

- [x] **Review comments for the agent**: comment on lines or hunks in the diff and send
  them back to the agent as feedback.
- [x] **Syntax highlighting**: highlight code in the diff by language.
- [x] **File tree**: show changed files as a directory tree, not a flat list.
- [x] **File browser**: browse and view any file in the worktree, not just changed
  ones, with the same tree and highlighting.
- [ ] **Manual edits**: edit code directly in the diff view.
- [ ] **Commit from the diff**: stage files or hunks, write a message and commit
  without leaving the diff view.
- [ ] **Commit history**: browse the branch's commits and open each one's diff.
- [ ] **Push**: push the session's branch from Bach.
- [x] **Explain this change**: select a hunk and ask the agent why it made it.

## Sessions

- [ ] **Notifications**: a native notification (and dock badge) when a turn finishes,
  an approval is waiting or an agent asks a question.
- [ ] **Status in the sidebar**: show whether each session is working, waiting on you,
  done or failed.
- [ ] **Search**: find past sessions by title or transcript text.
- [ ] **Fork or rewind**: branch a session from an earlier message to try another
  approach, with the worktree as it was at that point.
- [ ] **Hand off to another agent**: continue a session's task with a different agent
  (e.g. Claude to Codex) on the same worktree, with a summary of the work so far.
- [x] **Agents can spawn sessions**: let an agent start a new session in the same
  project for a separate task, so side tasks it spots don't derail the current one.
  Expose it as a tool on the existing MCP endpoint rather than a second server: let
  bach-core register extra tools alongside the `task_*` ones.
- [x] **One `bach` MCP server**: rename the agent-facing server from `satie` to `bach`
  (MCP config key, pre-approved tool names, Codex's server check, agent guidance), and
  drop "Satie" from the UI (Tasks panel empty state, `satie · …` tool labels,
  Transcript's `mcp__satie__` prefix). No compatibility needed for old transcripts or
  saved approvals. The task launcher crate (now `bach-tasks`) stays independent of
  the rest of Bach.

## Worktree cleanup

- [ ] **"Merged" label**: mark worktrees whose branch is already merged, so it's clear
  which ones still hold unique commits.

## SSH hosts

- [ ] **Host picker from SSH config**: offer a dropdown of hosts from `~/.ssh/config`
  when adding a host, keeping free text as a fallback.

## Terminal

- [ ] **Theme mismatch**: the terminal sometimes renders bright white instead of
  following the app theme.
