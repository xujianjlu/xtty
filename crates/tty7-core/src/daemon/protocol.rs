use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// A frame is a little-endian `u32` length, a one-byte kind, then the payload.
const HEADER: usize = 5;

pub const PROTOCOL_VERSION: u32 = 6;

pub const FEATURE_PANE_OWNER: &str = "pane-owner";

/// The daemon echoes a `DaemonMsg::Size` to the controlling subscriber, in
/// stream order, when it applies a `ClientMsg::Resize`. A client that sees
/// this feature defers its local grid reflow to that echo so the reflow lands
/// at the stream position where the PTY actually changed geometry; against an
/// older daemon it must keep reflowing locally at request time.
pub const FEATURE_RESIZE_ECHO: &str = "resize-echo";

/// The daemon can seed a new pane with the screen a dead one left behind, named
/// by `ClientMsg::Spawn`'s `restore` field. A client that does not see this
/// feature leaves the field out: an older daemon would ignore it and spawn a
/// blank pane, which is the same outcome, but sending it would make the wire
/// claim a restore that never happened.
pub const FEATURE_RESTORE_SCROLLBACK: &str = "restore-scrollback";

/// The daemon can replace its own binary without stopping, keeping every pty
/// and everything running on one — `ClientMsg::Handoff`. Advertised only where
/// it can actually be done, which is where `execve` exists, so a client can use
/// it to choose between offering an upgrade that costs the user nothing and one
/// that costs them every running command.
pub const FEATURE_HANDOFF: &str = "handoff";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonVersion {
    pub protocol: u32,
    #[serde(default)]
    pub build: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub instance: String,
}

impl DaemonVersion {
    pub fn current() -> DaemonVersion {
        let mut features = vec![
            FEATURE_PANE_OWNER.to_string(),
            FEATURE_RESIZE_ECHO.to_string(),
            FEATURE_RESTORE_SCROLLBACK.to_string(),
        ];
        if cfg!(unix) {
            features.push(FEATURE_HANDOFF.to_string());
        }
        DaemonVersion {
            protocol: PROTOCOL_VERSION,
            build: env!("CARGO_PKG_VERSION").to_string(),
            features,
            instance: process_instance().to_string(),
        }
    }

    pub fn has_feature(&self, name: &str) -> bool {
        self.features.iter().any(|f| f == name)
    }
}

pub fn process_instance() -> &'static str {
    static INSTANCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellSpec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub args_are_tty7_defaults: bool,
}

