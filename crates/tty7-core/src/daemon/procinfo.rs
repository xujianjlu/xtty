use std::collections::HashMap;

use crate::daemon::protocol::{PaneProcs, PortEntry, PortProbe, ProcEntry};

const MAX_DEPTH: u8 = 6;

/// How many processes the panel is asked to draw.
const MAX_PROCS: usize = 64;

/// How far the walk itself goes before it calls the tree pathological.
///
/// This is deliberately far above `MAX_PROCS`, and the gap is the point. The
/// walk is depth-first over children sorted by ascending pid, so capping it at
/// the number of rows the panel wants meant one busy early branch — a build,
/// a container runtime, an agent's worker pool — could consume the whole
/// budget before the traversal ever reached the pane's newest child. A server
/// someone just started is the *last* pid in that ordering, so the one process
/// the Ports section exists for was the one most likely to fall off the end,
/// and it fell off silently. The port probe is asked about everything the walk
/// found; only the list handed to the panel is cut back to `MAX_PROCS`.
const MAX_TREE: usize = 512;

pub fn snapshot(shell_pid: u32, fg_pgid: Option<i32>) -> PaneProcs {
    let table = process_table();
    let procs = walk(&table, shell_pid, fg_pgid);
    let (ports, probe) = listening_ports(&procs);
    let probe = match probe.is_ok() && tree_has_foreign_uid(&table, &procs, current_uid()) {
        true => PortProbe::Restricted,
        false => probe,
    };
    if let PortProbe::Unavailable(detail) = &probe {
        note_probe_failure(shell_pid, detail);
    }
    finish(procs, ports, probe)
}

/// How long the same probe failure waits before it is written down again.
///
/// `snapshot` answers one `QueryProcs`, and the Info panel sends one every two
/// seconds for as long as it is open — a probe that cannot run now will not
/// have started working two seconds later. Logging every attempt turns one
/// standing fact into thirty lines a minute in a file that truncates itself at
/// 4 MiB, which costs the reporter the rest of the session they turned logging
/// on to capture. Once a minute still leaves a trail for a failure that
/// outlives the panel.
const PROBE_LOG_GAP: std::time::Duration = std::time::Duration::from_secs(60);

fn note_probe_failure(shell_pid: u32, detail: &str) {
    static LAST: std::sync::Mutex<Option<(String, std::time::Instant)>> =
        std::sync::Mutex::new(None);
    let line = format!("listening-port probe failed for pane shell {shell_pid}: {detail}");
    let Ok(mut last) = LAST.lock() else { return };
    if probe_log_due(&mut last, &line, std::time::Instant::now(), PROBE_LOG_GAP) {
        log::warn!("{line}");
    }
}

/// Whether `line` is worth a log entry now, given what was written last.
///
/// A line that has not been said before is always worth saying — a probe that
/// starts failing for a second reason, or a second pane failing for the same
/// one, is news. Repeating one is worth it only once per `gap`.
fn probe_log_due(
    last: &mut Option<(String, std::time::Instant)>,
    line: &str,
    now: std::time::Instant,
    gap: std::time::Duration,
) -> bool {
    if let Some((said, at)) = last.as_ref() {
        if said == line && now.duration_since(*at) < gap {
            return false;
        }
    }
    *last = Some((line.to_string(), now));
    true
}

/// The answer as the panel gets it: the process list trimmed to what a sidebar
/// can show, and the ports left whole.
///
/// Trimming here rather than in `walk` is what keeps a port owned by the 100th
/// process in the tree on screen — the row names its owner out of the full
/// list, so cutting the list afterwards costs the panel a process row it had
/// no room for and costs the Ports section nothing.
fn finish(mut procs: Vec<ProcEntry>, ports: Vec<PortEntry>, probe: PortProbe) -> PaneProcs {
    procs.truncate(MAX_PROCS);
    PaneProcs {
        procs,
        ports,
        probe,
        // The caller fills `context`: only the pane knows where its session
        // lives, and this module only ever walks *this* machine's table.
        context: None,
    }
}

