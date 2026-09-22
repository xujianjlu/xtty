//! In-pane ZMODEM (`rz` / `sz`) support.
//!
//! Remote `sz` advertises with ZRQINIT → we receive into `~/Downloads`.
//! Remote `rz` advertises with ZRINIT → we open a native file picker and send.
//!
//! The reader thread peels the handshake out of the VT stream so binary frames
//! do not paint as garbage; the UI drives [`zmodem2`] and writes replies back
//! through the pane's normal input path.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use zmodem2::{Receiver, ReceiverEvent, Sender, SenderEvent};

/// Hex ZRQINIT / ZRINIT open with `**\x18B` then two hex digits for the frame.
const HEX_PREFIX: &[u8] = &[b'*', b'*', 0x18, b'B'];
/// Binary headers open with `*\x18` then encoding then frame type.
const BIN_PREFIX_ZBIN: &[u8] = &[b'*', 0x18, 0x41];
const BIN_PREFIX_ZBIN32: &[u8] = &[b'*', 0x18, 0x43];

const CAN: u8 = 0x18;
const MAX_INBOUND: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZmodemRole {
    /// Remote is sending (`sz`) — we receive.
    Receive = 1,
    /// Remote is receiving (`rz`) — we send after a file picker.
    Send = 2,
}

impl ZmodemRole {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Receive),
            2 => Some(Self::Send),
            _ => None,
        }
    }
}

/// Shared with the reader thread: divert flag, pending role, inbound wire bytes.
#[derive(Default)]
pub(crate) struct ZmodemPipe {
    divert: AtomicBool,
    pending_role: AtomicU8,
    inbound: Mutex<Vec<u8>>,
    /// Undiverted tail kept so a header split across reads still matches.
    scan_tail: Mutex<Vec<u8>>,
}

impl ZmodemPipe {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn is_diverting(&self) -> bool {
        self.divert.load(Ordering::Relaxed)
    }

    pub(crate) fn begin(&self, role: ZmodemRole) {
        self.pending_role.store(role as u8, Ordering::Release);
        self.divert.store(true, Ordering::Release);
        if let Ok(mut tail) = self.scan_tail.lock() {
            tail.clear();
        }
    }

    pub(crate) fn end(&self) {
        self.divert.store(false, Ordering::Release);
        self.pending_role.store(0, Ordering::Release);
        if let Ok(mut inbound) = self.inbound.lock() {
            inbound.clear();
        }
        if let Ok(mut tail) = self.scan_tail.lock() {
            tail.clear();
        }
    }

    pub(crate) fn take_pending_role(&self) -> Option<ZmodemRole> {
        let raw = self.pending_role.swap(0, Ordering::AcqRel);
        ZmodemRole::from_u8(raw)
    }

    pub(crate) fn push_inbound(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let Ok(mut inbound) = self.inbound.lock() else {
            return;
        };
        if inbound.len() + bytes.len() > MAX_INBOUND {
            let keep = MAX_INBOUND.saturating_sub(bytes.len().min(MAX_INBOUND));
            let excess = inbound.len().saturating_sub(keep);
            if excess > 0 {
                inbound.drain(..excess);
            }
        }
        inbound.extend_from_slice(bytes);
    }

    pub(crate) fn take_inbound(&self) -> Vec<u8> {
        self.inbound
            .lock()
            .map(|mut inbound| std::mem::take(&mut *inbound))
            .unwrap_or_default()
    }

    /// Feed live output. Returns bytes that should still reach the VT parser
    /// (prefix before a newly detected handshake). Empty when fully diverted.
    pub(crate) fn filter_output(&self, bytes: &[u8]) -> Vec<u8> {
        if bytes.is_empty() {
            return Vec::new();
        }
        if self.is_diverting() {
            self.push_inbound(bytes);
            return Vec::new();
        }

        let (tail_len, combined) = self
            .scan_tail
            .lock()
            .map(|mut tail| {
                let tail_len = tail.len();
                let mut v = std::mem::take(&mut *tail);
                v.extend_from_slice(bytes);
                (tail_len, v)
            })
            .unwrap_or_else(|_| (0, bytes.to_vec()));

        if let Some((at, role)) = find_zmodem_start(&combined) {
            // Only the still-undisplayed prefix may reach the VT parser. Bytes
            // already returned via `scan_tail` must not paint a second time.
            let display = if at > tail_len {
                combined[tail_len..at].to_vec()
            } else {
                Vec::new()
            };
            let rest = combined[at..].to_vec();
            self.begin(role);
            self.push_inbound(&rest);
            return display;
        }

        // Keep a short undiverted tail for a header that spans the next read.
        const KEEP: usize = 8;
        if let Ok(mut tail) = self.scan_tail.lock() {
            if combined.len() > KEEP {
                tail.extend_from_slice(&combined[combined.len() - KEEP..]);
            } else {
                *tail = combined;
            }
        }
        bytes.to_vec()
    }
}

