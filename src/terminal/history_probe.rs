//! PTY history dump for nested `ssh` / jumper hops.
//!
//! Process-table nested SSH cannot SFTP the inner host's `~/.bash_history`.
//! When Ctrl+R needs a corpus, we inject a quiet `history`/`fc` dump into the
//! PTY, divert the reply (so it does not paint), and parse it into the pane's
//! active history scope.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Unique markers — unlikely in real commands; ASCII RS keeps them off most fonts.
pub(crate) const BEGIN_MARK: &[u8] = b"\x1eTTY7_HIST_BEGIN\x1e";
pub(crate) const END_MARK: &[u8] = b"\x1eTTY7_HIST_END\x1e";

const MAX_CAPTURE: usize = 512 * 1024;

/// Shared with the reader thread: divert PTY bytes until the end marker lands.
#[derive(Default)]
pub(crate) struct HistoryProbePipe {
    divert: AtomicBool,
    complete: AtomicBool,
    inbound: Mutex<Vec<u8>>,
}

impl HistoryProbePipe {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn is_diverting(&self) -> bool {
        self.divert.load(Ordering::Relaxed)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.divert.load(Ordering::Relaxed) || self.complete.load(Ordering::Relaxed)
    }

    /// Start diverting every subsequent PTY byte until [`END_MARK`].
    pub(crate) fn arm(&self) {
        self.complete.store(false, Ordering::Release);
        if let Ok(mut inbound) = self.inbound.lock() {
            inbound.clear();
        }
        self.divert.store(true, Ordering::Release);
    }

    pub(crate) fn cancel(&self) {
        self.divert.store(false, Ordering::Release);
        self.complete.store(false, Ordering::Release);
        if let Ok(mut inbound) = self.inbound.lock() {
            inbound.clear();
        }
    }

    /// Bytes that should still reach the VT parser. Empty while diverting.
    pub(crate) fn filter_output(&self, bytes: &[u8]) -> Vec<u8> {
        if bytes.is_empty() || !self.divert.load(Ordering::Acquire) {
            return bytes.to_vec();
        }
        let Ok(mut inbound) = self.inbound.lock() else {
            return Vec::new();
        };
        if inbound.len() + bytes.len() > MAX_CAPTURE {
            let keep = MAX_CAPTURE.saturating_sub(bytes.len().min(MAX_CAPTURE));
            let excess = inbound.len().saturating_sub(keep);
            if excess > 0 {
                inbound.drain(..excess);
            }
        }
        inbound.extend_from_slice(bytes);
        if let Some(end_at) = find_subslice(&inbound, END_MARK) {
            let after = end_at + END_MARK.len();
            let leftover = if after < inbound.len() {
                inbound[after..].to_vec()
            } else {
                Vec::new()
            };
            inbound.truncate(after);
            self.divert.store(false, Ordering::Release);
            self.complete.store(true, Ordering::Release);
            leftover
        } else {
            Vec::new()
        }
    }

    /// Take the diverted dump once the end marker has arrived.
    pub(crate) fn take_if_complete(&self) -> Option<Vec<u8>> {
        if !self.complete.swap(false, Ordering::AcqRel) {
            return None;
        }
        self.inbound
            .lock()
            .ok()
            .map(|mut inbound| std::mem::take(&mut *inbound))
    }
}

/// Bytes written to the PTY to dump the remote shell's in-memory history.
///
/// Clears the current line (`^U`), prints begin/end markers around `history`
/// (bash) or `fc -ln` (zsh), and avoids recording the probe itself when the
/// shell honors `set +o history` / `setopt HIST_NO_STORE`.
pub(crate) fn probe_command_bytes() -> Vec<u8> {
    // One line, portable enough for bash/zsh login shells behind a jumper.
    // Markers use octal \036 so the shell emits the same RS bytes we scan for.
    let body = concat!(
        "set +o history 2>/dev/null || setopt HIST_NO_STORE 2>/dev/null; ",
        "printf '\\036TTY7_HIST_BEGIN\\036\\n'; ",
        "{ history 2>/dev/null || fc -ln 1 2>/dev/null || true; }; ",
        "printf '\\036TTY7_HIST_END\\036\\n'\r"
    );
    let mut out = Vec::with_capacity(1 + body.len());
    out.push(0x15); // ^U clear line
    out.extend_from_slice(body.as_bytes());
    out
}

