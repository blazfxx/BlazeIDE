//! Integrated terminal sessions on a real pseudo-terminal (ConPTY on
//! Windows, openpty on macOS/Linux) via portable-pty.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::error::{Error, Result};

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

pub type DataFn = Arc<dyn Fn(&str, String) + Send + Sync>;
pub type ExitFn = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default, Clone)]
pub struct PtyManager {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
}

fn default_shell() -> CommandBuilder {
    #[cfg(windows)]
    {
        let mut c = CommandBuilder::new("powershell.exe");
        c.arg("-NoLogo");
        c
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
        let shell = if Path::new(&shell).exists() { shell } else { "/bin/sh".into() };
        let mut c = CommandBuilder::new(shell);
        c.arg("-l");
        c
    }
}

fn clamp(v: u16, min: u16, max: u16) -> u16 {
    v.clamp(min, max)
}

/// Splits a byte stream into valid UTF-8 strings, carrying an incomplete
/// multi-byte sequence over to the next chunk instead of mangling it.
#[derive(Default)]
pub struct Utf8Carry(Vec<u8>);

impl Utf8Carry {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.0.extend_from_slice(bytes);
        match std::str::from_utf8(&self.0) {
            Ok(s) => {
                let out = s.to_string();
                self.0.clear();
                out
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // An invalid byte (not just a truncated tail): emit lossily.
                if e.error_len().is_some() {
                    let out = String::from_utf8_lossy(&self.0).into_owned();
                    self.0.clear();
                    return out;
                }
                let out = String::from_utf8_lossy(&self.0[..valid]).into_owned();
                self.0.drain(..valid);
                out
            }
        }
    }
}

impl PtyManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&self, cwd: &Path, on_data: DataFn, on_exit: ExitFn) -> Result<String> {
        let pair = native_pty_system()
            .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| Error::msg(format!("Could not open a terminal: {e}")))?;
        let mut cmd = default_shell();
        let dir = if cwd.is_dir() { cwd.to_path_buf() } else { dirs::home_dir().unwrap_or_else(|| cwd.to_path_buf()) };
        cmd.cwd(dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "BlazeIDE");
        let child = pair.slave.spawn_command(cmd).map_err(|e| Error::msg(format!("Could not start the shell: {e}")))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| Error::msg(e.to_string()))?;
        let writer = pair.master.take_writer().map_err(|e| Error::msg(e.to_string()))?;
        let id = uuid::Uuid::new_v4().to_string();
        self.sessions.lock().expect("pty lock").insert(id.clone(), Session { master: pair.master, writer, child });

        let sessions = self.sessions.clone();
        let tid = id.clone();
        std::thread::Builder::new()
            .name(format!("pty-{}", &id[..8]))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                let mut carry = Utf8Carry::default();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let text = carry.push(&buf[..n]);
                            if !text.is_empty() {
                                on_data(&tid, text);
                            }
                        }
                    }
                }
                if let Some(mut s) = sessions.lock().expect("pty lock").remove(&tid) {
                    let _ = s.child.kill();
                }
                on_exit(&tid);
            })
            .map_err(|e| Error::msg(e.to_string()))?;
        Ok(id)
    }

    pub fn write(&self, id: &str, data: &str) {
        if let Some(s) = self.sessions.lock().expect("pty lock").get_mut(id) {
            let _ = s.writer.write_all(data.as_bytes());
            let _ = s.writer.flush();
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if let Some(s) = self.sessions.lock().expect("pty lock").get(id) {
            let _ = s.master.resize(PtySize { rows: clamp(rows, 1, 1000), cols: clamp(cols, 2, 1000), pixel_width: 0, pixel_height: 0 });
        }
    }

    pub fn kill(&self, id: &str) {
        if let Some(mut s) = self.sessions.lock().expect("pty lock").remove(id) {
            let _ = s.child.kill();
        }
    }

    pub fn kill_all(&self) {
        let mut map = self.sessions.lock().expect("pty lock");
        for (_, mut s) in map.drain() {
            let _ = s.child.kill();
        }
    }

    pub fn count(&self) -> usize {
        self.sessions.lock().expect("pty lock").len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn utf8_carry_handles_split_sequences() {
        let mut c = Utf8Carry::default();
        let bytes = "héllo 日本".as_bytes();
        let mut out = String::new();
        for b in bytes {
            out.push_str(&c.push(&[*b]));
        }
        assert_eq!(out, "héllo 日本");
        assert_eq!(c.push(&[0xff, b'a']), "\u{fffd}a");
    }

    #[cfg(unix)]
    #[test]
    fn echo_roundtrip_and_exit() {
        let d = tempfile::tempdir().unwrap();
        let m = PtyManager::new();
        let (tx, rx) = mpsc::channel::<String>();
        let (etx, erx) = mpsc::channel::<()>();
        let txc = Mutex::new(tx);
        let etxc = Mutex::new(etx);
        let id = m
            .create(
                d.path(),
                Arc::new(move |_, s| { let _ = txc.lock().unwrap().send(s); }),
                Arc::new(move |_| { let _ = etxc.lock().unwrap().send(()); }),
            )
            .unwrap();
        m.resize(&id, 120, 40);
        m.write(&id, "echo blaze_$((40+2))\n");
        let mut seen = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !seen.contains("blaze_42") && std::time::Instant::now() < deadline {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) {
                seen.push_str(&s);
            }
        }
        assert!(seen.contains("blaze_42"), "{seen}");
        m.write(&id, "exit\n");
        assert!(erx.recv_timeout(Duration::from_secs(10)).is_ok());
        assert_eq!(m.count(), 0);
    }
}