/// Locate a ZMODEM session-start header. Returns byte offset and local role.
pub(crate) fn find_zmodem_start(bytes: &[u8]) -> Option<(usize, ZmodemRole)> {
    if bytes.len() < 4 {
        return None;
    }
    for i in 0..bytes.len() {
        if let Some(role) = role_at(bytes, i) {
            return Some((i, role));
        }
    }
    None
}

fn role_at(bytes: &[u8], i: usize) -> Option<ZmodemRole> {
    if bytes.len().saturating_sub(i) >= 6 && bytes[i..].starts_with(HEX_PREFIX) {
        let a = bytes[i + 4];
        let b = bytes[i + 5];
        if a == b'0' && b == b'0' {
            return Some(ZmodemRole::Receive);
        }
        if a == b'0' && b == b'1' {
            return Some(ZmodemRole::Send);
        }
    }
    if bytes.len().saturating_sub(i) >= 4 {
        let frame = bytes[i + 3];
        if bytes[i..].starts_with(BIN_PREFIX_ZBIN) || bytes[i..].starts_with(BIN_PREFIX_ZBIN32) {
            match frame {
                0 => return Some(ZmodemRole::Receive),
                1 => return Some(ZmodemRole::Send),
                _ => {}
            }
        }
    }
    None
}

pub(crate) fn cancel_sequence() -> Vec<u8> {
    vec![CAN; 8]
}

fn download_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
}

fn sanitize_remote_name(raw: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(raw);
    let name = Path::new(lossy.as_ref())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() || name == "." || name == ".." {
        "zmodem-file".into()
    } else {
        name.replace(['/', '\\', '\0'], "_")
    }
}

fn free_download_path(name: &str) -> Option<PathBuf> {
    let dir = download_dir();
    let _ = std::fs::create_dir_all(&dir);
    let first = dir.join(name);
    if !first.exists() {
        return Some(first);
    }
    let path = Path::new(name);
    let ext = path.extension().and_then(|e| e.to_str());
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    (2..1000u32).find_map(|n| {
        let candidate = match ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        };
        if candidate.exists() {
            None
        } else {
            Some(candidate)
        }
    })
}

struct SendFile {
    name: Vec<u8>,
    size: u32,
    file: File,
}

enum SessionKind {
    Receiving {
        receiver: Receiver,
        file: Option<File>,
        path: Option<PathBuf>,
        last_saved: Option<PathBuf>,
    },
    Sending {
        sender: Sender,
        queue: VecDeque<SendFile>,
        active: Option<SendFile>,
    },
    /// `rz` detected; native picker is open; wire bytes buffer here.
    AwaitingPicker { buffered: Vec<u8> },
}

pub(crate) struct ZmodemSession {
    kind: SessionKind,
}

pub(crate) enum ZmodemUiAction {
    /// Transfer finished successfully.
    Done { detail: String },
    /// Transfer failed or was cancelled.
    Failed { detail: String },
}

impl ZmodemSession {
    pub(crate) fn start_receive() -> Result<Self, String> {
        let receiver = Receiver::new().map_err(|e| format!("zmodem receiver: {e:?}"))?;
        Ok(Self {
            kind: SessionKind::Receiving {
                receiver,
                file: None,
                path: None,
                last_saved: None,
            },
        })
    }

    pub(crate) fn start_awaiting_picker(initial: Vec<u8>) -> Self {
        Self {
            kind: SessionKind::AwaitingPicker { buffered: initial },
        }
    }

    pub(crate) fn is_awaiting_picker(&self) -> bool {
        matches!(self.kind, SessionKind::AwaitingPicker { .. })
    }

    pub(crate) fn buffer_while_awaiting(&mut self, bytes: &[u8]) {
        if let SessionKind::AwaitingPicker { buffered } = &mut self.kind {
            buffered.extend_from_slice(bytes);
            if buffered.len() > MAX_INBOUND {
                let drain = buffered.len() - MAX_INBOUND;
                buffered.drain(..drain);
            }
        }
    }

