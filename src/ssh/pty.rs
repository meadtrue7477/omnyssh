//! PTY-backed SSH sessions for multi-session terminal.
//!
//! Each [`PtySession`] spawns the system `ssh` binary inside a PTY, drives a
//! [`vt100::Parser`] in a background reader thread, and exposes the parsed
//! screen state via an `Arc<Mutex<vt100::Parser>>` that the render loop can
//! snapshot without blocking.
//!
//! [`PtyManager`] owns all active sessions and provides a simple API for the
//! application layer.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

use crate::event::AppEvent;
use crate::ssh::client::Host;

/// Stable numeric identifier for a PTY session (mirrors [`crate::event::SessionId`]).
pub type SessionId = u64;

// ---------------------------------------------------------------------------
// SendMasterPty — newtype that re-adds the Send bound erased by the trait object
// ---------------------------------------------------------------------------

/// Wraps `Box<dyn MasterPty>` and asserts `Send`.
///
/// # Safety
/// `portable_pty::native_pty_system()` returns `UnixMasterPty` on Unix and
/// `ConPtyMasterPty` on Windows — both are `Send` in their concrete types but
/// the trait object erases that bound.  We only ever access the master from the
/// single async task that owns this `PtySession`, so there is no cross-thread
/// aliasing.
struct SendMasterPty(Box<dyn MasterPty>);

// SAFETY: see struct-level doc above.
unsafe impl Send for SendMasterPty {}

