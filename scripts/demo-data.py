#!/usr/bin/env python3
"""Build a throwaway Bach instance for screenshots, so they never show a personal database.

    python3 scripts/demo-data.py /tmp/bach-demo      # (re)creates the folder

It writes made-up git projects under <dir>/projects and a database with sessions in them at
<dir>/data/bach.db. Then run, from the repo root:

    BACH_DB=<dir>/data/bach.db BACH_PORT=3451 BACH_ALLOWED_ORIGINS=http://localhost:3450 \
        cargo run --bin bach-server
    VITE_BACH_WS=ws://localhost:3451 pnpm exec vite --port 3450
"""
import json, shutil, sqlite3, subprocess, sys, time
from pathlib import Path

root = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/bach-demo").resolve()
shutil.rmtree(root, ignore_errors=True)
(root / "data").mkdir(parents=True)

NOW = int(time.time() * 1000)
MIN = 60_000


def git(cwd, *args):
    env = {"GIT_AUTHOR_NAME": "Demo", "GIT_AUTHOR_EMAIL": "demo@example.com",
           "GIT_COMMITTER_NAME": "Demo", "GIT_COMMITTER_EMAIL": "demo@example.com",
           "PATH": "/usr/bin:/bin:/run/current-system/sw/bin", "HOME": str(root)}
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, env=env)


def project(name, files, changed):
    """A git repo with `files` committed, then `changed` edited and left uncommitted."""
    d = root / "projects" / name
    d.mkdir(parents=True)
    git(d, "init", "-b", "main")
    for path, text in files.items():
        (d / path).parent.mkdir(parents=True, exist_ok=True)
        (d / path).write_text(text)
    git(d, "add", "."); git(d, "commit", "-m", "Initial commit")
    for path, text in changed.items():
        (d / path).parent.mkdir(parents=True, exist_ok=True)
        (d / path).write_text(text)
    return str(d)


LIMITER_OLD = '''import { Router } from "express";

export const uploads = Router();

uploads.post("/uploads", async (req, res) => {
  const file = await saveUpload(req);
  res.status(201).json({ id: file.id, url: file.url });
});
'''
LIMITER_NEW = '''import { Router } from "express";
import { rateLimit } from "../middleware/rate-limit";

export const uploads = Router();

// 20 uploads per minute per API key; anonymous callers share one bucket.
uploads.post("/uploads", rateLimit({ windowMs: 60_000, max: 20 }), async (req, res) => {
  const file = await saveUpload(req);
  res.status(201).json({ id: file.id, url: file.url });
});
'''
MIDDLEWARE = '''export function rateLimit({ windowMs, max }: { windowMs: number; max: number }) {
  const hits = new Map<string, number[]>();
  return (req, res, next) => {
    const key = req.get("x-api-key") ?? "anonymous";
    const now = Date.now();
    const recent = (hits.get(key) ?? []).filter((t) => now - t < windowMs);
    if (recent.length >= max) {
      res.set("retry-after", String(Math.ceil((recent[0] + windowMs - now) / 1000)));
      return res.status(429).json({ error: "rate_limited" });
    }
    hits.set(key, [...recent, now]);
    next();
  };
}
'''

tidepool = project(
    "tidepool",
    {"src/routes/uploads.ts": LIMITER_OLD, "README.md": "# tidepool\n\nUpload API.\n"},
    {"src/routes/uploads.ts": LIMITER_NEW, "src/middleware/rate-limit.ts": MIDDLEWARE},
)
harbor = project("harbor", {"main.go": "package main\n\nfunc main() {}\n"}, {})
driftwood = project("driftwood", {"docs/index.md": "# Driftwood\n"}, {})
lantern = project("lantern", {"lib/lantern.py": "def glow():\n    return 1\n"}, {})


def session(sid, title, cwd, age_min, entries, agent="claude", model="claude-fable-5-1", context=None):
    data = {
        "id": sid, "title": title, "titleEdited": False, "agent": agent, "cwd": cwd,
        "branch": "main", "worktree": False, "modelChoice": None, "permissionMode": "auto",
        "effort": None, "workdir": cwd, "gitBranch": "main", "workdirRemoved": False,
        "agentSessionId": sid, "allowRules": [], "model": model,
        "context": context, "archived": False, "runId": None, "openApprovals": [],
        "queued": [], "createdAt": NOW - (age_min + 30) * MIN, "updatedAt": NOW - age_min * MIN,
        "lastSeq": len(entries),
    }
    return data, entries