/// Whether any process in the pane's tree belongs to a user other than `me`.
///
/// `sudo go run main.go` is the shape this is about. The process tree still
/// walks — the kernel will name another user's processes — but the sockets
/// they hold are readable only by their owner or by root, so `lsof` running as
/// this user answers "nothing is listening" about a server that plainly is.
/// Saying "some of these are another user's" is the difference between a panel
/// that is wrong and a panel that is honest.
fn tree_has_foreign_uid(table: &HashMap<u32, Row>, procs: &[ProcEntry], me: u32) -> bool {
    // Root sees everyone's sockets, so nothing is hidden from a daemon that is
    // already root and there is nothing to warn about.
    if me == 0 {
        return false;
    }
    procs.iter().any(|p| {
        table
            .get(&p.pid)
            .is_some_and(|row| row.uid != me && confirm_foreign(p.pid, me))
    })
}

/// A second opinion on a row that looks like another user's.
///
/// Asked only about the pane's own tree, and only about the rows that already
/// look foreign, so an ordinary pane pays nothing for it and a `sudo` pays one
/// file read.
///
/// Linux needs it because the owner of `/proc/<pid>` is not always the process's
/// uid: the kernel makes that directory `root:root` whenever a process's
/// dumpable attribute has been cleared, which is what executing a set-user-ID
/// binary or one carrying file capabilities does. A plain `ping` in a pane is
/// the user's own process behind a root-owned `/proc` entry, and taking the
/// directory's word for it would have the panel apologise for sockets it can
/// read perfectly well — the opposite mistake to the one this file is fixing,
/// and just as wrong. `Uid:` in `/proc/<pid>/status` is the real answer and is
/// readable either way.
#[cfg(target_os = "linux")]
fn confirm_foreign(pid: u32, me: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(text) => status_euid(&text).is_none_or(|uid| uid != me),
        // Gone, or unreadable: keep the directory's verdict rather than
        // inventing a second one out of a failed read.
        Err(_) => true,
    }
}

/// macOS reads the effective uid straight out of `PROC_PIDTBSDINFO`, and
/// Windows has no uid to be wrong about, so there is nothing to confirm.
#[cfg(not(target_os = "linux"))]
fn confirm_foreign(_pid: u32, _me: u32) -> bool {
    true
}

/// The effective uid on the `Uid:` line of a `/proc/<pid>/status`, which reads
/// `Uid:\t<real>\t<effective>\t<saved>\t<filesystem>`.
///
/// The effective one is the second: it is the credential the kernel checks when
/// something asks to read the process's sockets.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn status_euid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` reads the calling process's own credentials and cannot
    // fail.
    unsafe { libc::getuid() }
}

/// Windows has no uid, and its port probe is a kernel table rather than a
/// subprocess with an identity — `Row::uid` is 0 there and so is this, so the
/// check above is a constant false.
#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

struct Row {
    ppid: u32,
    pgid: u32,
    /// The effective uid of the process, which is what decides whether this
    /// daemon may look at its sockets. 0 on platforms that have no such thing,
    /// and on Linux the cheapest reading of it rather than the last word — see
    /// `confirm_foreign`.
    uid: u32,
    name: String,
}