pub fn ssh_option_takes_value(flag: char) -> bool {
    matches!(
        flag,
        'B' | 'b'
            | 'c'
            | 'D'
            | 'E'
            | 'e'
            | 'F'
            | 'I'
            | 'i'
            | 'J'
            | 'L'
            | 'l'
            | 'm'
            | 'O'
            | 'o'
            | 'p'
            | 'Q'
            | 'R'
            | 'S'
            | 'W'
            | 'w'
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane_id: u64,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub title: String,
    /// See [`crate::core::machine::PaneRecord::osc_title`] — the terminal's own
    /// title, not the foreground process name `title` carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub osc_title: Option<String>,
    pub alive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteContext {
    pub kind: RemoteKind,
    pub argv: Vec<String>,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteKind {
    Ssh,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopbackForwardRequest {
    pub pane_id: u64,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopbackForward {
    pub local_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcEntry {
    pub pid: u32,
    pub name: String,
    pub depth: u8,
    #[serde(default)]
    pub foreground: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortEntry {
    pub port: u16,
    pub pid: u32,
    pub name: String,
    /// The address the socket is bound to, as `lsof` spells it — `*`,
    /// `0.0.0.0`, `127.0.0.1`, `[::1]`, or a specific interface.
    ///
    /// `serde(default)` because a daemon from before this field existed
    /// answers `QueryProcs` without it, and an empty address is read the same
    /// way that daemon's callers read every address: as localhost.
    #[serde(default)]
    pub addr: String,
}

impl PortEntry {
    /// Whether a bound address can be reached on this machine's loopback —
    /// true for the wildcards and the loopback addresses themselves, false for
    /// a socket pinned to one specific non-loopback interface.
    pub fn reaches_loopback(addr: &str) -> bool {
        matches!(
            addr,
            "" | "*" | "0.0.0.0" | "::" | "[::]" | "127.0.0.1" | "::1" | "[::1]" | "localhost"
        )
    }

    /// What to copy, and what to open in a browser: `host:port`.
    ///
    /// Loopback and wildcard binds are spelled `localhost`, which is what
    /// anyone typing the address by hand would write. A socket bound to one
    /// specific interface keeps that interface — the panel presents this as
    /// the address of the pane's server, and `localhost` would not be it.
    pub fn authority(&self) -> String {
        match Self::reaches_loopback(&self.addr) {
            true => format!("localhost:{}", self.port),
            false => format!("{}:{}", self.addr, self.port),
        }
    }
}

/// Whether the answer in `PaneProcs::ports` can be believed.
///
/// An empty port list used to mean two very different things at once: nothing
/// in this pane is listening, or the thing that looks for listeners never got
/// to say. On unix that look is an `lsof` subprocess, and every way it can go
/// wrong — absent from the daemon's `PATH`, killed, hung on a wedged mount,
/// pointed at sockets it has no permission to read — arrived as the same empty
/// vector as a genuinely quiet pane. Whoever reads the list gets to know which
/// one it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum PortProbe {
    /// The probe ran and the list is its whole answer.
    #[default]
    Ok,
    /// The probe ran, but at least one process in this pane belongs to another
    /// user — `sudo go run`, a root-owned server on a low port — and a probe
    /// running as this user cannot see that process's sockets. The list holds
    /// what could be seen, which may be nothing.
    Restricted,
    /// The probe could not be run at all. The string is for a log line or for
    /// `tty7 procs`, not for the panel: it names the tool and what went wrong.
    Unavailable(String),
}

impl PortProbe {
    pub fn is_ok(&self) -> bool {
        matches!(self, PortProbe::Ok)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneProcs {
    pub procs: Vec<ProcEntry>,
    pub ports: Vec<PortEntry>,
    /// `serde(default)` because a daemon from before this field existed
    /// answers `QueryProcs` without it, and its silence is read the way that
    /// daemon's callers read every answer it gives: as a complete one.
    #[serde(default)]
    pub probe: PortProbe,
    /// What the pane can say about itself that the process list cannot — see
    /// [`PaneContext`]. `None` from a daemon built before the field existed,
    /// which reads as "this daemon cannot say", never as a set of falses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<PaneContext>,
}

/// Where a pane's session actually lives, and what its shell says about itself.
///
/// The process list beside it is always *this* machine's: it starts at the
/// pty's own child and walks down. For a pane that is only the near end of a
/// connection — an `ssh` the shell is running, a native-SSH pane whose pty is
/// on another host — that list describes the tunnel, not the work. This is the
/// part of the answer that can still speak for the far side.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneContext {
    /// The host the pane is pointed at, when it is not this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteContext>,
    /// Whether this machine holds the pane's pty at all. `false` for a
    /// native-SSH pane, whose `procs` is empty because there is nothing here
    /// to walk — not because the walk failed.
    pub local_pty: bool,
    /// What the pane's shell integration last said, `None` until it emits its
    /// first OSC 133 mark.
    ///
    /// This is the mark's own reading, taken before the suppression that keeps
    /// a foreground program's prompt marks from engaging the local line editor.
    /// That suppression is right for the editor and wrong here: on a pane
    /// running `ssh` the marks are the *far* shell's, and they are the only
    /// thing on this side that knows whether the far shell is busy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_prompt: Option<bool>,
    /// Whether a prompt mark has arrived while the pane was pointed at
    /// `remote`. Only the far shell can be at a prompt while the near one is
    /// occupied by the connection, so this is the proof that the far side's
    /// shell integration is loaded and reporting. Without it, "not at a
    /// prompt" on a remote pane means nothing: the newest mark is then the
    /// near shell's own "I started `ssh`", and it will never be replaced.
    pub remote_prompt_seen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    Spawn {
        cwd: Option<PathBuf>,
        size: WinSize,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        workspace: Option<String>,
        restore: Option<RestoreFrom>,
        allow_remote_clipboard_write: bool,
    },
    Attach {
        pane_id: u64,
        size: WinSize,
        allow_remote_clipboard_write: bool,
    },
    Observe {
        pane_id: u64,
        size: WinSize,
    },
    Input(Vec<u8>),
    SendInput {
        pane_id: u64,
        bytes: Vec<u8>,
    },
    Resize(WinSize),
    Detach,
    Kill {
        pane_id: u64,
    },
    List,
    Shutdown,
    /// Become `exe` without stopping: the daemon rewrites itself in place and
    /// keeps every pty, shell and pane id it is holding. The connection dies in
    /// the process — the new image has never heard of it — so this is the last
    /// thing a client can say on it, and the reply is the socket closing.
    ///
    /// Unix only. Elsewhere the daemon answers with an error and the caller
    /// falls back to stopping and starting it.
    Handoff {
        exe: PathBuf,
    },
    EnsureLoopbackForward(LoopbackForwardRequest),
    QueryProcs {
        pane_id: u64,
    },
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonMsg {
    Spawned {
        pane_id: u64,
    },
    Size(WinSize),
    Snapshot(Vec<u8>),
    Output(Vec<u8>),
    /// A kitty graphics image lifted out of the PTY stream daemon-side (issue
    /// #213). Carried out-of-band as a compact binary frame
    /// ([`crate::core::kitty_graphics::Image::encode_frame`]) so the base64 text
    /// never rides the socket and the client's VT parser never sees it. The
    /// pixel payload stays *compressed* on the wire — the client inflates — so a
    /// remote pane's frames don't balloon across the SSH tunnel.
    Image(Vec<u8>),
    /// A kitty graphics delete (`a=d`) lifted out of the PTY stream daemon-side.
    /// Payload is a compact selector frame
    /// ([`crate::core::kitty_graphics::ImageDelete::encode`]) telling the client
    /// which stored image(s)/placement(s) to drop.
    DeleteImage(Vec<u8>),
    /// A completed OSC 5522 clipboard write. The daemon strips the control
    /// sequence from replay; the GUI decides whether it may touch the clipboard.
    ClipboardWrite(Vec<u8>),
    Cwd(PathBuf),
    Prompt {
        active: bool,
        at_prompt: bool,
        last_exit: Option<i32>,
    },
    Exited {
        code: Option<i32>,
    },
    PaneList(Vec<PaneInfo>),
    InputAck {
        pane_id: u64,
    },
    RemoteContext(Option<RemoteContext>),
    Agent(Option<crate::core::cli_agent::CLIAgent>),
    AgentStatus(Option<crate::core::cli_agent::AgentSessionState>),
    LoopbackForward(LoopbackForward),
    Procs(PaneProcs),
    Version(DaemonVersion),
    Error(String),
}

mod kind {
    pub const SPAWN: u8 = 1;
    pub const ATTACH: u8 = 2;
    pub const INPUT: u8 = 3;
    pub const RESIZE: u8 = 4;
    pub const DETACH: u8 = 5;
    pub const KILL: u8 = 6;
    pub const LIST: u8 = 7;
    pub const SHUTDOWN: u8 = 8;
    pub const SPAWN_SHELL: u8 = 9;
    pub const ENSURE_LOOPBACK_FORWARD: u8 = 10;
    pub const VERSION: u8 = 40;
    pub const QUERY_PROCS: u8 = 50;
    pub const SPAWN_OWNED: u8 = 53;
    pub const OBSERVE: u8 = 54;
    pub const SEND_INPUT: u8 = 55;
    pub const HANDOFF: u8 = 56;

    pub const SPAWNED: u8 = 1;
    pub const SNAPSHOT: u8 = 2;
    pub const OUTPUT: u8 = 3;
    pub const CWD: u8 = 4;
    pub const PROMPT: u8 = 5;
    pub const EXITED: u8 = 6;
    pub const PANE_LIST: u8 = 7;
    pub const ERROR: u8 = 8;
    pub const SIZE: u8 = 9;
    pub const REMOTE_CONTEXT: u8 = 10;
    pub const LOOPBACK_FORWARD: u8 = 11;
    pub const AGENT: u8 = 21;
    pub const AGENT_STATUS: u8 = 22;
    pub const VERSION_REPLY: u8 = 40;
    pub const PROCS: u8 = 50;
    pub const INPUT_ACK: u8 = 51;
    /// `Image` — a kitty graphics frame lifted out of the PTY stream (issue
    /// #213). 60 sits clear of every range above; the payload is the compact
    /// binary encoding, not JSON, so it stays outside the `to_json` arms.
    pub const IMAGE: u8 = 60;
    /// `DeleteImage` — a kitty graphics `a=d` delete lifted out of the stream
    /// (issue #213). Compact binary selector, like `IMAGE`.
    pub const DELETE_IMAGE: u8 = 61;
    pub const CLIPBOARD_WRITE: u8 = 62;
}

pub fn write_frame<W: Write>(w: &mut W, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = payload.len();
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame payload exceeds MAX_FRAME",
        ));
    }
    // One write, not three. The pane socket is a loopback `TcpStream` with
    // `TCP_NODELAY` set, so three `write_all`s put the length, the kind and the
    // payload on the wire as three separate segments, and the reader on the far
    // side wakes from `read()` three times for one frame. Under a PTY flood the
    // daemon frames every ConPTY read, so that is two extra syscalls on each
    // side per frame, tens of thousands a second (issue #713).
    let mut header = [0u8; HEADER];
    header[..4].copy_from_slice(&(len as u32).to_le_bytes());
    header[4] = kind;
    let mut bufs = [io::IoSlice::new(&header), io::IoSlice::new(payload)];
    let mut rest: &mut [io::IoSlice<'_>] = &mut bufs;
    while !rest.is_empty() {
        match w.write_vectored(rest) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "the frame could not be written in full",
                ));
            }
            // A writer that does not implement `write_vectored` natively falls
            // back to writing the first non-empty slice, so this loop still
            // terminates — it just costs the two writes it used to cost.
            Ok(n) => io::IoSlice::advance_slices(&mut rest, n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame payload exceeds MAX_FRAME",
        ));
    }
    let mut kind = [0u8; 1];
    r.read_exact(&mut kind)?;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((kind[0], payload))
}

pub fn peek_frame_kind(buf: &[u8]) -> Option<u8> {
    (buf.len() >= 5).then(|| buf[4])
}

pub fn is_error_kind(kind: u8) -> bool {
    kind == kind::ERROR
}

pub fn take_frame(buf: &mut Vec<u8>) -> io::Result<Option<(u8, Vec<u8>)>> {
    if buf.len() < HEADER {
        return Ok(None);
    }
    let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame payload exceeds MAX_FRAME",
        ));
    }
    if buf.len() < HEADER + len {
        return Ok(None);
    }
    let kind = buf[4];
    let payload = buf[HEADER..HEADER + len].to_vec();
    buf.drain(..HEADER + len);
    Ok(Some((kind, payload)))
}