impl std::ops::Deref for SendMasterPty {
    type Target = dyn MasterPty;
    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

// ---------------------------------------------------------------------------
// PtySession
// ---------------------------------------------------------------------------

/// A running SSH process inside a PTY together with its VT100 parser state.
///
/// Dropping this struct kills the child SSH process and joins the reader thread.
pub struct PtySession {
    /// Unique identifier for this session.
    pub id: SessionId,
    /// Display name (= `host.name`).
    pub host_name: String,
    /// Shared VT100 parser. The reader thread writes into it; the render loop
    /// takes a read-side snapshot. Never held for more than a few microseconds
    /// to avoid blocking the reader.
    screen: Arc<Mutex<vt100::Parser>>,
    /// Keyboard / paste input → PTY master stdin.
    writer: Box<dyn Write + Send>,
    /// PTY master handle kept for `resize()`. `take_writer()` takes the write fd
    /// out of the master but leaves the master object (and its resize fd) intact,
    /// so we hold it here to send `TIOCSWINSZ` / `SIGWINCH` on resize.
    ///
    /// PTY master — wrapped in [`SendMasterPty`] to restore the `Send` bound that
    /// the `Box<dyn MasterPty>` trait object erases.  Only ever accessed from the
    /// async task that owns this `PtySession`.
    master: SendMasterPty,
    /// Kept alive so the SSH process stays connected. Dropping this sends
    /// SIGHUP to the child, causing the reader to see EOF.
    _child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Background reader thread handle (joined on drop).
    reader_thread: Option<std::thread::JoinHandle<()>>,
}

impl PtySession {
    /// Spawns `ssh <args>` inside a freshly-allocated PTY.
    ///
    /// * `cols` / `rows` — initial PTY dimensions (should match the render area).
    /// * `tx` — application event channel; `PtyOutput` / `PtyExited` are sent here.
    ///
    /// # Errors
    /// Returns an error if the PTY cannot be allocated or SSH cannot be spawned.
    pub fn spawn(
        id: SessionId,
        host: &Host,
        cols: u16,
        rows: u16,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system
            .openpty(size)
            .context("failed to open PTY pair")?;

        // Build SSH command (same flags as the former connect_system_ssh).
        let mut cmd = CommandBuilder::new("ssh");
        cmd.args(["-o", "ConnectTimeout=10"]);
        // Ensure the remote side allocates a PTY (-t) for interactive use.
        cmd.arg("-t");
        if host.port != 22 {
            cmd.args(["-p", &host.port.to_string()]);
        }
        if let Some(ref key) = host.identity_file {
            cmd.args(["-i", key]);
        }
        if let Some(ref jump) = host.proxy_jump {
            cmd.args(["-J", jump]);
        }
        cmd.arg(format!("{}@{}", host.user, host.hostname));

        // Spawn the child — slave is consumed and becomes the child's controlling TTY.
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to spawn SSH in PTY")?;

        // Writer (keyboard → SSH stdin) and reader (SSH stdout → parser).
        // take_writer() takes the write-fd out of the master but leaves the master
        // object intact, so we can still call resize() on it later.
        let writer = pair
            .master
            .take_writer()
            .context("failed to get PTY writer")?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let master = SendMasterPty(pair.master);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 1000)));
        let parser_clone = Arc::clone(&parser);

        // Reader thread: blocking I/O, forwards bytes to the vt100 parser and
        // sends a lightweight PtyOutput notification (no bulk data copy) so the
        // main loop can update `has_activity` state.
        let reader_thread = std::thread::Builder::new()
            .name(format!("pty-reader-{id}"))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut buf = [0u8; 4096];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => {
                                let _ = tx.blocking_send(AppEvent::PtyExited(id));
                                break;
                            }
                            Ok(n) => {
                                // Process in small sub-chunks (256 B) so the mutex
                                // is released between chunks.  Without this, a single
                                // 4 KB read holds the lock for the entire vt100 parse
                                // pass, starving the render thread and causing UI freeze
                                // during large output (e.g. `cat big_file`).
                                const CHUNK: usize = 256;
                                let mut off = 0;
                                while off < n {
                                    let end = (off + CHUNK).min(n);
                                    if let Ok(mut p) = parser_clone.lock() {
                                        p.process(&buf[off..end]);
                                    }
                                    off = end;
                                }
                                let _ = tx.blocking_send(AppEvent::PtyOutput(id));
                            }
                        }
                    }
                }));
                if let Err(e) = result {
                    tracing::error!(session = id, "PTY reader thread panicked: {:?}", e);
                    // Notify the app so the tab can be cleaned up.
                    let _ = tx.blocking_send(AppEvent::PtyExited(id));
                }
            })
            .context("failed to spawn PTY reader thread")?;

        tracing::info!("PTY session {} opened for host '{}'", id, host.name);
        Ok(Self {
            id,
            host_name: host.name.clone(),
            screen: parser,
            writer,
            master,
            _child: child,
            reader_thread: Some(reader_thread),
        })
    }

    /// Sends raw bytes to the PTY (keystrokes, pasted text, etc.).
    ///
    /// # Errors
    /// Returns an error if the write fails (e.g. the PTY master was closed).
    pub fn write_input(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data).context("PTY write failed")?;
        self.writer.flush().context("PTY flush failed")
    }

    /// Notifies the PTY and the vt100 parser about a dimension change.
    ///
    /// Should be called whenever the render area allocated to this session
    /// changes (terminal resize or split-view toggle).
    ///
    /// # Errors
    /// Returns an error if the PTY resize syscall fails.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        // Resize the kernel PTY — this triggers SIGWINCH in the child process so
        // interactive programs (vim, htop, less) reflow to the new dimensions.
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("PTY resize (TIOCSWINSZ) failed")?;
        // Also update the vt100 parser so the local screen model matches.
        if let Ok(mut p) = self.screen.lock() {
            p.set_size(rows, cols);
        }
        Ok(())
    }

    /// Returns a clone of the `Arc` so the renderer can lock and snapshot the
    /// screen without requiring a mutable reference to `PtySession`.
    pub fn parser_arc(&self) -> Arc<Mutex<vt100::Parser>> {
        Arc::clone(&self.screen)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Drop order matters:
        //   1. writer is dropped first → SSH stdin gets EOF → SSH may exit cleanly.
        //   2. _child is dropped   → SIGHUP/SIGTERM sent to child process.
        //   3. When the child exits, the slave PTY fd closes → the master returns
        //      EIO on the next read → the reader thread breaks out of its loop.
        //
        // We intentionally do NOT call t.join() here. `Drop` runs on the main
        // tokio task; join() would block the entire async runtime until the
        // reader thread exits (which requires the PTY read to return — may take
        // hundreds of milliseconds). Dropping the JoinHandle detaches the thread
        // instead; it self-terminates once the child exits.
        if let Some(t) = self.reader_thread.take() {
            drop(t); // detach — thread exits on its own after PTY closes
        }
        tracing::info!("PTY session {} closed", self.id);
    }
}

// ---------------------------------------------------------------------------
// PtyManager
// ---------------------------------------------------------------------------

/// Manages all active [`PtySession`]s.
///
/// Stored in [`crate::app::App`] (not in `AppState`) to avoid reference cycles.
/// Dropping `PtyManager` or calling [`PtyManager::shutdown`] closes all
/// sessions gracefully.
pub struct PtyManager {
    sessions: Vec<PtySession>,
    next_id: u64,
}

