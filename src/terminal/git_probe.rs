//! PTY `git` status dump for nested `ssh` / jumper hops.
//!
//! Process-table nested SSH has no `Host` / SFTP to the inner box. Native SSH
//! probes via `Host::git`; here we inject a quiet framed dump into the PTY
//! (same divert pattern as [`super::history_probe`]), parse it into a
//! [`RepoSnapshot`], and never scrape branch / dirty state from PS1.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::core::git::RepoSnapshot;

/// Unique markers — ASCII RS keeps them off most fonts / prompts.
pub(crate) const BEGIN_MARK: &[u8] = b"\x1eTTY7_GIT_BEGIN\x1e";
pub(crate) const END_MARK: &[u8] = b"\x1eTTY7_GIT_END\x1e";
pub(crate) const SEP_MARK: &[u8] = b"\x1eTTY7_GIT_SEP\x1e";

const MAX_CAPTURE: usize = 256 * 1024;

/// Shared with the reader thread: divert PTY bytes until the end marker lands.
#[derive(Default)]
pub(crate) struct GitProbePipe {
    divert: AtomicBool,
    complete: AtomicBool,
    inbound: Mutex<Vec<u8>>,
}

impl GitProbePipe {
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

/// Bytes written to the PTY to dump remote `git` identity + diff counts.
///
/// Same shape as the Native / `Host::git` probe (`rev-parse` paths, branch,
/// `diff --numstat HEAD`), framed so the divert pipe can hide the echo.
/// `GIT_OPTIONAL_LOCKS=0` keeps the read path write-free (no index refresh).
pub(crate) fn probe_command_bytes() -> Vec<u8> {
    let body = concat!(
        "set +o history 2>/dev/null || setopt HIST_NO_STORE 2>/dev/null; ",
        "printf '\\036TTY7_GIT_BEGIN\\036\\n'; ",
        "GIT_OPTIONAL_LOCKS=0; export GIT_OPTIONAL_LOCKS; ",
        "if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then ",
        "git rev-parse --path-format=absolute --show-toplevel --git-dir --git-common-dir 2>/dev/null; ",
        "printf '\\036TTY7_GIT_SEP\\036\\n'; ",
        "{ git symbolic-ref --quiet --short HEAD 2>/dev/null || git rev-parse --short HEAD 2>/dev/null || true; }; ",
        "printf '\\036TTY7_GIT_SEP\\036\\n'; ",
        "git diff --numstat HEAD 2>/dev/null || true; ",
        "fi; ",
        "printf '\\036TTY7_GIT_END\\036\\n'\r"
    );
    let mut out = Vec::with_capacity(1 + body.len());
    out.push(0x15); // ^U clear line
    out.extend_from_slice(body.as_bytes());
    out
}

/// Parse a diverted dump into a [`RepoSnapshot`], or `None` when not in a repo
/// / the frame is incomplete / junk-only (probe echo without markers).
pub(crate) fn parse_git_probe_dump(dump: &[u8]) -> Option<RepoSnapshot> {
    let begin = find_subslice(dump, BEGIN_MARK)?;
    let end = find_subslice(dump, END_MARK)?;
    if end < begin + BEGIN_MARK.len() {
        return None;
    }
    let body = &dump[begin + BEGIN_MARK.len()..end];
    // Outside a work tree the script prints only BEGIN…END — treat as "no repo".
    let parts: Vec<&[u8]> = split_on(body, SEP_MARK);
    if parts.is_empty() || parts.iter().all(|p| trim_ws(p).is_empty()) {
        return None;
    }
    // Need paths + branch sections; numstat may be empty.
    if parts.len() < 2 {
        return None;
    }
    let path_lines: Vec<&str> = lines_of(parts[0]);
    let root = path_lines.first().filter(|s| !s.is_empty()).map(PathBuf::from)?;
    let git_dir = path_lines.get(1).copied();
    let common = path_lines.get(2).copied();
    let home = repo_home_paths(&root, git_dir, common);
    let branch = lines_of(parts[1])
        .into_iter()
        .find(|s| !s.is_empty())
        .filter(|s| !is_probe_junk(s))?
        .to_string();
    let counts = parts.get(2).map(|p| sum_numstat(p));
    Some(RepoSnapshot {
        root,
        home,
        branch,
        counts,
    })
}

fn repo_home_paths(root: &Path, git_dir: Option<&str>, common: Option<&str>) -> PathBuf {
    let (Some(git_dir), Some(common)) = (git_dir, common) else {
        return root.to_path_buf();
    };
    if git_dir == common {
        return root.to_path_buf();
    }
    let common = PathBuf::from(common);
    if common.file_name().is_some_and(|name| name == ".git")
        && let Some(parent) = common.parent()
    {
        return parent.to_path_buf();
    }
    common
}

fn sum_numstat(bytes: &[u8]) -> (u32, u32) {
    let mut added = 0u32;
    let mut removed = 0u32;
    for line in lines_of(bytes) {
        let mut fields = line.split('\t');
        if let Some(n) = fields.next().and_then(|s| s.parse::<u32>().ok()) {
            added = added.saturating_add(n);
        }
        if let Some(n) = fields.next().and_then(|s| s.parse::<u32>().ok()) {
            removed = removed.saturating_add(n);
        }
    }
    (added, removed)
}

fn lines_of(bytes: &[u8]) -> Vec<&str> {
    let text = std::str::from_utf8(bytes).unwrap_or("");
    text.lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .filter(|l| !l.is_empty() && !is_probe_junk(l))
        .collect()
}

fn is_probe_junk(line: &str) -> bool {
    line.contains("TTY7_GIT_")
        || line.starts_with("set +o history")
        || line.starts_with("GIT_OPTIONAL_LOCKS=")
        || line.starts_with("if git rev-parse")
        || line.starts_with("git rev-parse")
        || line.starts_with("git symbolic-ref")
        || line.starts_with("git diff")
        || line.starts_with("printf ")
}

fn trim_ws(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

fn split_on<'a>(hay: &'a [u8], needle: &[u8]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut rest = hay;
    while let Some(at) = find_subslice(rest, needle) {
        out.push(&rest[..at]);
        rest = &rest[at + needle.len()..];
    }
    out.push(rest);
    out
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len())
        .position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut dump = Vec::new();
        dump.extend_from_slice(BEGIN_MARK);
        dump.extend_from_slice(b"\n");
        dump.extend_from_slice(body);
        dump.extend_from_slice(END_MARK);
        dump.extend_from_slice(b"\n");
        dump
    }

    #[test]
    fn parse_work_tree_snapshot() {
        let mut body = Vec::new();
        body.extend_from_slice(b"/home/carol/src/app\n");
        body.extend_from_slice(b"/home/carol/src/app/.git\n");
        body.extend_from_slice(b"/home/carol/src/app/.git\n");
        body.extend_from_slice(SEP_MARK);
        body.extend_from_slice(b"\nfeat/x\n");
        body.extend_from_slice(SEP_MARK);
        body.extend_from_slice(b"\n3\t1\tsrc/main.rs\n0\t2\tREADME.md\n");
        let snap = parse_git_probe_dump(&framed(&body)).expect("snapshot");
        assert_eq!(snap.root, PathBuf::from("/home/carol/src/app"));
        assert_eq!(snap.home, PathBuf::from("/home/carol/src/app"));
        assert_eq!(snap.branch, "feat/x");
        assert_eq!(snap.counts, Some((3, 3)));
    }

    #[test]
    fn parse_linked_worktree_home() {
        let mut body = Vec::new();
        body.extend_from_slice(b"/home/carol/src/app/.wt/feat\n");
        body.extend_from_slice(b"/home/carol/src/app/.git/worktrees/feat\n");
        body.extend_from_slice(b"/home/carol/src/app/.git\n");
        body.extend_from_slice(SEP_MARK);
        body.extend_from_slice(b"\nfeat/x\n");
        body.extend_from_slice(SEP_MARK);
        body.extend_from_slice(b"\n");
        let snap = parse_git_probe_dump(&framed(&body)).expect("snapshot");
        assert_eq!(snap.root, PathBuf::from("/home/carol/src/app/.wt/feat"));
        assert_eq!(snap.home, PathBuf::from("/home/carol/src/app"));
        assert_eq!(snap.branch, "feat/x");
        assert_eq!(snap.counts, Some((0, 0)));
    }

    #[test]
    fn parse_empty_frame_is_not_a_repo() {
        assert!(parse_git_probe_dump(&framed(b"\n")).is_none());
    }

    #[test]
    fn parse_rejects_unframed_junk() {
        assert!(parse_git_probe_dump(b"git status\nOn branch main\n").is_none());
    }

    #[test]
    fn pipe_diverts_until_end_then_passes_tail() {
        let pipe = GitProbePipe::new();
        pipe.arm();
        assert!(pipe.is_diverting());
        assert!(pipe.filter_output(b"echoed cmd\r\n").is_empty());
        assert!(pipe.filter_output(BEGIN_MARK).is_empty());
        assert!(pipe.filter_output(b"\n/repo\n").is_empty());
        let mut end = END_MARK.to_vec();
        end.extend_from_slice(b"$ ");
        let leftover = pipe.filter_output(&end);
        assert_eq!(leftover, b"$ ");
        assert!(!pipe.is_diverting());
        let dump = pipe.take_if_complete().expect("complete");
        assert!(find_subslice(&dump, BEGIN_MARK).is_some());
        assert!(find_subslice(&dump, END_MARK).is_some());
    }

    #[test]
    fn probe_command_clears_line_and_contains_markers() {
        let cmd = probe_command_bytes();
        assert_eq!(cmd[0], 0x15);
        assert!(find_subslice(&cmd, b"TTY7_GIT_BEGIN").is_some());
        assert!(find_subslice(&cmd, b"TTY7_GIT_END").is_some());
        assert!(find_subslice(&cmd, b"GIT_OPTIONAL_LOCKS").is_some());
        assert!(find_subslice(&cmd, b"rev-parse").is_some());
    }
}