fn to_json<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn from_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OwnedSpawn {
    #[serde(default)]
    cwd: Option<PathBuf>,
    size: WinSize,
    #[serde(default)]
    shell: Option<ShellSpec>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    restore: Option<RestoreFrom>,
    #[serde(default)]
    allow_remote_clipboard_write: bool,
}

/// "This pane replaces one that died with the daemon."
///
/// Carried on a spawn rather than an attach because there is nothing to attach
/// to: the process is gone. The daemon looks up what pane `pane_id` last had on
/// its screen and seeds the new pane's ring with it, so the window shows the
/// output it lost under a shell that is plainly new.
///
/// `banner` is the line drawn between the two, and it comes from the client
/// because the daemon has no locale — it serves a GUI that might be running in
/// any language, and a CLI whose output is always English. A client that has
/// nothing to say can leave it out; the reset sequence is emitted either way.
///
/// Old daemons decode this frame without the field and simply spawn a blank
/// pane, which is what they did before it existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreFrom {
    pub pane_id: u64,
    #[serde(default)]
    pub banner: Option<String>,
}

impl ClientMsg {
    pub fn encode<W: Write>(&self, w: &mut W) -> io::Result<()> {
        match self {
            ClientMsg::Spawn {
                cwd,
                size,
                shell: None,
                owner: None,
                workspace: None,
                restore: None,
                allow_remote_clipboard_write: false,
            } => write_frame(w, kind::SPAWN, &to_json(&(cwd, size))?),
            ClientMsg::Spawn {
                cwd,
                size,
                shell: shell @ Some(_),
                owner: None,
                workspace: None,
                restore: None,
                allow_remote_clipboard_write: false,
            } => write_frame(w, kind::SPAWN_SHELL, &to_json(&(cwd, size, shell))?),
            ClientMsg::Spawn {
                cwd,
                size,
                shell,
                owner,
                workspace,
                restore,
                allow_remote_clipboard_write,
            } => write_frame(
                w,
                kind::SPAWN_OWNED,
                &to_json(&OwnedSpawn {
                    cwd: cwd.clone(),
                    size: *size,
                    shell: shell.clone(),
                    owner: owner.clone(),
                    workspace: workspace.clone(),
                    restore: restore.clone(),
                    allow_remote_clipboard_write: *allow_remote_clipboard_write,
                })?,
            ),
            ClientMsg::Attach {
                pane_id,
                size,
                allow_remote_clipboard_write,
            } => write_frame(
                w,
                kind::ATTACH,
                &to_json(&(pane_id, size, allow_remote_clipboard_write))?,
            ),
            ClientMsg::Observe { pane_id, size } => {
                write_frame(w, kind::OBSERVE, &to_json(&(pane_id, size))?)
            }
            ClientMsg::Input(bytes) => write_frame(w, kind::INPUT, bytes),
            ClientMsg::SendInput { pane_id, bytes } => {
                write_frame(w, kind::SEND_INPUT, &to_json(&(pane_id, bytes))?)
            }
            ClientMsg::Resize(size) => write_frame(w, kind::RESIZE, &to_json(size)?),
            ClientMsg::Detach => write_frame(w, kind::DETACH, &[]),
            ClientMsg::Kill { pane_id } => write_frame(w, kind::KILL, &to_json(pane_id)?),
            ClientMsg::List => write_frame(w, kind::LIST, &[]),
            ClientMsg::Shutdown => write_frame(w, kind::SHUTDOWN, &[]),
            ClientMsg::Handoff { exe } => write_frame(w, kind::HANDOFF, &to_json(exe)?),
            ClientMsg::EnsureLoopbackForward(req) => {
                write_frame(w, kind::ENSURE_LOOPBACK_FORWARD, &to_json(req)?)
            }
            ClientMsg::QueryProcs { pane_id } => {
                write_frame(w, kind::QUERY_PROCS, &to_json(pane_id)?)
            }
            ClientMsg::Version => write_frame(w, kind::VERSION, &[]),
        }
    }

