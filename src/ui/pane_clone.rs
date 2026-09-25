//! Rebuild a pane's connection for Copy Tab / split-as-clone.
//!
//! Goal: same host + same cwd when known; reuse Native / shell-ssh argv; never
//! require downloading xtty-server on the far side. Missing cwd still reconnects
//! the host — the new shell lands in the remote default home.

use std::path::{Path, PathBuf};

use crate::daemon::protocol::{NativeSshSpec, RemoteKind};
use crate::terminal::view::TerminalView;
use crate::ui::app::SpawnAs;

/// What to spawn, plus optional lines typed after the pane is up.
#[derive(Clone)]
pub(crate) struct PaneClonePlan {
    pub spawn: SpawnAs,
    /// Local / workspace spawn cwd. Native cwd goes into `login_script` instead.
    pub cwd: Option<PathBuf>,
    pub follow_up: Vec<String>,
}

pub(crate) fn plan_for(view: &TerminalView) -> PaneClonePlan {
    let far_cwd = view.cwd();

    // Process-table nested ssh/jumper: local shell, then re-run the hop argv.
    if let Some(argv) = view.nested_ssh_argv() {
        return PaneClonePlan {
            spawn: SpawnAs::Shell(view.shell_spec()),
            // Far cwd is not a path on this machine — do not pass it as spawn cwd.
            cwd: None,
            follow_up: vec![ssh_landing_command(&argv, far_cwd.as_deref())],
        };
    }

    // Native SSH (optionally with an interactive hop still in last_ssh_command).
    if let Some(mut spec) = view.ssh_spec() {
        let mut follow_up = Vec::new();
        if let Some(hop) = view.nested_ssh_command() {
            follow_up.push(ssh_command_landing(&hop, far_cwd.as_deref()));
        } else if let Some(cwd) = far_cwd.as_ref() {
            push_cd_login(&mut spec, cwd);
        }
        return PaneClonePlan {
            spawn: SpawnAs::Ssh(spec),
            cwd: None,
            follow_up,
        };
    }

    // Local pane or remote-workspace pane (daemon-local on the far machine).
    let cwd = view.spawnable_cwd().or_else(|| {
        // Workspace panes report paths on their host; spawnable_cwd allows them
        // when remote_context is none.
        far_cwd.clone()
    });
    PaneClonePlan {
        spawn: SpawnAs::Shell(view.shell_spec()),
        cwd,
        follow_up: Vec::new(),
    }
}

fn push_cd_login(spec: &mut NativeSshSpec, cwd: &Path) {
    let path = cwd.to_string_lossy();
    if path.is_empty() {
        return;
    }
    // Absolute / home-relative only — relative scraps from a partial probe
    // would land in the wrong place on a fresh login shell.
    if !(path.starts_with('/') || path.starts_with('~') || looks_like_windows_abs(&path)) {
        return;
    }
    spec.login_script
        .push(format!("cd {}", posix_single_quote(&path)));
}

fn looks_like_windows_abs(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

fn posix_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn join_argv_for_typing(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty()
                || a.chars()
                    .any(|c| c.is_whitespace() || "\"'\\$`|&;<>()".contains(c))
            {
                posix_single_quote(a)
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rebuild `ssh …` and optionally force a login shell in `cwd` via `-t`.
fn ssh_landing_command(argv: &[String], cwd: Option<&Path>) -> String {
    let base = join_argv_for_typing(argv);
    let Some(cwd) = cwd else {
        return base;
    };
    let path = cwd.to_string_lossy();
    if path.is_empty() {
        return base;
    }
    // Avoid stacking -t when the recorded argv already requested a tty / remote command.
    if argv.iter().any(|a| a == "-t" || a == "-tt") {
        return base;
    }
    let q = posix_single_quote(&path);
    format!("{base} -t 'cd {q} && exec \"${{SHELL:-bash}}\" -l'")
}

fn ssh_command_landing(cmd: &str, cwd: Option<&Path>) -> String {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let Some(cwd) = cwd else {
        return trimmed.to_string();
    };
    let path = cwd.to_string_lossy();
    if path.is_empty() || trimmed.contains(" -t ") || trimmed.ends_with(" -t") {
        return trimmed.to_string();
    }
    let q = posix_single_quote(&path);
    format!("{trimmed} -t 'cd {q} && exec \"${{SHELL:-bash}}\" -l'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_appends_cd_via_t() {
        let argv = vec!["ssh".into(), "carol@box".into()];
        let cmd = ssh_landing_command(&argv, Some(Path::new("/home/carol/src")));
        assert!(cmd.starts_with("ssh carol@box -t "));
        assert!(cmd.contains("/home/carol/src"));
    }

    #[test]
    fn landing_skips_cd_when_t_present() {
        let argv = vec!["ssh".into(), "-t".into(), "carol@box".into()];
        let cmd = ssh_landing_command(&argv, Some(Path::new("/tmp")));
        assert_eq!(cmd, "ssh -t carol@box");
    }

    #[test]
    fn login_script_cd_quotes_spaces() {
        let mut spec: NativeSshSpec = serde_json::from_str(
            r#"{"host":"h","port":22,"user":"u","auth_mode":"auto"}"#,
        )
        .unwrap();
        push_cd_login(&mut spec, Path::new("/tmp/a b"));
        assert_eq!(spec.login_script, vec!["cd '/tmp/a b'".to_string()]);
    }
}
