//! Terminals on the backend host: your login shell in a PTY, kept here (not in any window) so
//! they survive reloads and reconnects. Each keeps a scrollback buffer to redraw from; output is
//! also pushed as numbered `terminal` events.
use crate::store::now_ms;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use bach_protocol::{
    ApiError, ExitStatus, ServerEvent, TerminalEvent, TerminalInfo, TerminalSnapshot,
};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, mpsc};

/// How much output each terminal keeps for redrawing.
const SCROLLBACK: usize = 512 * 1024;

struct Term {
    info: TerminalInfo,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    buffer: VecDeque<u8>,
    seq: u64,
}

#[derive(Clone)]
pub struct Terminals {
    terms: Arc<Mutex<HashMap<String, Term>>>,
    events: broadcast::Sender<ServerEvent>,
}

fn not_found() -> ApiError {
    ApiError::not_found("No such terminal.")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

impl Terminals {
    pub fn new(events: broadcast::Sender<ServerEvent>) -> Self {
        Terminals {
            terms: Arc::default(),
            events,
        }
    }

    fn send(&self, ev: TerminalEvent) {
        let _ = self.events.send(ServerEvent::Terminal(ev));
    }

    pub fn list(&self) -> Vec<TerminalInfo> {
        let mut all: Vec<TerminalInfo> = self
            .terms
            .lock()
            .unwrap()
            .values()
            .map(|t| t.info.clone())
            .collect();
        all.sort_by_key(|t| t.created_at);
        all
    }

    pub fn open(&self, cwd: Option<String>, cols: u16, rows: u16) -> Result<TerminalInfo, ApiError> {
        let cwd = cwd
            .map(|c| crate::fs::expand_home(c.trim()))
            .filter(|c| !c.as_os_str().is_empty())
            .unwrap_or_else(home);
        if !cwd.is_dir() {
            return Err(ApiError::invalid(format!("{} is not a folder.", cwd.display())));
        }
        let size = PtySize {
            rows: rows.max(2),
            cols: cols.max(2),
            pixel_width: 0,
            pixel_height: 0,
        };
        let failed = |e: String| ApiError::failed(format!("Couldn't start a terminal: {e}"));
        let pair = native_pty_system().openpty(size).map_err(|e| failed(e.to_string()))?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(&shell);
        // A login shell, so its profile sets PATH as in a terminal app.
        cmd.arg("-l");
        cmd.cwd(&cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| failed(e.to_string()))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| failed(e.to_string()))?;
        let writer = pair.master.take_writer().map_err(|e| failed(e.to_string()))?;
        let killer = child.clone_killer();

        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let info = TerminalInfo {
            id: id.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            shell: std::path::Path::new(&shell)
                .file_name()
                .map_or(shell.clone(), |n| n.to_string_lossy().into_owned()),
            cols: size.cols,
            rows: size.rows,
            created_at: now_ms(),
            exited: None,
        };
        self.terms.lock().unwrap().insert(
            id.clone(),
            Term {
                info: info.clone(),
                master: pair.master,
                writer,
                killer,
                buffer: VecDeque::new(),
                seq: 0,
            },
        );
        self.send(TerminalEvent::Opened {
            terminal: info.clone(),
        });

        // The PTY is read with blocking calls, on a thread of its own.
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(256);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(mut chunk) = rx.recv().await {
                // Whatever else has arrived goes out in the same event.
                tokio::time::sleep(Duration::from_millis(4)).await;
                while let Ok(more) = rx.try_recv() {
                    chunk.extend(more);
                }
                this.output(&id, &chunk);
            }
            // The shell is gone (or the terminal was closed).
            let status = tokio::task::spawn_blocking(move || child.wait())
                .await
                .ok()
                .and_then(|r| r.ok());
            let status = ExitStatus {
                code: status.map(|s| s.exit_code()),
            };
            let known = {
                let mut terms = this.terms.lock().unwrap();
                match terms.get_mut(&id) {
                    Some(t) => {
                        t.info.exited = Some(status.clone());
                        true
                    }
                    None => false,
                }
            };
            if known {
                this.send(TerminalEvent::Exited {
                    terminal_id: id,
                    status,
                });
            }
        });
        Ok(info)
    }

    fn output(&self, id: &str, chunk: &[u8]) {
        let seq = {
            let mut terms = self.terms.lock().unwrap();
            let Some(t) = terms.get_mut(id) else { return };
            t.buffer.extend(chunk);
            if t.buffer.len() > SCROLLBACK {
                // Drop the oldest output, and start the rest at a line.
                let mut cut = t.buffer.len() - SCROLLBACK;
                if let Some(nl) = t.buffer.range(cut..).position(|b| *b == b'\n') {
                    cut += nl + 1;
                }
                t.buffer.drain(..cut);
            }
            t.seq += 1;
            t.seq
        };
        self.send(TerminalEvent::Output {
            terminal_id: id.to_string(),
            seq,
            data: B64.encode(chunk),
        });
    }

    pub fn snapshot(&self, id: &str) -> Result<TerminalSnapshot, ApiError> {
        let terms = self.terms.lock().unwrap();
        let t = terms.get(id).ok_or_else(not_found)?;
        let (a, b) = t.buffer.as_slices();
        Ok(TerminalSnapshot {
            terminal: t.info.clone(),
            data: B64.encode([a, b].concat()),
            seq: t.seq,
        })
    }

    pub fn input(&self, id: &str, data: &str) -> Result<(), ApiError> {
        let mut terms = self.terms.lock().unwrap();
        let t = terms.get_mut(id).ok_or_else(not_found)?;
        if t.info.exited.is_some() {
            return Err(ApiError::invalid("This terminal's shell has ended."));
        }
        t.writer
            .write_all(data.as_bytes())
            .and_then(|_| t.writer.flush())
            .map_err(|e| ApiError::failed(format!("Couldn't write to the terminal: {e}")))
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), ApiError> {
        let mut terms = self.terms.lock().unwrap();
        let t = terms.get_mut(id).ok_or_else(not_found)?;
        let size = PtySize {
            rows: rows.max(2),
            cols: cols.max(2),
            pixel_width: 0,
            pixel_height: 0,
        };
        t.master
            .resize(size)
            .map_err(|e| ApiError::failed(format!("Couldn't resize the terminal: {e}")))?;
        t.info.cols = size.cols;
        t.info.rows = size.rows;
        Ok(())
    }

    pub fn close(&self, id: &str) -> Result<(), ApiError> {
        let mut t = self.terms.lock().unwrap().remove(id).ok_or_else(not_found)?;
        // Ends the shell; dropping the PTY hangs up on anything it started.
        let _ = t.killer.kill();
        drop(t);
        self.send(TerminalEvent::Closed {
            terminal_id: id.to_string(),
        });
        Ok(())
    }
}