    pub fn from_frame(k: u8, payload: Vec<u8>) -> io::Result<Self> {
        Ok(match k {
            kind::SPAWN => {
                let (cwd, size) = from_json(&payload)?;
                ClientMsg::Spawn {
                    cwd,
                    size,
                    shell: None,
                    owner: None,
                    workspace: None,
                    restore: None,
                    allow_remote_clipboard_write: false,
                }
            }
            kind::SPAWN_SHELL => {
                let (cwd, size, shell) = from_json(&payload)?;
                ClientMsg::Spawn {
                    cwd,
                    size,
                    shell,
                    owner: None,
                    workspace: None,
                    restore: None,
                    allow_remote_clipboard_write: false,
                }
            }
            kind::SPAWN_OWNED => {
                let OwnedSpawn {
                    cwd,
                    size,
                    shell,
                    owner,
                    workspace,
                    restore,
                    allow_remote_clipboard_write,
                } = from_json(&payload)?;
                ClientMsg::Spawn {
                    cwd,
                    size,
                    shell,
                    owner,
                    workspace,
                    restore,
                    allow_remote_clipboard_write,
                }
            }
            kind::ATTACH => {
                let (pane_id, size, allow_remote_clipboard_write) = from_json(&payload)?;
                ClientMsg::Attach {
                    pane_id,
                    size,
                    allow_remote_clipboard_write,
                }
            }
            kind::OBSERVE => {
                let (pane_id, size) = from_json(&payload)?;
                ClientMsg::Observe { pane_id, size }
            }
            kind::INPUT => ClientMsg::Input(payload),
            kind::SEND_INPUT => {
                let (pane_id, bytes) = from_json(&payload)?;
                ClientMsg::SendInput { pane_id, bytes }
            }
            kind::RESIZE => ClientMsg::Resize(from_json(&payload)?),
            kind::DETACH => ClientMsg::Detach,
            kind::KILL => ClientMsg::Kill {
                pane_id: from_json(&payload)?,
            },
            kind::LIST => ClientMsg::List,
            kind::SHUTDOWN => ClientMsg::Shutdown,
            kind::HANDOFF => ClientMsg::Handoff {
                exe: from_json(&payload)?,
            },
            kind::ENSURE_LOOPBACK_FORWARD => ClientMsg::EnsureLoopbackForward(from_json(&payload)?),
            kind::QUERY_PROCS => ClientMsg::QueryProcs {
                pane_id: from_json(&payload)?,
            },
            kind::VERSION => ClientMsg::Version,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown ClientMsg kind {other}"),
                ));
            }
        })
    }

    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let (k, payload) = read_frame(r)?;
        Self::from_frame(k, payload)
    }
}