    pub(crate) fn begin_send_with_paths(&mut self, paths: Vec<PathBuf>) -> Result<Vec<u8>, String> {
        let SessionKind::AwaitingPicker { buffered } = &self.kind else {
            return Err("zmodem send is not waiting for files".into());
        };
        let buffered = buffered.clone();
        if paths.is_empty() {
            return Err("no files selected".into());
        }

        let mut queue = VecDeque::new();
        for path in paths {
            let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if !meta.is_file() {
                continue;
            }
            let size = u32::try_from(meta.len())
                .map_err(|_| format!("{} is larger than 4 GiB", path.display()))?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            queue.push_back(SendFile {
                name: name.into_bytes(),
                size,
                file,
            });
        }
        if queue.is_empty() {
            return Err("no regular files selected".into());
        }

        let mut sender = Sender::new().map_err(|e| format!("zmodem sender: {e:?}"))?;
        let mut wire = sender.drain_outgoing().to_vec();
        sender.advance_outgoing(wire.len());

        // Feed any ZRINIT (and following) that arrived while the picker was open.
        let mut rest = buffered.as_slice();
        while !rest.is_empty() {
            let n = sender
                .feed_incoming(rest)
                .map_err(|e| format!("zmodem feed: {e:?}"))?;
            if n == 0 {
                break;
            }
            rest = &rest[n..];
            let out = sender.drain_outgoing().to_vec();
            sender.advance_outgoing(out.len());
            wire.extend_from_slice(&out);
        }

        let first = queue.pop_front().expect("non-empty");
        sender
            .start_file(&first.name, first.size)
            .map_err(|e| format!("zmodem start_file: {e:?}"))?;
        let out = sender.drain_outgoing().to_vec();
        sender.advance_outgoing(out.len());
        wire.extend_from_slice(&out);

        self.kind = SessionKind::Sending {
            sender,
            queue,
            active: Some(first),
        };
        Ok(wire)
    }

    /// Drive the state machine with newly diverted wire bytes. Returns bytes
    /// to write back to the PTY, plus an optional UI action.
    pub(crate) fn pump(
        &mut self,
        inbound: &[u8],
    ) -> Result<(Vec<u8>, Option<ZmodemUiAction>), String> {
        match &mut self.kind {
            SessionKind::AwaitingPicker { buffered } => {
                buffered.extend_from_slice(inbound);
                Ok((Vec::new(), None))
            }
            SessionKind::Receiving {
                receiver,
                file,
                path,
                last_saved,
            } => pump_receive(receiver, file, path, last_saved, inbound),
            SessionKind::Sending {
                sender,
                queue,
                active,
            } => pump_send(sender, queue, active, inbound),
        }
    }

    /// Initial outgoing after starting a receive session (ZRINIT).
    pub(crate) fn take_initial_outgoing(&mut self) -> Vec<u8> {
        match &mut self.kind {
            SessionKind::Receiving { receiver, .. } => {
                let out = receiver.drain_outgoing().to_vec();
                receiver.advance_outgoing(out.len());
                out
            }
            _ => Vec::new(),
        }
    }
}

fn pump_receive(
    receiver: &mut Receiver,
    file: &mut Option<File>,
    path: &mut Option<PathBuf>,
    last_saved: &mut Option<PathBuf>,
    inbound: &[u8],
) -> Result<(Vec<u8>, Option<ZmodemUiAction>), String> {
    let mut wire = Vec::new();
    let mut rest = inbound;
    loop {
        // Flush any pending file bytes first.
        let data = receiver.drain_file().to_vec();
        if !data.is_empty() {
            let f = file
                .as_mut()
                .ok_or_else(|| "zmodem file data before FileStart".to_string())?;
            f.write_all(&data)
                .map_err(|e| format!("write download: {e}"))?;
            receiver
                .advance_file(data.len())
                .map_err(|e| format!("zmodem advance_file: {e:?}"))?;
        }

        let out = receiver.drain_outgoing().to_vec();
        if !out.is_empty() {
            receiver.advance_outgoing(out.len());
            wire.extend_from_slice(&out);
            continue;
        }

        if let Some(ev) = receiver.poll_event() {
            match ev {
                ReceiverEvent::FileStart => {
                    let name = sanitize_remote_name(receiver.file_name());
                    let dest = free_download_path(&name)
                        .ok_or_else(|| format!("no free name in Downloads for {name}"))?;
                    let f = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&dest)
                        .map_err(|e| format!("{}: {e}", dest.display()))?;
                    *path = Some(dest.clone());
                    *file = Some(f);
                    *last_saved = Some(dest);
                }
                ReceiverEvent::FileComplete => {
                    if let Some(f) = file.take() {
                        let _ = f.sync_all();
                    }
                    path.take();
                }
                ReceiverEvent::SessionComplete => {
                    let detail = last_saved
                        .as_ref()
                        .map(|p| format!("saved {}", p.display()))
                        .unwrap_or_else(|| "transfer complete".into());
                    return Ok((wire, Some(ZmodemUiAction::Done { detail })));
                }
                ReceiverEvent::Aborted => {
                    return Ok((
                        wire,
                        Some(ZmodemUiAction::Failed {
                            detail: "remote aborted the transfer".into(),
                        }),
                    ));
                }
            }
            continue;
        }

        if rest.is_empty() {
            break;
        }
        let n = receiver
            .feed_incoming(rest)
            .map_err(|e| format!("zmodem feed: {e:?}"))?;
        if n == 0 {
            break;
        }
        rest = &rest[n..];
    }
    Ok((wire, None))
}