def user(text):
    return {"type": "user", "text": text}


def agent(run, **event):
    return {"type": "agent", "runId": run, "event": event}


def text(run, t):
    return agent(run, type="text", text=t)


def tool(run, tid, name, inp, output, error=False):
    return [agent(run, type="tool_use", id=tid, name=name, input=inp),
            agent(run, type="tool_result", id=tid, output=output, isError=error)]


def flat(*parts):
    out = []
    for p in parts:
        out.extend(p if isinstance(p, list) else [p])
    return out


R = "run-1"
rate_limit = flat(
    user("Add rate limiting to the upload endpoint: 20 uploads a minute per API key."),
    agent(R, type="session", id="demo-1", model="claude-fable-5-1"),
    text(R, "I'll start by looking at how the upload route is wired up."),
    tool(R, "t1", "Read", {"file_path": f"{tidepool}/src/routes/uploads.ts"}, LIMITER_OLD),
    tool(R, "t2", "Bash", {"command": "grep -rn \"x-api-key\" src", "description": "Find where API keys are read"},
         "src/middleware/auth.ts:12:  const key = req.get(\"x-api-key\");"),
    text(R, "Keys come from the `x-api-key` header, so I'll key the limiter on that and give anonymous callers one shared bucket."),
    tool(R, "t3", "Write", {"file_path": f"{tidepool}/src/middleware/rate-limit.ts", "content": MIDDLEWARE},
         "File created successfully"),
    tool(R, "t4", "Edit", {"file_path": f"{tidepool}/src/routes/uploads.ts", "old_string": LIMITER_OLD, "new_string": LIMITER_NEW},
         "The file has been updated"),
    tool(R, "t5", "Bash", {"command": "pnpm test -- uploads", "description": "Run the upload tests"},
         "✓ uploads › accepts a file (12 ms)\n✓ uploads › rejects the 21st upload in a minute with 429 (9 ms)\n\nTests: 2 passed, 2 total"),
    agent(R, type="context", used=41_000),
    text(R, "Uploads are now limited to **20 per minute per API key**.\n\n"
            "- `src/middleware/rate-limit.ts` is a small in-memory sliding window. It answers `429` with a `retry-after` header once the limit is hit.\n"
            "- `src/routes/uploads.ts` applies it to `POST /uploads`.\n"
            "- Anonymous requests share one bucket, so they're limited together.\n\n"
            "The window lives in process memory, so with more than one instance each keeps its own count. If you scale out, a shared store like Redis is the next step."),
)
R2 = "run-2"
other = lambda q, a: flat(user(q), agent(R2, type="session", id="demo", model="claude-fable-5-1"), text(R2, a))

sessions = [
    session("s1", "Add rate limiting to the upload endpoint", tidepool, 4, rate_limit,
            context={"used": 41_000, "window": 1_000_000}),
    session("s2", "Why does the retry loop double-count?", tidepool, 55,
            other("Why does the retry loop double-count failed uploads?", "The counter is incremented both when the request is attempted and again in the `catch`. Removing the second increment fixes it.")),
    session("s3", "Migrate the health checks to gRPC", harbor, 130,
            other("Migrate the health checks to gRPC.", "Done: `Check` and `Watch` are implemented, and the old HTTP endpoint forwards to them.")),
    session("s4", "Write the deploy guide", driftwood, 300,
            other("Write the deploy guide.", "I've added `docs/deploy.md` covering build, release and rollback.")),
    session("s5", "Speed up the glow() hot path", lantern, 1500,
            other("Speed up glow().", "`glow()` now caches its result; a call is about 40x faster."), agent="codex", model="gpt-5.5"),
]

db = sqlite3.connect(root / "data" / "bach.db")
db.executescript("""
CREATE TABLE sessions (id TEXT PRIMARY KEY, data TEXT NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE entries (session_id TEXT NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY (session_id, seq));
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
""")
for data, entries in sessions:
    db.execute("INSERT INTO sessions VALUES (?,?,?)", (data["id"], json.dumps(data), data["updatedAt"]))
    for i, e in enumerate(entries, 1):
        db.execute("INSERT INTO entries VALUES (?,?,?,?)",
                   (data["id"], i, data["createdAt"] + i * 5_000, json.dumps(e)))
db.execute("PRAGMA user_version = 1")
db.commit()
print(f"demo instance in {root}")