impl DaemonMsg {
    pub fn encode<W: Write>(&self, w: &mut W) -> io::Result<()> {
        match self {
            DaemonMsg::Spawned { pane_id } => write_frame(w, kind::SPAWNED, &to_json(pane_id)?),
            DaemonMsg::Size(size) => write_frame(w, kind::SIZE, &to_json(size)?),
            DaemonMsg::Snapshot(bytes) => write_frame(w, kind::SNAPSHOT, bytes),
            DaemonMsg::Output(bytes) => write_frame(w, kind::OUTPUT, bytes),
            DaemonMsg::Image(frame) => write_frame(w, kind::IMAGE, frame),
            DaemonMsg::DeleteImage(sel) => write_frame(w, kind::DELETE_IMAGE, sel),
            DaemonMsg::ClipboardWrite(frame) => write_frame(w, kind::CLIPBOARD_WRITE, frame),
            DaemonMsg::Cwd(path) => write_frame(w, kind::CWD, &to_json(path)?),
            DaemonMsg::Prompt {
                active,
                at_prompt,
                last_exit,
            } => write_frame(w, kind::PROMPT, &to_json(&(active, at_prompt, last_exit))?),
            DaemonMsg::Exited { code } => write_frame(w, kind::EXITED, &to_json(code)?),
            DaemonMsg::PaneList(list) => write_frame(w, kind::PANE_LIST, &to_json(list)?),
            DaemonMsg::InputAck { pane_id } => write_frame(w, kind::INPUT_ACK, &to_json(pane_id)?),
            DaemonMsg::RemoteContext(remote) => {
                write_frame(w, kind::REMOTE_CONTEXT, &to_json(remote)?)
            }
            DaemonMsg::Agent(agent) => write_frame(w, kind::AGENT, &to_json(agent)?),
            DaemonMsg::AgentStatus(state) => write_frame(w, kind::AGENT_STATUS, &to_json(state)?),
            DaemonMsg::LoopbackForward(forward) => {
                write_frame(w, kind::LOOPBACK_FORWARD, &to_json(forward)?)
            }
            DaemonMsg::Procs(procs) => write_frame(w, kind::PROCS, &to_json(procs)?),
            DaemonMsg::Version(version) => write_frame(w, kind::VERSION_REPLY, &to_json(version)?),
            DaemonMsg::Error(msg) => write_frame(w, kind::ERROR, &to_json(msg)?),
        }
    }