fn pump_send(
    sender: &mut Sender,
    queue: &mut VecDeque<SendFile>,
    active: &mut Option<SendFile>,
    inbound: &[u8],
) -> Result<(Vec<u8>, Option<ZmodemUiAction>), String> {
    let mut wire = Vec::new();
    let mut rest = inbound;
    loop {
        if let Some(req) = sender.poll_file() {
            let current = active
                .as_mut()
                .ok_or_else(|| "zmodem file request with no open file".to_string())?;
            current
                .file
                .seek(SeekFrom::Start(u64::from(req.offset)))
                .map_err(|e| format!("seek: {e}"))?;
            let mut buf = vec![0u8; req.len];
            let n = current
                .file
                .read(&mut buf)
                .map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                return Err("unexpected EOF while sending".into());
            }
            buf.truncate(n);
            sender
                .feed_file(&buf)
                .map_err(|e| format!("zmodem feed_file: {e:?}"))?;
            let out = sender.drain_outgoing().to_vec();
            sender.advance_outgoing(out.len());
            wire.extend_from_slice(&out);
            continue;
        }

        let out = sender.drain_outgoing().to_vec();
        if !out.is_empty() {
            sender.advance_outgoing(out.len());
            wire.extend_from_slice(&out);
            continue;
        }

        if let Some(ev) = sender.poll_event() {
            match ev {
                SenderEvent::FileComplete => {
                    *active = None;
                    if let Some(next) = queue.pop_front() {
                        sender
                            .start_file(&next.name, next.size)
                            .map_err(|e| format!("zmodem start_file: {e:?}"))?;
                        *active = Some(next);
                        let out = sender.drain_outgoing().to_vec();
                        sender.advance_outgoing(out.len());
                        wire.extend_from_slice(&out);
                    } else {
                        sender
                            .finish_session()
                            .map_err(|e| format!("zmodem finish: {e:?}"))?;
                        let out = sender.drain_outgoing().to_vec();
                        sender.advance_outgoing(out.len());
                        wire.extend_from_slice(&out);
                    }
                }
                SenderEvent::SessionComplete => {
                    return Ok((
                        wire,
                        Some(ZmodemUiAction::Done {
                            detail: "upload complete".into(),
                        }),
                    ));
                }
                SenderEvent::Aborted => {
                    return Ok((
                        wire,
                        Some(ZmodemUiAction::Failed {
                            detail: "remote aborted the transfer".into(),
                        }),
                    ));
                }
            }
            continue;
        }

        if rest.is_empty() {
            break;
        }
        let n = sender
            .feed_incoming(rest)
            .map_err(|e| format!("zmodem feed: {e:?}"))?;
        if n == 0 {
            break;
        }
        rest = &rest[n..];
    }
    Ok((wire, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_hex_zrqinit_as_receive() {
        let mut bytes = HEX_PREFIX.to_vec();
        bytes.extend_from_slice(b"00000000000000");
        let (at, role) = find_zmodem_start(&bytes).expect("detect");
        assert_eq!(at, 0);
        assert_eq!(role, ZmodemRole::Receive);
    }

    #[test]
    fn detects_hex_zrinit_as_send() {
        let mut bytes = b"noise".to_vec();
        bytes.extend_from_slice(HEX_PREFIX);
        bytes.extend_from_slice(b"01000000000000");
        let (at, role) = find_zmodem_start(&bytes).expect("detect");
        assert_eq!(at, 5);
        assert_eq!(role, ZmodemRole::Send);
    }

    #[test]
    fn detects_binary_zrqinit() {
        let bytes = [b'*', 0x18, 0x41, 0];
        let (at, role) = find_zmodem_start(&bytes).expect("detect");
        assert_eq!(at, 0);
        assert_eq!(role, ZmodemRole::Receive);
    }

    #[test]
    fn filter_output_diverts_from_handshake() {
        let pipe = ZmodemPipe::new();
        let mut frame = b"ok\r\n".to_vec();
        frame.extend_from_slice(HEX_PREFIX);
        frame.extend_from_slice(b"00deadbeefcafe");
        let display = pipe.filter_output(&frame);
        assert_eq!(display, b"ok\r\n");
        assert!(pipe.is_diverting());
        assert_eq!(pipe.take_pending_role(), Some(ZmodemRole::Receive));
        let inbound = pipe.take_inbound();
        assert!(inbound.starts_with(HEX_PREFIX));
    }

    #[test]
    fn sanitize_strips_path_components() {
        assert_eq!(sanitize_remote_name(b"/tmp/../x.bin"), "x.bin");
        assert_eq!(sanitize_remote_name(b""), "zmodem-file");
        assert_eq!(sanitize_remote_name(b".."), "zmodem-file");
    }
}
