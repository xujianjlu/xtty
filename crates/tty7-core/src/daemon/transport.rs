use std::io;

use crate::core::config;

#[cfg(unix)]
pub use imp_unix::*;
/// Deal with an endpoint someone left behind, so the bind after it can only be
/// refused for a reason worth reporting.
///
/// `alone` is proof that this process holds the single-server seat. That, and
/// not a probe, is what makes removal safe: whoever holds the seat is the
/// server, so anything still sitting at the endpoint belongs to a process that
/// is gone. Answering "is a server already running?" by connecting and reading
/// one failed connect as proof of death is the race [`crate::daemon::singleton`]
/// exists to eliminate — the loser holds a listener on an unlinked path and
/// serves nobody, forever — so it is only used where there is no seat to reason
/// from, and even there a socket that answers is refused rather than removed.
///
/// Removing is not optional on Unix. The endpoint is a socket *file* and `bind`
/// refuses any path that already exists, so a daemon that died without
/// unlinking its socket stops every later daemon from ever starting: the client
/// launches one, it exits on the bind, the client launches another. On Windows
/// the endpoint is a port file the bind overwrites, and the removal costs
/// nothing.
pub fn clear_endpoint_before_bind(alone: bool) -> anyhow::Result<()> {
    if alone {
        remove_stale_endpoint();
        return Ok(());
    }
    if !endpoint_exists() {
        return Ok(());
    }
    match connect() {
        Ok(_) => Err(anyhow::anyhow!(
            "daemon already running at {}",
            endpoint_display()
        )),
        Err(_) => {
            remove_stale_endpoint();
            Ok(())
        }
    }
}

#[cfg(unix)]
mod imp_unix {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    pub type Stream = UnixStream;
    pub type Listener = UnixListener;