    pub fn from_frame(k: u8, payload: Vec<u8>) -> io::Result<Self> {
        Ok(match k {
            kind::SPAWNED => DaemonMsg::Spawned {
                pane_id: from_json(&payload)?,
            },
            kind::SIZE => DaemonMsg::Size(from_json(&payload)?),
            kind::SNAPSHOT => DaemonMsg::Snapshot(payload),
            kind::OUTPUT => DaemonMsg::Output(payload),
            kind::IMAGE => DaemonMsg::Image(payload),
            kind::DELETE_IMAGE => DaemonMsg::DeleteImage(payload),
            kind::CLIPBOARD_WRITE => DaemonMsg::ClipboardWrite(payload),
            kind::CWD => DaemonMsg::Cwd(from_json(&payload)?),
            kind::PROMPT => {
                let (active, at_prompt, last_exit) = from_json(&payload)?;
                DaemonMsg::Prompt {
                    active,
                    at_prompt,
                    last_exit,
                }
            }
            kind::EXITED => DaemonMsg::Exited {
                code: from_json(&payload)?,
            },
            kind::PANE_LIST => DaemonMsg::PaneList(from_json(&payload)?),
            kind::INPUT_ACK => DaemonMsg::InputAck {
                pane_id: from_json(&payload)?,
            },
            kind::REMOTE_CONTEXT => DaemonMsg::RemoteContext(from_json(&payload)?),
            kind::AGENT => DaemonMsg::Agent(from_json(&payload)?),
            kind::AGENT_STATUS => DaemonMsg::AgentStatus(from_json(&payload)?),
            kind::LOOPBACK_FORWARD => DaemonMsg::LoopbackForward(from_json(&payload)?),
            kind::PROCS => DaemonMsg::Procs(from_json(&payload)?),
            kind::VERSION_REPLY => DaemonMsg::Version(from_json(&payload)?),
            kind::ERROR => DaemonMsg::Error(from_json(&payload)?),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown DaemonMsg kind {other}"),
                ));
            }
        })
    }

    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let (k, payload) = read_frame(r)?;
        Self::from_frame(k, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: WinSize = WinSize {
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 17,
    };

    #[test]
    fn full_session_round_trips_over_a_real_duplex_stream() {
        use std::io::Write;
        use std::net::{TcpListener, TcpStream};
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let client_msgs = vec![
            ClientMsg::Spawn {
                cwd: Some(PathBuf::from("/work")),
                size: SIZE,
                shell: None,
                owner: None,
                workspace: None,
                restore: None,
                allow_remote_clipboard_write: false,
            },
            ClientMsg::Resize(SIZE),
            ClientMsg::Input(vec![b'l', b's', b'\r']),
            ClientMsg::Detach,
        ];
        let daemon_msgs = vec![
            DaemonMsg::Spawned { pane_id: 9 },
            DaemonMsg::Snapshot(vec![0x1b, b'[', b'2', b'J']),
            DaemonMsg::Output(b"hello\r\n".to_vec()),
            DaemonMsg::ClipboardWrite(vec![1, 2, 3, 4]),
            DaemonMsg::Prompt {
                active: true,
                at_prompt: true,
                last_exit: Some(0),
            },
            DaemonMsg::Exited { code: Some(0) },
        ];

        let expect_from_client = client_msgs.clone();
        let reply_with = daemon_msgs.clone();
        let daemon = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let got: Vec<ClientMsg> = (0..expect_from_client.len())
                .map(|_| ClientMsg::read(&mut sock).unwrap())
                .collect();
            for m in &reply_with {
                m.encode(&mut sock).unwrap();
            }
            sock.flush().unwrap();
            got
        });

        let mut sock = TcpStream::connect(addr).unwrap();
        for m in &client_msgs {
            m.encode(&mut sock).unwrap();
        }
        sock.flush().unwrap();
        let got_from_daemon: Vec<DaemonMsg> = (0..daemon_msgs.len())
            .map(|_| DaemonMsg::read(&mut sock).unwrap())
            .collect();

        let got_from_client = daemon.join().unwrap();
        assert_eq!(got_from_client, client_msgs, "daemon decoded client stream");
        assert_eq!(got_from_daemon, daemon_msgs, "client decoded daemon stream");
    }

    #[test]
    fn default_spawn_stays_wire_compatible_with_old_daemons() {
        let msg = ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/work")),
            size: SIZE,
            shell: None,
            owner: None,
            workspace: None,
            restore: None,
            allow_remote_clipboard_write: false,
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(k, kind::SPAWN, "default spawn must use the legacy kind");
        let (cwd, size): (Option<PathBuf>, WinSize) = serde_json::from_slice(&payload).unwrap();
        assert_eq!(cwd, Some(PathBuf::from("/work")));
        assert_eq!(size, SIZE);

        let legacy = serde_json::to_vec(&(Some(PathBuf::from("/old")), SIZE)).unwrap();
        let decoded = ClientMsg::from_frame(kind::SPAWN, legacy).unwrap();
        assert_eq!(
            decoded,
            ClientMsg::Spawn {
                cwd: Some(PathBuf::from("/old")),
                size: SIZE,
                shell: None,
                owner: None,
                workspace: None,
                restore: None,
                allow_remote_clipboard_write: false,
            }
        );
    }

    #[test]
    fn explicit_shell_spawn_uses_shell_kind() {
        let shell = ShellSpec {
            program: "fish".to_string(),
            args: vec!["-l".to_string()],
            args_are_tty7_defaults: true,
        };
        let msg = ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/work")),
            size: SIZE,
            shell: Some(shell.clone()),
            owner: None,
            workspace: None,
            restore: None,
            allow_remote_clipboard_write: false,
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(k, kind::SPAWN_SHELL);
        let decoded = ClientMsg::from_frame(k, payload).unwrap();
        assert_eq!(
            decoded,
            ClientMsg::Spawn {
                cwd: Some(PathBuf::from("/work")),
                size: SIZE,
                shell: Some(shell),
                owner: None,
                workspace: None,
                restore: None,
                allow_remote_clipboard_write: false,
            }
        );
    }

    #[test]
    fn owned_spawn_uses_the_owned_kind_and_round_trips() {
        let msg = ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/work")),
            size: SIZE,
            shell: Some(ShellSpec {
                program: "fish".into(),
                args: vec!["-l".into()],
                args_are_tty7_defaults: false,
            }),
            owner: Some("bda10e44-02de-44a0-8412-ec1cda2b5f5b".into()),
            workspace: Some("ws-7".into()),
            restore: None,
            allow_remote_clipboard_write: true,
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(k, kind::SPAWN_OWNED);
        assert_eq!(ClientMsg::from_frame(k, payload).unwrap(), msg);
    }

    #[test]
    fn owned_spawn_payload_tolerates_unknown_and_missing_fields() {
        let payload = serde_json::to_vec(&serde_json::json!({
            "size": {"cols": 80, "rows": 24, "cell_w": 8, "cell_h": 17},
            "some_future_field": true,
        }))
        .unwrap();
        let decoded = ClientMsg::from_frame(kind::SPAWN_OWNED, payload).unwrap();
        assert_eq!(
            decoded,
            ClientMsg::Spawn {
                cwd: None,
                size: WinSize {
                    cols: 80,
                    rows: 24,
                    cell_w: 8,
                    cell_h: 17
                },
                shell: None,
                owner: None,
                workspace: None,
                restore: None,
                allow_remote_clipboard_write: false,
            }
        );
    }

    #[test]
    fn a_workspace_spawn_uses_the_owned_kind_and_round_trips() {
        let msg = ClientMsg::Spawn {
            cwd: None,
            size: SIZE,
            shell: None,
            owner: None,
            workspace: Some("ws-main".into()),
            restore: None,
            allow_remote_clipboard_write: true,
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(
            k,
            kind::SPAWN_OWNED,
            "a workspace-tagged spawn must not ride the legacy kinds, which drop the field"
        );
        assert_eq!(ClientMsg::from_frame(k, payload).unwrap(), msg);
    }

    #[test]
    fn observe_uses_its_own_kind_and_round_trips() {
        let msg = ClientMsg::Observe {
            pane_id: 42,
            size: SIZE,
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(k, kind::OBSERVE);
        assert_ne!(k, kind::ATTACH, "observing must never preempt an attach");
        assert_eq!(ClientMsg::from_frame(k, payload).unwrap(), msg);
    }

    #[test]
    fn pane_info_owner_defaults_for_old_daemons() {
        let old = serde_json::json!({"pane_id": 3, "title": "zsh", "alive": true});
        let info: PaneInfo = serde_json::from_value(old).unwrap();
        assert_eq!(info.owner, None);
        assert!(info.alive);
    }

    #[test]
    fn frame_edges() {
        let mut buf = Vec::new();
        write_frame(&mut buf, 3, &[]).unwrap();
        let mut cursor = std::io::Cursor::new(&buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), (3, vec![]));

        let mut bad = Vec::new();
        bad.extend_from_slice(&(u32::MAX).to_le_bytes());
        bad.push(3);
        let mut cursor = std::io::Cursor::new(&bad);
        assert!(read_frame(&mut cursor).is_err());
    }

    #[test]
    fn write_frame_rejects_oversize_payload() {
        let oversize = vec![0u8; MAX_FRAME + 1];
        let mut buf = Vec::new();
        assert!(write_frame(&mut buf, 3, &oversize).is_err());
        assert!(buf.is_empty());
    }

    /// The pane socket has `TCP_NODELAY` set, so a write is a segment and a
    /// segment is a wakeup on the far side. A frame must therefore cost one
    /// write, not one for the length, one for the kind and one for the payload
    /// (issue #713) — at flood rates that difference is tens of thousands of
    /// syscalls a second on each end.
    #[test]
    fn a_frame_is_one_write_on_a_vectored_writer() {
        #[derive(Default)]
        struct Counting {
            writes: usize,
            bytes: Vec<u8>,
        }
        impl Write for Counting {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.writes += 1;
                self.bytes.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
                self.writes += 1;
                let mut n = 0;
                for b in bufs {
                    self.bytes.extend_from_slice(b);
                    n += b.len();
                }
                Ok(n)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut w = Counting::default();
        write_frame(&mut w, kind::OUTPUT, b"a chunk of pty output").expect("write the frame");
        assert_eq!(w.writes, 1, "one frame must cost one write");

        // An empty payload is a frame too — the header still has to land, and
        // the empty second slice must not spin the loop.
        let mut empty = Counting::default();
        write_frame(&mut empty, kind::DETACH, &[]).expect("write the empty frame");
        assert_eq!(empty.writes, 1);

        // Whatever the write count, the bytes on the wire are unchanged: a
        // `read_frame` over them gives back exactly what went in.
        let mut cursor = io::Cursor::new(w.bytes);
        assert_eq!(
            read_frame(&mut cursor).expect("read it back"),
            (kind::OUTPUT, b"a chunk of pty output".to_vec())
        );
    }

    #[test]
    fn from_frame_rejects_unknown_kind() {
        assert!(ClientMsg::from_frame(99, vec![]).is_err());
        assert!(DaemonMsg::from_frame(99, vec![]).is_err());
    }

    #[test]
    fn take_frame_is_resumable_and_mirrors_read_frame() {
        let mut wire = Vec::new();
        write_frame(&mut wire, 3, b"hello").unwrap();
        write_frame(&mut wire, 9, &[]).unwrap();

        let mut buf = Vec::new();
        let mut got = Vec::new();
        for &b in &wire {
            buf.push(b);
            while let Some(frame) = take_frame(&mut buf).unwrap() {
                got.push(frame);
            }
        }
        assert_eq!(got, vec![(3, b"hello".to_vec()), (9, vec![])]);
        assert!(buf.is_empty(), "nothing left over after both frames");

        let mut buf = Vec::new();
        write_frame(&mut buf, 3, b"done").unwrap();
        buf.extend_from_slice(&10u32.to_le_bytes());
        assert_eq!(take_frame(&mut buf).unwrap(), Some((3, b"done".to_vec())));
        assert_eq!(take_frame(&mut buf).unwrap(), None);
        assert_eq!(buf, 10u32.to_le_bytes());

        let mut bad = (u32::MAX).to_le_bytes().to_vec();
        bad.push(3);
        assert!(take_frame(&mut bad).is_err());
    }

    #[test]
    fn read_frame_on_truncated_frame_is_an_error() {
        let mut cut = std::io::Cursor::new(5u32.to_le_bytes().to_vec());
        assert_eq!(
            read_frame(&mut cut).unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );

        let mut buf = Vec::new();
        buf.extend_from_slice(&10u32.to_le_bytes());
        buf.push(3);
        buf.extend_from_slice(b"only4");
        let mut cut = std::io::Cursor::new(buf);
        assert_eq!(
            read_frame(&mut cut).unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }
}