/// Parse a diverted dump into command lines (oldest → newest).
pub(crate) fn parse_history_builtin_dump(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let start = text
        .find("TTY7_HIST_BEGIN")
        .map(|i| i + "TTY7_HIST_BEGIN".len())
        .unwrap_or(0);
    let end = text[start..]
        .find("TTY7_HIST_END")
        .map(|i| start + i)
        .unwrap_or(text.len());
    let body = text[start..end]
        .trim_matches(|c: char| c == '\x1e' || c.is_whitespace());

    let mut out = Vec::new();
    for raw in body.lines() {
        let line = raw.trim_matches(|c: char| c == '\x1e' || c == '\r' || c.is_whitespace());
        if line.is_empty() || line.contains("TTY7_HIST") {
            continue;
        }
        if let Some(cmd) = strip_history_number(line) {
            let cmd = cmd.trim();
            if !cmd.is_empty() && !cmd.contains("TTY7_HIST") && cmd != "\u{1e}" {
                out.push(cmd.to_string());
            }
        }
    }
    // Cap like file-backed history so a huge dump cannot blow the overlay.
    const MAX: usize = 5000;
    if out.len() > MAX {
        out.drain(..out.len() - MAX);
    }
    out
}

/// `  512  ls -la` / `512* cmd` → `ls -la`; bare `fc -ln` lines pass through.
fn strip_history_number(line: &str) -> Option<&str> {
    let rest = line.trim_start();
    let mut chars = rest.char_indices();
    let Some((_, first)) = chars.next() else {
        return None;
    };
    if !first.is_ascii_digit() {
        return Some(rest);
    }
    for (i, c) in rest.char_indices() {
        if c.is_ascii_digit() {
            continue;
        }
        if c == '*' {
            // bash "modified" marker after the index
            let after_star = i + c.len_utf8();
            let tail = rest.get(after_star..)?.trim_start();
            return (!tail.is_empty()).then_some(tail);
        }
        if c.is_whitespace() {
            let tail = rest[i..].trim_start();
            return (!tail.is_empty()).then_some(tail);
        }
        // Digit run then non-space junk — treat as a bare command.
        return Some(rest);
    }
    // Entire line was digits.
    None
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bash_history_lines() {
        let dump = b"\x1eTTY7_HIST_BEGIN\x1e\n  510  ll\n  511  jobs\n  512  vim scon_agentic.py\n\x1eTTY7_HIST_END\x1e\n";
        assert_eq!(
            parse_history_builtin_dump(dump),
            vec![
                "ll".to_string(),
                "jobs".to_string(),
                "vim scon_agentic.py".to_string(),
            ]
        );
    }

    #[test]
    fn parse_fc_ln_bare_lines() {
        let dump = b"\x1eTTY7_HIST_BEGIN\x1e\nhostname\necho hi\n\x1eTTY7_HIST_END\x1e";
        assert_eq!(
            parse_history_builtin_dump(dump),
            vec!["hostname".to_string(), "echo hi".to_string()]
        );
    }

    #[test]
    fn parse_skips_star_marker_and_probe_echo() {
        let dump = concat!(
            "\x1eTTY7_HIST_BEGIN\x1e\n",
            "  1* ls\n",
            "  2  printf TTY7_HIST_BEGIN\n",
            "  3  real command\n",
            "\x1eTTY7_HIST_END\x1e"
        );
        assert_eq!(
            parse_history_builtin_dump(dump.as_bytes()),
            vec!["ls".to_string(), "real command".to_string()]
        );
    }

    #[test]
    fn pipe_diverts_until_end_then_passes_tail() {
        let pipe = HistoryProbePipe::new();
        pipe.arm();
        assert!(pipe.is_diverting());
        assert!(pipe.filter_output(b"echoed cmd\r\n").is_empty());
        assert!(pipe.filter_output(BEGIN_MARK).is_empty());
        assert!(pipe.filter_output(b"\n  1  ls\n").is_empty());
        let tail = pipe.filter_output(b"\x1eTTY7_HIST_END\x1e\r\nprompt> ");
        assert_eq!(tail, b"\r\nprompt> ");
        assert!(!pipe.is_diverting());
        let captured = pipe.take_if_complete().expect("complete");
        assert_eq!(
            parse_history_builtin_dump(&captured),
            vec!["ls".to_string()]
        );
        assert!(pipe.take_if_complete().is_none());
    }

    #[test]
    fn probe_command_clears_line_and_contains_markers() {
        let cmd = probe_command_bytes();
        assert_eq!(cmd[0], 0x15);
        let s = String::from_utf8_lossy(&cmd);
        assert!(s.contains("TTY7_HIST_BEGIN"));
        assert!(s.contains("TTY7_HIST_END"));
        assert!(s.contains("history"));
        assert!(s.contains("fc -ln"));
    }
}