impl PtyManager {
    /// Creates an empty manager.
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            next_id: 1,
        }
    }

    /// Opens a new PTY tab for `host` and returns the assigned [`SessionId`].
    ///
    /// # Errors
    /// Propagates errors from [`PtySession::spawn`].
    pub fn open(
        &mut self,
        host: &Host,
        cols: u16,
        rows: u16,
        tx: mpsc::Sender<AppEvent>,
    ) -> Result<SessionId> {
        let id = self.next_id;
        self.next_id += 1;
        let session = PtySession::spawn(id, host, cols, rows, tx)?;
        self.sessions.push(session);
        Ok(id)
    }

    /// Sends raw bytes to the session identified by `id`.
    ///
    /// # Errors
    /// Returns an error if the write fails or the session does not exist.
    pub fn write(&mut self, id: SessionId, data: &[u8]) -> Result<()> {
        match self.sessions.iter_mut().find(|s| s.id == id) {
            Some(s) => s.write_input(data),
            None => Ok(()), // session already gone — silently ignore
        }
    }

    /// Resizes the session identified by `id`. No-op if the session was not found.
    ///
    /// # Errors
    /// Propagates errors from [`PtySession::resize`].
    pub fn resize(&mut self, id: SessionId, cols: u16, rows: u16) -> Result<()> {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.resize(cols, rows)?;
        }
        Ok(())
    }

    /// Closes and removes the session with the given `id`.
    ///
    /// Dropping `PtySession` kills the child process and joins the reader thread.
    pub fn close(&mut self, id: SessionId) {
        self.sessions.retain(|s| s.id != id);
    }

    /// Gracefully shuts down all sessions.
    pub fn shutdown(mut self) {
        self.sessions.clear(); // Drop in order → each PtySession drops, killing child + joining thread.
    }

    /// Returns the parser `Arc` for the session with the given `id`, if any.
    pub fn parser_for(&self, id: SessionId) -> Option<Arc<Mutex<vt100::Parser>>> {
        self.sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.parser_arc())
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// key_to_bytes — convert crossterm KeyEvent to PTY stdin bytes
// ---------------------------------------------------------------------------

/// Converts a crossterm [`KeyEvent`] to the raw byte sequence that should be
/// written to PTY stdin.
///
/// Returns an empty `Vec` for events that have no meaningful byte
/// representation (e.g. lone modifier keys). The caller discards empty vecs.
///
/// This function is `pub` so `app.rs` can call it from the terminal input handler.
pub fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        // ----------------------------------------------------------------
        // Printable characters
        // ----------------------------------------------------------------
        KeyCode::Char(c) if ctrl => {
            // Control codes: Ctrl+A = 0x01, Ctrl+Z = 0x1A.
            match c {
                'a'..='z' => vec![c as u8 - b'a' + 1],
                'A'..='Z' => vec![c as u8 - b'A' + 1],
                '[' => vec![0x1b], // Ctrl+[ = ESC
                '\\' => vec![0x1c],
                ']' => vec![0x1d],
                '^' => vec![0x1e],
                '_' => vec![0x1f],
                '@' => vec![0x00], // Ctrl+@ = NUL
                _ => vec![c as u8],
            }
        }
        KeyCode::Char(c) if alt => {
            // Alt sequences: ESC + char.
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(s.as_bytes());
            bytes
        }
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            s.as_bytes().to_vec()
        }

        // ----------------------------------------------------------------
        // Special keys
        // ----------------------------------------------------------------
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                vec![0x1b, b'[', b'Z'] // Shift+Tab (reverse-tab)
            } else {
                vec![0x09]
            }
        }
        KeyCode::Esc => vec![0x1b],

        // ----------------------------------------------------------------
        // Cursor keys (DECCKM off — application mode not yet detected)
        // ----------------------------------------------------------------
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],

        // ----------------------------------------------------------------
        // Edit keys
        // ----------------------------------------------------------------
        KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],

        // ----------------------------------------------------------------
        // Function keys (xterm/VT220 encoding)
        // ----------------------------------------------------------------
        KeyCode::F(1) => vec![0x1b, b'O', b'P'],
        KeyCode::F(2) => vec![0x1b, b'O', b'Q'],
        KeyCode::F(3) => vec![0x1b, b'O', b'R'],
        KeyCode::F(4) => vec![0x1b, b'O', b'S'],
        KeyCode::F(5) => vec![0x1b, b'[', b'1', b'5', b'~'],
        KeyCode::F(6) => vec![0x1b, b'[', b'1', b'7', b'~'],
        KeyCode::F(7) => vec![0x1b, b'[', b'1', b'8', b'~'],
        KeyCode::F(8) => vec![0x1b, b'[', b'1', b'9', b'~'],
        KeyCode::F(9) => vec![0x1b, b'[', b'2', b'0', b'~'],
        KeyCode::F(10) => vec![0x1b, b'[', b'2', b'1', b'~'],
        KeyCode::F(11) => vec![0x1b, b'[', b'2', b'3', b'~'],
        KeyCode::F(12) => vec![0x1b, b'[', b'2', b'4', b'~'],

        // Unknown — produce nothing so callers can skip the write.
        _ => vec![],
    }
}
