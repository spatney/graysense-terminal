//! Terminal bays: shells in pseudoterminals (ConPTY on Windows) streamed to xterm.js.
//!
//! A terminal is a [`Session`]: a shell in a PTY and four threads. One reads what the shell
//! prints and one sends it to the UI over the terminal's channel as text, batched when it comes
//! faster than it can be sent. One writes keystrokes in the order they came, and one waits for
//! the shell to end, then says so after its last output. The UI pauses the output while xterm.js
//! is behind (a command printing a flood): the reader stops reading, the PTY's buffer fills and
//! the command waits, as in any terminal.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::ipc::Channel;

pub mod shells;
mod utf8;

pub use shells::Shell;

const READ_CHUNK: usize = 64 * 1024;
/// The most one output event carries when output queues up faster than it's sent.
const MAX_BATCH: usize = 256 * 1024;
/// How long a shell that ended has to hand over the rest of its output.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TerminalEvent {
    /// What the shell printed, escapes and all.
    Output { data: String },
    /// The shell ended, after its last output.
    Exit { code: Option<u32> },
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    pub shell_id: String,
    pub shell_name: String,
    pub cwd: String,
    pub pid: Option<u32>,
    /// Windows' build number, which decides how xterm.js treats ConPTY.
    pub windows_build: Option<u32>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TerminalShell {
    pub id: String,
    pub name: String,
    pub path: String,
    pub default: bool,
}

pub struct Launch {
    pub id: String,
    pub cwd: PathBuf,
    pub shell: Shell,
    pub cols: u16,
    pub rows: u16,
}

/// Every open terminal, by id (Tauri managed state).
#[derive(Default, Clone)]
pub struct Terminals {
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
}

/// The environment a terminal starts with: Graysense's own, marked as a 256-colour terminal.
fn environment() -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !matches!(k.to_string_lossy().to_ascii_uppercase().as_str(), "TERM" | "COLORTERM" | "TERM_PROGRAM" | "TERM_PROGRAM_VERSION"))
        .collect();
    env.push(("TERM".into(), "xterm-256color".into()));
    env.push(("COLORTERM".into(), "truecolor".into()));
    env.push(("TERM_PROGRAM".into(), "Graysense".into()));
    env.push(("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into()));
    env
}

impl Terminals {
    /// Start `launch.shell`, streaming what it prints, then how it ended, to `events`.
    pub fn spawn(&self, launch: Launch, events: Channel<TerminalEvent>) -> Result<TerminalInfo, String> {
        if let Some(old) = self.sessions.lock().unwrap().remove(&launch.id) {
            // The UI reloaded and is starting this bay's shell again.
            old.kill();
        }
        let size = pty_size(launch.cols, launch.rows);
        let pair = native_pty_system().openpty(size).map_err(|e| format!("Couldn't open a terminal: {e}"))?;
        let mut command = CommandBuilder::new(&launch.shell.program);
        command.args(&launch.shell.args);
        command.cwd(&launch.cwd);
        command.env_clear();
        for (key, value) in environment() {
            command.env(key, value);
        }
        for (key, value) in &launch.shell.env {
            command.env(key, value);
        }
        let mut child = pair.slave.spawn_command(command).map_err(|e| format!("Couldn't start {}: {e}", launch.shell.name))?;
        drop(pair.slave);

        let pid = child.process_id();
        let ends = pair.master.try_clone_reader().and_then(|reader| Ok((reader, pair.master.take_writer()?)));
        let (reader, writer) = match ends {
            Ok(ends) => ends,
            Err(e) => {
                let _ = child.kill();
                return Err(format!("Couldn't connect to {}: {e}", launch.shell.name));
            }
        };

        let (input, keystrokes) = mpsc::channel::<Vec<u8>>();
        let session = Arc::new(Session {
            input: Mutex::new(Some(input)),
            master: Mutex::new(Some(pair.master)),
            killer: Mutex::new(child.clone_killer()),
            pid,
            exited: AtomicBool::new(false),
            flow: Flow::default(),
            outlet: Mutex::new(Outlet { events, ended: false }),
        });
        let id = launch.id;
        self.sessions.lock().unwrap().insert(id.clone(), session.clone());

        let (chunks, received) = mpsc::channel::<Vec<u8>>();
        let (drained, all_sent) = mpsc::channel::<()>();
        let (reading, sending, waiting) = (session.clone(), session.clone(), session.clone());
        let sessions = self.sessions.clone();
        let waiting_id = id.clone();
        let threads: [(&str, Box<dyn FnOnce() + Send>); 4] = [
            ("input", Box::new(move || write_input(writer, keystrokes))),
            ("read", Box::new(move || read_output(reader, chunks, &reading.flow))),
            ("output", Box::new(move || send_output(&sending, received, drained))),
            (
                "wait",
                Box::new(move || {
                    wait_for_exit(&waiting, child, all_sent);
                    let mut open = sessions.lock().unwrap();
                    if open.get(&waiting_id).is_some_and(|s| Arc::ptr_eq(s, &waiting)) {
                        open.remove(&waiting_id);
                    }
                }),
            ),
        ];
        for (name, run) in threads {
            if let Err(e) = std::thread::Builder::new().name(format!("terminal-{name}")).spawn(run) {
                session.kill();
                self.sessions.lock().unwrap().remove(&id);
                return Err(format!("Couldn't start the terminal: {e}"));
            }
        }
        Ok(TerminalInfo { id, shell_id: launch.shell.id, shell_name: launch.shell.name, cwd: launch.cwd.display().to_string(), pid, windows_build: windows_build() })
    }

    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let session = self.get(id)?;
        let input = session.input.lock().unwrap();
        match input.as_ref() {
            Some(input) if input.send(data.as_bytes().to_vec()).is_ok() => Ok(()),
            _ => Err(ENDED.into()),
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let session = self.get(id)?;
        let master = session.master.lock().unwrap();
        match master.as_ref() {
            Some(master) => master.resize(pty_size(cols, rows)).map_err(|e| format!("Couldn't resize the terminal: {e}")),
            None => Ok(()),
        }
    }

    pub fn set_paused(&self, id: &str, paused: bool) -> Result<(), String> {
        self.get(id)?.flow.set(paused);
        Ok(())
    }

    pub fn kill(&self, id: &str) {
        if let Ok(session) = self.get(id) {
            session.kill();
        }
    }

    pub fn kill_all(&self) {
        let open: Vec<Arc<Session>> = self.sessions.lock().unwrap().values().cloned().collect();
        for session in open {
            session.kill();
        }
    }

    fn get(&self, id: &str) -> Result<Arc<Session>, String> {
        self.sessions.lock().unwrap().get(id).cloned().ok_or_else(|| ENDED.into())
    }
}

const ENDED: &str = "That terminal has ended.";

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 }
}