fn walk(table: &HashMap<u32, Row>, shell_pid: u32, fg_pgid: Option<i32>) -> Vec<ProcEntry> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, row) in table {
        children.entry(row.ppid).or_default().push(*pid);
    }
    for kids in children.values_mut() {
        kids.sort_unstable();
    }

    let mut out = Vec::new();
    let mut stack = vec![(shell_pid, 0u8)];
    while let Some((pid, depth)) = stack.pop() {
        let Some(row) = table.get(&pid) else { continue };
        if out.len() >= MAX_TREE {
            break;
        }
        out.push(ProcEntry {
            pid,
            name: row.name.clone(),
            depth,
            foreground: fg_pgid.is_some_and(|g| g as u32 == row.pgid),
        });
        if depth + 1 > MAX_DEPTH {
            continue;
        }
        if let Some(kids) = children.get(&pid) {
            for kid in kids.iter().rev() {
                stack.push((*kid, depth + 1));
            }
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn process_table() -> HashMap<u32, Row> {
    let mut table = HashMap::new();
    let bytes = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if bytes <= 0 {
        return table;
    }
    let cap = (bytes as usize / std::mem::size_of::<libc::c_int>()) + 64;
    let mut pids = vec![0 as libc::c_int; cap];
    let written = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr() as *mut libc::c_void,
            (cap * std::mem::size_of::<libc::c_int>()) as libc::c_int,
        )
    };
    if written <= 0 {
        return table;
    }
    let n = written as usize / std::mem::size_of::<libc::c_int>();
    for &pid in pids.iter().take(n.min(cap)) {
        if pid <= 0 {
            continue;
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let ret = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if ret != size {
            continue;
        }
        let name = proc_name(pid).unwrap_or_else(|| cstr_field(&info.pbi_comm));
        table.insert(
            pid as u32,
            Row {
                ppid: info.pbi_ppid,
                pgid: info.pbi_pgid,
                // The effective uid, not the real one: a setuid `sudo` still
                // runs as the user who typed it, and it is the effective uid
                // that decides whose sockets `lsof` may read.
                uid: info.pbi_uid,
                name,
            },
        );
    }
    table
}

#[cfg(target_os = "macos")]
fn cstr_field(buf: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(target_os = "linux")]
fn process_table() -> HashMap<u32, Row> {
    let mut table = HashMap::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return table;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some(close) = stat.rfind(')') else {
            continue;
        };
        let mut fields = stat[close + 1..].split_whitespace();
        let (Some(_state), Some(ppid), Some(pgid)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(ppid), Ok(pgid)) = (ppid.parse::<u32>(), pgid.parse::<u32>()) else {
            continue;
        };
        let name = proc_name(pid as i32).unwrap_or_else(|| {
            stat[..close]
                .rfind('(')
                .map_or_else(|| String::new(), |open| stat[open + 1..close].to_string())
        });
        // `/proc/<pid>` is normally owned by the process's effective uid, which
        // is the one that governs who may read its sockets. Normally: the
        // kernel hands the directory to `root` for a process whose dumpable
        // attribute it cleared, so this is a cheap first pass over every pid on
        // the machine and `confirm_foreign` settles the few that look foreign
        // and are in a pane's tree. A stat that fails on a pid whose `stat`
        // file just parsed is a race with the process exiting; calling that
        // "mine" keeps a dying process from being mistaken for another user's.
        let uid = std::fs::metadata(format!("/proc/{pid}"))
            .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
            .unwrap_or_else(|_| current_uid());
        table.insert(
            pid,
            Row {
                ppid,
                pgid,
                uid,
                name,
            },
        );
    }
    table
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_table() -> HashMap<u32, Row> {
    HashMap::new()
}

/// The executable name behind a pid.
///
/// One copy, shared with `pane.rs`. There used to be two, and each carried a
/// guard the other lacked — this one had no `pid <= 0` check, and its Linux
/// arm had no `/proc/<pid>/comm` fallback — so the two disagreed about the
/// name of the same process whenever the executable link was unreadable.
///
/// Callers may still layer their own fallback on top: `process_table` reaches
/// for the kernel's short name when this returns `None`, which is what covers
/// a process whose path this cannot read at all.
#[cfg(target_os = "macos")]
pub(super) fn proc_name(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let ret =
        unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if ret <= 0 {
        return None;
    }
    let path = std::str::from_utf8(&buf[..ret as usize]).ok()?;
    Some(path.rsplit('/').next().unwrap_or(path).to_string())
}

/// See the macOS arm above.
///
/// `/proc/<pid>/exe` is a link the kernel refuses to resolve for a process
/// owned by someone else, so the `comm` fallback is what keeps a differently
/// owned process from coming back nameless.
#[cfg(target_os = "linux")]
pub(super) fn proc_name(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    if let Ok(path) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let name = name.strip_suffix(" (deleted)").unwrap_or(name);
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    (!comm.is_empty()).then(|| comm.to_string())
}

/// Where to look for `lsof`, in order.
///
/// `PATH` first, and then the two absolute paths it actually lives at, because
/// the daemon's `PATH` is not the shell's. macOS ships `lsof` in `/usr/sbin`,
/// which is on the default login `PATH` and is exactly the kind of entry a
/// hand-written `export PATH=...` in a dotfile drops on the floor — and the
/// daemon inherits whatever the app that launched it had. Falling back to the
/// absolute path costs one failed `execvp` in the case that used to end with
/// the panel quietly claiming nothing was listening.
#[cfg(unix)]
const LSOF_CANDIDATES: [&str; 3] = ["lsof", "/usr/sbin/lsof", "/usr/bin/lsof"];

/// How long the probe gets before it is declared hung.
///
/// `lsof` is famous for blocking on a wedged network mount, and this one runs
/// on the daemon's connection thread: without a bound, one stuck call does not
/// merely lose a port, it stops the pane answering `QueryProcs` at all, for as
/// long as the mount stays wedged. The Info panel re-polls every two seconds,
/// so a probe still running after three has already missed its slot.
#[cfg(unix)]
const PROBE_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

#[cfg(unix)]
fn listening_ports(procs: &[ProcEntry]) -> (Vec<PortEntry>, PortProbe) {
    if procs.is_empty() {
        return (Vec::new(), PortProbe::Ok);
    }
    let pid_list = procs
        .iter()
        .map(|p| p.pid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut first_err = String::new();
    for tool in LSOF_CANDIDATES {
        let mut cmd = std::process::Command::new(tool);
        cmd.args([
            "-nP",
            "-iTCP",
            "-sTCP:LISTEN",
            "-a",
            "-p",
            &pid_list,
            "-Fpn",
        ]);
        match run_bounded(cmd, PROBE_BUDGET) {
            Ok(Run::Finished(out)) => {
                let ports = parse_lsof(&String::from_utf8_lossy(&out.stdout), procs);
                // The exit status is deliberately not read as failure. `lsof`
                // returns 1 for a pid it could not locate, and a pane's tree
                // grows and loses processes between the walk and this call as
                // a matter of course — treating that as a broken probe would
                // put a doubt on screen every time a `ls` finished. What the
                // status is worth is a log line when the run both complained
                // and came back with nothing.
                if ports.is_empty() && !out.status.success() {
                    if let Some(line) = String::from_utf8_lossy(&out.stderr)
                        .lines()
                        .find(|l| !l.trim().is_empty())
                    {
                        log::debug!("{tool} found no listeners and said: {line}");
                    }
                }
                return (ports, PortProbe::Ok);
            }
            Ok(Run::TimedOut) => {
                return (
                    Vec::new(),
                    PortProbe::Unavailable(format!(
                        "{tool} did not answer within {}s",
                        PROBE_BUDGET.as_secs()
                    )),
                );
            }
            // Try the next candidate: this one is not there, or is not
            // runnable. The complaint kept is the first one, about the name as
            // the daemon's `PATH` sees it, since that is the failure worth
            // reading — the others are fallbacks nobody asked for.
            Err(e) => {
                if first_err.is_empty() {
                    first_err = format!("{tool}: {e}");
                }
            }
        }
    }
    (
        Vec::new(),
        PortProbe::Unavailable(format!(
            "{first_err} (also tried {})",
            LSOF_CANDIDATES[1..].join(", ")
        )),
    )
}

/// What became of a probe process.
///
/// Compiled everywhere and used by the unix probe and by the tests, which is
/// how a parser and a timeout that only ever run on macOS and Linux get
/// exercised on a Windows machine.
#[cfg_attr(not(unix), allow(dead_code))]
enum Run {
    Finished(std::process::Output),
    TimedOut,
}

/// Run `cmd` to completion, or kill it once `budget` is up.
///
/// `Command::output` has no deadline, and the caller is a daemon thread that a
/// hung child would own forever. Both pipes are read after the wait rather
/// than while it runs, which is safe for a probe whose whole output is a few
/// hundred bytes and which is killed if it ever stops making progress.
///
/// A killed run is reaped and its output abandoned unread. Reading it would
/// reintroduce the hang this exists to prevent: a child that spawned something
/// of its own hands the write end of the pipe on, and waiting for end-of-file
/// then means waiting for a grandchild nobody killed.
#[cfg_attr(not(unix), allow(dead_code))]
fn run_bounded(
    mut cmd: std::process::Command,
    budget: std::time::Duration,
) -> std::io::Result<Run> {
    use std::process::Stdio;

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = std::time::Instant::now() + budget;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(Run::Finished(child.wait_with_output()?));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            // Reaps the process this call started, so a timed-out probe leaves
            // no zombie behind; the pipes close as `child` drops.
            let _ = child.wait();
            return Ok(Run::TimedOut);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The listeners in one `lsof -Fpn` report, named after the processes that own
/// them.
///
/// Split out from the call so the format can be tested off a Mac: this parser
/// is the half of the probe that has no platform in it.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_lsof(text: &str, procs: &[ProcEntry]) -> Vec<PortEntry> {
    let by_pid: HashMap<u32, &str> = procs.iter().map(|p| (p.pid, p.name.as_str())).collect();
    let mut ports: Vec<PortEntry> = Vec::new();
    let mut current = 0u32;
    for line in text.lines() {
        let Some((tag, rest)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => current = rest.parse().unwrap_or(0),
            "n" => {
                let Some((addr, port)) = parse_listen_addr(rest) else {
                    continue;
                };
                record_listener(
                    &mut ports,
                    by_pid.get(&current).copied().unwrap_or_default(),
                    port,
                    current,
                    addr.to_string(),
                );
            }
            _ => {}
        }
    }
    ports.sort_by_key(|e| (e.port, e.pid));
    ports
}

/// Adds one listening socket to the list, merging it with a row already there
/// for the same port and pid.
///
/// One rule for both platforms — it was written twice, once inline in the
/// `lsof` parser and once here, and two copies of a merge rule is one too
/// many. A process bound to both `192.168.1.5` and `*` is on localhost, and
/// the row the panel turns into a clickable URL should say so rather than
/// keeping whichever address the kernel or `lsof` happened to list first.
fn record_listener(ports: &mut Vec<PortEntry>, name: &str, port: u16, pid: u32, addr: String) {
    if let Some(seen) = ports.iter_mut().find(|e| e.port == port && e.pid == pid) {
        if !PortEntry::reaches_loopback(&seen.addr) && PortEntry::reaches_loopback(&addr) {
            seen.addr = addr;
        }
        return;
    }
    ports.push(PortEntry {
        port,
        pid,
        addr,
        name: name.to_string(),
    });
}

#[cfg(not(unix))]
fn listening_ports(_procs: &[ProcEntry]) -> (Vec<PortEntry>, PortProbe) {
    (
        Vec::new(),
        PortProbe::Unavailable("no port probe on this platform".to_string()),
    )
}

/// The address and port `lsof -Fn` reports a listener on — `*:3000`,
/// `127.0.0.1:8080`, `[::1]:5173`.
///
/// The address used to be dropped on the floor, which was harmless while the
/// port was a number to read. It stopped being harmless when the panel started
/// handing the port over as an address to open: a server bound only to
/// `172.17.0.1` or a LAN address is not on `localhost`, and offering it as one
/// sends the browser to a refused connection or, worse, to whatever else holds
/// that port on loopback.
fn parse_listen_addr(name: &str) -> Option<(&str, u16)> {
    let name = name.split_whitespace().next()?;
    let (addr, port) = name.rsplit_once(':')?;
    Some((addr, port.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The uid every fabricated row belongs to unless a test says otherwise.
    const ME: u32 = 501;

    fn row(ppid: u32, name: &str) -> Row {
        Row {
            ppid,
            pgid: 0,
            uid: ME,
            name: name.to_string(),
        }
    }

    #[test]
    fn walk_is_depth_first_from_the_shell() {
        let table: HashMap<u32, Row> = [
            (100, row(1, "zsh")),
            (200, row(100, "make")),
            (300, row(200, "cc")),
            (400, row(100, "vim")),
            (500, row(1, "Finder")),
        ]
        .into_iter()
        .collect();

        let got = walk(&table, 100, None);
        let names: Vec<_> = got.iter().map(|p| (p.name.as_str(), p.depth)).collect();
        assert_eq!(
            names,
            vec![("zsh", 0), ("make", 1), ("cc", 2), ("vim", 1)],
            "depth-first, ascending pid, shell's tree only"
        );
    }

    #[test]
    fn walk_marks_the_foreground_process_group() {
        let mut table: HashMap<u32, Row> = [(100, row(1, "zsh")), (200, row(100, "vim"))]
            .into_iter()
            .collect();
        table.get_mut(&100).unwrap().pgid = 100;
        table.get_mut(&200).unwrap().pgid = 200;

        let got = walk(&table, 100, Some(200));
        assert!(
            !got[0].foreground,
            "the shell is backgrounded while vim runs"
        );
        assert!(got[1].foreground, "vim's group owns the terminal");
    }

    #[test]
    fn walk_survives_a_cycle_in_the_table() {
        let table: HashMap<u32, Row> = [(100, row(200, "a")), (200, row(100, "b"))]
            .into_iter()
            .collect();
        let got = walk(&table, 100, None);
        assert!(got.len() <= MAX_TREE, "bounded, not infinite");
    }

    /// #731. The walk is depth-first over children in ascending pid order, so
    /// a shell whose earlier children brought a crowd used to exhaust the row
    /// budget before the traversal reached the newest child — and the newest
    /// child, highest pid and visited last, is precisely the `go run` someone
    /// started ten seconds ago and is looking for the port of.
    #[test]
    fn the_newest_child_survives_a_shell_crowded_with_older_ones() {
        let mut table: HashMap<u32, Row> = [(100, row(1, "zsh"))].into_iter().collect();
        // 80 older children, each with a child of its own: 160 processes, well
        // past the 64 the panel draws.
        for i in 0..80u32 {
            table.insert(200 + i * 2, row(100, "node"));
            table.insert(201 + i * 2, row(200 + i * 2, "esbuild"));
        }
        table.insert(9000, row(100, "go"));
        table.insert(9001, row(9000, "main"));

        let walked = walk(&table, 100, None);
        assert!(
            walked.iter().any(|p| p.pid == 9001 && p.name == "main"),
            "the process holding the listener must reach the probe, got {} rows",
            walked.len()
        );

        let ports = vec![PortEntry {
            port: 8080,
            pid: 9001,
            addr: "*".into(),
            name: "main".into(),
        }];
        let out = finish(walked, ports, PortProbe::Ok);
        assert_eq!(
            out.procs.len(),
            MAX_PROCS,
            "the panel still gets a short list"
        );
        assert_eq!(
            out.ports.first().map(|p| p.port),
            Some(8080),
            "and the port survives the trim that dropped its owner's row"
        );
    }

    #[test]
    fn a_pathological_tree_still_stops() {
        let mut table: HashMap<u32, Row> = [(100, row(1, "zsh"))].into_iter().collect();
        for pid in 200..2000u32 {
            table.insert(pid, row(100, "fork-bomb"));
        }
        assert_eq!(walk(&table, 100, None).len(), MAX_TREE);
    }

    /// A `sudo go run` is a server the panel can see and a socket it cannot.
    /// Saying nothing is listening is the one answer that is certainly wrong.
    #[test]
    fn another_users_process_in_the_tree_is_noticed() {
        let mut table: HashMap<u32, Row> = [
            (100, row(1, "zsh")),
            (200, row(100, "sudo")),
            (300, row(200, "main")),
        ]
        .into_iter()
        .collect();
        let procs = walk(&table, 100, None);
        assert!(
            !tree_has_foreign_uid(&table, &procs, ME),
            "everything is mine until sudo takes over"
        );

        table.get_mut(&200).unwrap().uid = 0;
        table.get_mut(&300).unwrap().uid = 0;
        assert!(tree_has_foreign_uid(&table, &procs, ME));
        assert!(
            !tree_has_foreign_uid(&table, &procs, 0),
            "a daemon already running as root is shown everyone's sockets"
        );
    }

    /// The `go run` shape from #731, as `lsof -Fpn` reports it: the shell is in
    /// the pid list and holds nothing, the compiled binary under `$TMPDIR`
    /// holds the listener, and it is bound over both address families.
    #[test]
    fn parses_a_go_run_report() {
        let procs = vec![
            ProcEntry {
                pid: 100,
                name: "zsh".into(),
                depth: 0,
                foreground: false,
            },
            ProcEntry {
                pid: 9000,
                name: "go".into(),
                depth: 1,
                foreground: true,
            },
            ProcEntry {
                pid: 9001,
                name: "main".into(),
                depth: 2,
                foreground: true,
            },
        ];
        let report = "p9001\nf3\nn*:8080\nf5\nn[::]:8080\np100\n";
        let ports = parse_lsof(report, &procs);
        assert_eq!(
            ports.len(),
            1,
            "one server, not one row per family: {ports:?}"
        );
        assert_eq!(ports[0].port, 8080);
        assert_eq!(ports[0].pid, 9001);
        assert_eq!(
            ports[0].name, "main",
            "the row names the binary `go run` built, not `go`"
        );
        assert_eq!(ports[0].authority(), "localhost:8080");
    }

    #[test]
    fn a_report_about_nobody_we_asked_about_still_parses() {
        // A pid that vanished between the walk and the probe leaves a row with
        // no name rather than dropping the port someone can still click.
        let ports = parse_lsof("p4242\nn127.0.0.1:5173\n", &[]);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].name, "");
    }

    #[test]
    fn a_quick_probe_comes_back_whole() {
        let got = run_bounded(echo_command(), std::time::Duration::from_secs(30));
        let out = match got {
            Ok(Run::Finished(ref out)) => out,
            ref other => panic!("expected a finished run, got {}", describe(other)),
        };
        assert!(String::from_utf8_lossy(&out.stdout).contains("tty7"));
    }

    /// The bound is the whole point: this probe runs on the daemon thread that
    /// answers `QueryProcs`, and an `lsof` wedged on a dead mount used to own
    /// it forever.
    ///
    /// The sleeper is deliberately a wrapper around a second process, which is
    /// the shape that makes this hard: killing the child does not close the
    /// pipe its own child inherited, so a `wait_with_output` after the kill
    /// would sit on that pipe for the full sleep and hand the caller a timeout
    /// that took as long as no timeout at all.
    #[test]
    fn a_probe_that_never_finishes_is_killed_and_named() {
        let cmd = sleeper_command();
        let started = std::time::Instant::now();
        let got = run_bounded(cmd, std::time::Duration::from_millis(300));
        assert!(
            matches!(got, Ok(Run::TimedOut)),
            "expected a timeout, got {}",
            describe(&got)
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "the caller was released long before the child would have exited"
        );
    }

    #[test]
    fn a_missing_probe_is_an_error_and_not_an_empty_answer() {
        let cmd = std::process::Command::new("tty7-no-such-probe-b0rk");
        assert!(
            run_bounded(cmd, std::time::Duration::from_secs(5)).is_err(),
            "a tool that is not there has to be distinguishable from one that found nothing"
        );
    }

    /// A failure that stands still is one fact, and `snapshot` is asked again
    /// every two seconds for as long as the Info panel is open.
    #[test]
    fn a_standing_probe_failure_is_logged_once_a_minute_not_once_a_poll() {
        let mut last = None;
        let t0 = std::time::Instant::now();
        let gap = std::time::Duration::from_secs(60);
        let line = "shell 100: lsof: program not found";
        assert!(
            probe_log_due(&mut last, line, t0, gap),
            "the first one talks"
        );
        for poll in 1..30u64 {
            let now = t0 + std::time::Duration::from_secs(poll * 2);
            assert!(
                !probe_log_due(&mut last, line, now, gap),
                "the same reason again at +{}s",
                poll * 2
            );
        }
        assert!(
            probe_log_due(&mut last, "shell 100: lsof: permission denied", t0, gap),
            "a different reason is news even in the same breath"
        );
        assert!(
            probe_log_due(&mut last, line, t0 + gap, gap),
            "and a failure that outlives the gap still leaves a trail"
        );
    }

    /// A `/proc/<pid>` the kernel handed to root is not evidence of another
    /// user: `ping`, and anything else carrying file capabilities, is the
    /// caller's own process behind one.
    #[test]
    fn the_status_files_effective_uid_is_the_one_that_counts() {
        let ping = "Name:\tping\nState:\tS (sleeping)\nTgid:\t4242\n\
                    Uid:\t501\t501\t501\t501\nGid:\t20\t20\t20\t20\n";
        assert_eq!(status_euid(ping), Some(501));

        let sudo = "Name:\tmain\nUid:\t501\t0\t0\t0\n";
        assert_eq!(
            status_euid(sudo),
            Some(0),
            "the effective uid, not the real one that typed the password"
        );

        assert_eq!(status_euid("Name:\tzsh\n"), None);
        assert_eq!(
            status_euid("Uid:\t501\n"),
            None,
            "a truncated line is no answer"
        );
    }

    fn describe(run: &std::io::Result<Run>) -> String {
        match run {
            Ok(Run::Finished(out)) => format!("finished with {}", out.status),
            Ok(Run::TimedOut) => "a timeout".to_string(),
            Err(e) => format!("an error: {e}"),
        }
    }

    #[cfg(not(windows))]
    fn echo_command() -> std::process::Command {
        let mut c = std::process::Command::new("echo");
        c.arg("tty7");
        c
    }

    #[cfg(not(windows))]
    fn sleeper_command() -> std::process::Command {
        let mut c = std::process::Command::new("sleep");
        c.arg("60");
        c
    }

    #[test]
    fn parses_lsof_listen_addresses() {
        assert_eq!(parse_listen_addr("*:3000"), Some(("*", 3000)));
        assert_eq!(
            parse_listen_addr("127.0.0.1:8080"),
            Some(("127.0.0.1", 8080))
        );
        assert_eq!(parse_listen_addr("[::1]:5173"), Some(("[::1]", 5173)));
        assert_eq!(parse_listen_addr("*:5432 (LISTEN)"), Some(("*", 5432)));
        assert_eq!(parse_listen_addr("/tmp/some.sock"), None);
    }

    #[test]
    fn an_address_only_becomes_localhost_when_localhost_reaches_it() {
        // What the panel copies and opens. A wildcard or a loopback bind is
        // spelled the way anyone would type it; an interface-specific bind is
        // kept, because `localhost` is not that server.
        let entry = |addr: &str| PortEntry {
            port: 8080,
            pid: 1,
            addr: addr.into(),
            name: "server".into(),
        };
        for addr in ["", "*", "0.0.0.0", "::", "[::]", "127.0.0.1", "[::1]"] {
            assert_eq!(
                entry(addr).authority(),
                "localhost:8080",
                "{addr} is reachable on loopback"
            );
        }
        assert_eq!(entry("172.17.0.1").authority(), "172.17.0.1:8080");
        assert_eq!(entry("192.168.1.20").authority(), "192.168.1.20:8080");
    }
}