    pub(super) const MAX_SOCKET_PATH_BYTES: usize = 100;

    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    pub(crate) fn socket_path_for(config_dir: &Path) -> PathBuf {
        use std::os::unix::ffi::OsStrExt as _;
        let inline = config_dir.join("daemon.sock");
        if inline.as_os_str().as_bytes().len() <= MAX_SOCKET_PATH_BYTES {
            return inline;
        }
        let hash = fnv1a64(config_dir.as_os_str().as_bytes());
        let name = format!("tty7-{hash:016x}.sock");
        let xdg = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from);
        pick_fallback_socket(xdg.as_deref(), &std::env::temp_dir(), &name)
    }

    pub(super) fn pick_fallback_socket(xdg: Option<&Path>, temp: &Path, name: &str) -> PathBuf {
        use std::os::unix::ffi::OsStrExt as _;
        let fits = |p: &PathBuf| p.as_os_str().as_bytes().len() <= MAX_SOCKET_PATH_BYTES;
        let preferred = xdg.unwrap_or(temp).join(name);
        if fits(&preferred) {
            return preferred;
        }
        let temp_path = temp.join(name);
        if fits(&temp_path) {
            return temp_path;
        }
        preferred
    }

    fn socket_path() -> Option<PathBuf> {
        Some(socket_path_for(&config::config_dir_path()?))
    }

    pub fn connect() -> io::Result<Stream> {
        let path = socket_path().ok_or_else(|| {
            io::Error::other("could not resolve daemon socket path (no config dir)")
        })?;
        connect_endpoint_at(&path)
    }

    pub fn connect_endpoint_at(path: &Path) -> io::Result<Stream> {
        let stream = UnixStream::connect(path)?;
        tune(&stream);
        Ok(stream)
    }

    pub fn tune(stream: &Stream) {
        use std::os::unix::io::AsRawFd as _;
        let size: libc::c_int = 256 * 1024;
        for opt in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
            unsafe {
                libc::setsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    opt,
                    (&raw const size).cast(),
                    size_of::<libc::c_int>() as libc::socklen_t,
                );
            }
        }
    }

    #[inline]
    pub fn authenticate(_stream: &mut Stream) -> io::Result<()> {
        Ok(())
    }

    pub fn endpoint_exists() -> bool {
        socket_path().is_some_and(|p| p.exists())
    }

    pub fn remove_stale_endpoint() {
        if let Some(path) = socket_path() {
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn bind() -> anyhow::Result<Listener> {
        use std::os::unix::fs::PermissionsExt as _;
        let path = socket_path().ok_or_else(|| {
            anyhow::anyhow!("could not resolve daemon socket path (no config dir)")
        })?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
            let owns_parent = config::config_dir_path().is_some_and(|c| c.as_path() == parent);
            if owns_parent {
                if let Err(e) =
                    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                {
                    log::warn!(
                        "could not chmod 0700 daemon socket dir {}: {e}",
                        parent.display()
                    );
                }
            }
        }
        let listener = UnixListener::bind(&path)
            .map_err(|e| anyhow::anyhow!("bind {} failed: {}", path.display(), e))?;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            log::warn!("could not chmod 0600 daemon socket {}: {e}", path.display());
        }
        Ok(listener)
    }

    pub fn endpoint_display() -> String {
        socket_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unresolved>".to_string())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The config directory is process-global and so is the one socket path
    /// under it, so every test that binds it has to take a turn. Without this
    /// they race each other into `bind` and fail on `EEXIST`.
    fn pin_config_dir() -> std::sync::MutexGuard<'static, ()> {
        static SOCKET: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = SOCKET.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("tty7-covtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        config::set_config_dir(dir);
        remove_stale_endpoint();
        guard
    }

    #[test]
    fn endpoint_lifecycle_bind_connect_and_clear() {
        let _turn = pin_config_dir();
        assert!(!endpoint_exists(), "no endpoint before bind");

        let listener = bind().expect("bind should succeed under the temp config dir");
        assert!(endpoint_exists(), "the socket file marks the endpoint");
        assert!(
            endpoint_display().contains("daemon.sock"),
            "display names the socket file"
        );

        let _client = connect().expect("connect to the live listener");

        drop(listener);
        remove_stale_endpoint();
        assert!(!endpoint_exists(), "endpoint cleared after removal");
    }

    /// A daemon that died without unlinking its socket leaves a file `bind`
    /// refuses, so every later daemon exits on the bind and the client
    /// launches another, forever. Holding the seat is what says the file is a
    /// leftover: nobody else can be the server while we are.
    #[test]
    fn holding_the_seat_clears_a_socket_its_daemon_never_unlinked() {
        let _turn = pin_config_dir();

        let dead = bind().expect("bind under the temp config dir");
        drop(dead);
        assert!(endpoint_exists(), "the file outlives the listener");

        // And its pidfile still names it. That pair is what a daemon that
        // died leaves behind, and it is the state the clearing used to skip:
        // "the recorded daemon is gone, so let the bind overwrite the file" is
        // true of a Windows port file and false of a Unix socket.
        let pidfile = crate::daemon::pidfile::path().expect("a pinned config dir has one");
        std::fs::write(&pidfile, dead_pid().to_string()).expect("plant it");
        assert!(
            crate::daemon::spawn::recorded_daemon_is_dead(),
            "the recorded daemon is gone, which is the whole condition"
        );

        clear_endpoint_before_bind(true).expect("a leftover is not a refusal");
        assert!(!endpoint_exists(), "and it is gone before the bind sees it");
        let listener = bind().expect("so the next daemon starts");
        let _client = connect().expect("and is reachable");

        drop(listener);
        remove_stale_endpoint();
        let _ = std::fs::remove_file(pidfile);
    }

    /// A pid nothing on this machine is using.
    fn dead_pid() -> u32 {
        let mut pid = 200_000u32;
        while unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
            pid += 1;
        }
        pid
    }

    /// Without a seat there is nothing to reason from, so the endpoint is
    /// asked instead — and one that answers is refused, never removed.
    /// Removing on a failed connect is the race `singleton` exists to kill;
    /// doing it to a socket that *did* answer would be that race with the
    /// evidence pointing the other way.
    #[test]
    fn without_a_seat_a_live_endpoint_is_refused_and_a_dead_one_cleared() {
        let _turn = pin_config_dir();

        let live = bind().expect("bind under the temp config dir");
        let err = clear_endpoint_before_bind(false).expect_err("someone answers there");
        assert!(
            err.to_string().contains("daemon already running"),
            "and is named rather than unlinked: {err}"
        );
        let _client = connect().expect("the listener still owns its endpoint");

        drop(live);
        assert!(endpoint_exists(), "the file outlives the listener");
        clear_endpoint_before_bind(false).expect("now nothing answers");
        assert!(!endpoint_exists(), "so it comes off");
    }

    #[test]
    fn socket_path_stays_in_config_dir_when_it_fits() {
        let dir = std::path::PathBuf::from("/tmp/tty7-short");
        assert_eq!(imp_unix::socket_path_for(&dir), dir.join("daemon.sock"));
    }

    #[test]
    fn socket_path_falls_back_when_config_dir_is_too_long() {
        use std::os::unix::ffi::OsStrExt as _;
        let long_a = std::path::PathBuf::from(format!("/tmp/{}", "a".repeat(150)));
        let long_b = std::path::PathBuf::from(format!("/tmp/{}", "b".repeat(150)));

        let path = imp_unix::socket_path_for(&long_a);
        assert!(
            path.as_os_str().as_bytes().len() <= imp_unix::MAX_SOCKET_PATH_BYTES,
            "fallback path must fit sun_path: {}",
            path.display()
        );
        assert_eq!(
            path,
            imp_unix::socket_path_for(&long_a),
            "GUI and daemon must derive the same endpoint"
        );
        assert_ne!(
            path,
            imp_unix::socket_path_for(&long_b),
            "distinct config dirs keep distinct daemons"
        );
    }

    #[test]
    fn fallback_socket_binds_and_connects() {
        use std::os::unix::net::{UnixListener, UnixStream};
        let long_dir =
            std::env::temp_dir().join(format!("{}-{}", "x".repeat(120), std::process::id()));
        let path = imp_unix::socket_path_for(&long_dir);
        assert_ne!(
            path.parent(),
            Some(long_dir.as_path()),
            "must not live in the long dir"
        );

        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind fallback socket");
        let _client = UnixStream::connect(&path).expect("connect fallback socket");
        drop(listener);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_long_runtime_dir_falls_through_to_the_temp_dir() {
        let name = "tty7-0123456789abcdef.sock";
        let long_xdg = PathBuf::from(format!("/run/user/1000/{}", "d".repeat(90)));
        let temp = PathBuf::from("/tmp");

        let picked = imp_unix::pick_fallback_socket(Some(&long_xdg), &temp, name);
        assert_eq!(picked, temp.join(name), "falls through to the temp dir");

        let short_xdg = PathBuf::from("/run/user/1000");
        assert_eq!(
            imp_unix::pick_fallback_socket(Some(&short_xdg), &temp, name),
            short_xdg.join(name),
            "$XDG_RUNTIME_DIR still wins whenever it fits",
        );
        assert_eq!(
            imp_unix::pick_fallback_socket(None, &temp, name),
            temp.join(name),
            "no runtime dir means the temp dir, as before",
        );
    }

    #[test]
    fn an_unusable_pair_of_bases_still_reports_the_preferred_path() {
        let name = "tty7-0123456789abcdef.sock";
        let long_xdg = PathBuf::from(format!("/run/{}", "d".repeat(90)));
        let long_temp = PathBuf::from(format!("/tmp/{}", "t".repeat(90)));
        assert_eq!(
            imp_unix::pick_fallback_socket(Some(&long_xdg), &long_temp, name),
            long_xdg.join(name),
        );
    }
}