/// Windows' build number: xterm.js reflows lines itself only where ConPTY no longer redraws them
/// on a resize (build 21376 and later).
#[cfg(windows)]
fn windows_build() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (key, name) = (wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"), wide("CurrentBuildNumber"));
    let mut value = [0u16; 32];
    let mut bytes = (value.len() * 2) as u32;
    // SAFETY: valid NUL-terminated strings and an output buffer of `bytes` bytes.
    let status = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), value.as_mut_ptr().cast(), &mut bytes) };
    if status != 0 {
        return None;
    }
    let units = (bytes as usize / 2).min(value.len());
    String::from_utf16_lossy(&value[..units]).trim_end_matches('\0').trim().parse().ok()
}

#[cfg(not(windows))]
fn windows_build() -> Option<u32> {
    None
}

struct Session {
    input: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    exited: AtomicBool,
    flow: Flow,
    outlet: Mutex<Outlet>,
}

/// The UI's channel. Events go out under its lock, so none follows the exit.
struct Outlet {
    events: Channel<TerminalEvent>,
    ended: bool,
}

impl Session {
    fn emit(&self, event: TerminalEvent) {
        let outlet = self.outlet.lock().unwrap();
        if !outlet.ended {
            let _ = outlet.events.send(event);
        }
    }

    fn end(&self, code: Option<u32>) {
        let mut outlet = self.outlet.lock().unwrap();
        if !outlet.ended {
            outlet.ended = true;
            let _ = outlet.events.send(TerminalEvent::Exit { code });
        }
    }

    fn close_pty(&self) {
        let master = self.master.lock().unwrap().take();
        drop(master);
    }

    fn kill(self: &Arc<Self>) {
        self.flow.set(false);
        if self.exited.load(Ordering::Acquire) {
            return;
        }
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            kill_tree(pid);
        }
        let _ = self.killer.lock().unwrap().kill();
        self.close_pty();
    }
}

/// End a process and everything it started.
#[cfg(windows)]
pub fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[derive(Default)]
struct Flow {
    paused: Mutex<bool>,
    changed: Condvar,
}

impl Flow {
    fn set(&self, paused: bool) {
        *self.paused.lock().unwrap() = paused;
        self.changed.notify_all();
    }

    fn wait_while_paused(&self) {
        let mut paused = self.paused.lock().unwrap();
        while *paused {
            paused = self.changed.wait(paused).unwrap();
        }
    }
}

fn write_input(mut writer: Box<dyn Write + Send>, keystrokes: mpsc::Receiver<Vec<u8>>) {
    while let Ok(bytes) = keystrokes.recv() {
        if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
            break;
        }
    }
}

fn read_output(mut reader: Box<dyn Read + Send>, chunks: mpsc::Sender<Vec<u8>>, flow: &Flow) {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        flow.wait_while_paused();
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if chunks.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn send_output(session: &Session, received: mpsc::Receiver<Vec<u8>>, drained: mpsc::Sender<()>) {
    let mut text = utf8::Utf8Stream::default();
    while let Ok(mut bytes) = received.recv() {
        while bytes.len() < MAX_BATCH {
            match received.try_recv() {
                Ok(more) => bytes.extend_from_slice(&more),
                Err(_) => break,
            }
        }
        let data = text.decode(&bytes);
        if !data.is_empty() {
            session.emit(TerminalEvent::Output { data });
        }
    }
    let data = text.finish();
    if !data.is_empty() {
        session.emit(TerminalEvent::Output { data });
    }
    let _ = drained.send(());
}

fn wait_for_exit(session: &Session, mut child: Box<dyn Child + Send + Sync>, all_sent: mpsc::Receiver<()>) {
    let code = child.wait().ok().map(|status| status.exit_code());
    session.exited.store(true, Ordering::Release);
    session.input.lock().unwrap().take();
    session.flow.set(false);
    // ConPTY keeps the output open until the pseudoconsole closes.
    session.close_pty();
    let _ = all_sent.recv_timeout(DRAIN_GRACE);
    session.end(code);
}
