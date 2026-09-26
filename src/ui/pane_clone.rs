//! Rebuild a pane's connection for Copy Tab / split-as-clone.
//!
//! Two cases:
//!
//! 1. Local pane — respawn the same shell in the same cwd.
//! 2. Any SSH hop (`+`, typed `ssh`, jumper, further hop) — spawn system
//!    `ssh` to the current dest with SI bootstrap and `cd`, same as `+`.
//!    Never respawn the local shell and type `ssh`.

use std::path::{Path, PathBuf};

use crate::daemon::protocol::ShellSpec;
use crate::terminal::view::TerminalView;
use crate::ui::app::SpawnAs;

/// What to spawn, plus optional lines typed after the pane is up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneClonePlan {
    pub spawn: SpawnAs,
    /// Local / workspace spawn cwd. Far-side paths never go here.
    pub cwd: Option<PathBuf>,
    pub follow_up: Vec<String>,
}

/// Snapshot `plan_for` reads off a live view. Tests build this directly.
#[derive(Clone, Debug, Default)]
pub(crate) struct CloneFacts {
    pub shell: Option<ShellSpec>,
    pub far_cwd: Option<PathBuf>,
    pub spawnable_cwd: Option<PathBuf>,
    pub nested_ssh_argv: Option<Vec<String>>,
}

pub(crate) fn plan_for(view: &TerminalView) -> PaneClonePlan {
    plan_from_facts(CloneFacts {
        shell: view.shell_spec(),
        far_cwd: view.cwd(),
        spawnable_cwd: view.spawnable_cwd(),
        nested_ssh_argv: view.nested_ssh_argv(),
    })
}

fn cloneable_far_cwd(path: Option<&Path>) -> Option<&Path> {
    let path = path?;
    let s = path.to_string_lossy();
    let ok = (s.starts_with('/') || s == "~" || s.starts_with("~/"))
        && !s.contains('$')
        && !s.contains(']')
        && !s.contains('[')
        && !s.chars().any(char::is_whitespace);
    ok.then_some(path)
}

pub(crate) fn plan_from_facts(facts: CloneFacts) -> PaneClonePlan {
    let far_cwd = cloneable_far_cwd(facts.far_cwd.as_deref());

    if let Some(argv) = facts.nested_ssh_argv {
        return PaneClonePlan {
            spawn: SpawnAs::Shell(Some(ssh_spec_from_argv(argv, far_cwd))),
            cwd: None,
            follow_up: Vec::new(),
        };
    }

    if let Some(spec) = facts.shell {
        if is_ssh_program(&spec.program) {
            return PaneClonePlan {
                spawn: SpawnAs::Shell(Some(ssh_spec_landing_in(spec, far_cwd))),
                cwd: None,
                follow_up: Vec::new(),
            };
        }
        let cwd = facts.spawnable_cwd.or(facts.far_cwd);
        return PaneClonePlan {
            spawn: SpawnAs::Shell(Some(spec)),
            cwd,
            follow_up: Vec::new(),
        };
    }

    let cwd = facts.spawnable_cwd.or(facts.far_cwd);
    PaneClonePlan {
        spawn: SpawnAs::Shell(None),
        cwd,
        follow_up: Vec::new(),
    }
}

fn ssh_spec_from_argv(argv: Vec<String>, cwd: Option<&Path>) -> ShellSpec {
    let (program, args) = match argv.split_first() {
        Some((prog, rest)) if is_ssh_program(prog) => (prog.clone(), rest.to_vec()),
        _ => ("ssh".into(), argv),
    };
    ssh_spec_landing_in(
        ShellSpec {
            program,
            args,
            args_are_tty7_defaults: false,
        },
        cwd,
    )
}

fn is_ssh_program(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "ssh")
}

fn argv_has_tty_request(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "-t" || a == "-tt")
}

/// Same OpenSSH argv, with far cwd forced via a trailing `-t` remote command.
fn ssh_spec_landing_in(mut spec: ShellSpec, cwd: Option<&Path>) -> ShellSpec {
    spec.args = ssh_args_landing_in(std::mem::take(&mut spec.args), cwd);
    spec.args_are_tty7_defaults = false;
    spec
}

fn ssh_args_landing_in(args: Vec<String>, cwd: Option<&Path>) -> Vec<String> {
    let mut full = vec!["ssh".into()];
    full.extend(args.iter().cloned());
    let Some(through) = crate::daemon::ssh_argv_through_dest(&full) else {
        return args;
    };
    let existing = crate::daemon::ssh_remote_command(&full);
    if cwd.is_none() && existing.is_none() {
        return args;
    }
    let mut out: Vec<String> = through.into_iter().skip(1).collect();
    if !argv_has_tty_request(&out) {
        out.insert(0, "-t".into());
    }
    out.push(landing_remote_command(cwd, existing.as_deref()));
    out
}

fn strip_leading_cd(cmd: &str) -> &str {
    let rest = cmd.trim_start();
    if !rest.starts_with("cd ") {
        return cmd;
    }
    match rest.find(" && ") {
        Some(i) => &rest[i + 4..],
        None => cmd,
    }
}

fn looks_like_si_bootstrap(cmd: &str) -> bool {
    cmd.contains("TTY7_SI_DIR") || cmd.contains("tty7-si-") || cmd.contains("TTY7_SSH_BOOT")
}

/// Far-side remote command: `cd` + login shell. Same as typing
/// `ssh -t dest 'cd … && exec $SHELL -l'` — no SI bootstrap.
fn landing_remote_command(cwd: Option<&Path>, existing: Option<&str>) -> String {
    let rest = match existing.filter(|cmd| !looks_like_si_bootstrap(cmd)) {
        Some(cmd) => strip_leading_cd(cmd).to_string(),
        None => "exec ${SHELL:-/bin/sh} -l".to_string(),
    };
    match cwd.map(|p| p.to_string_lossy()).filter(|p| !p.is_empty()) {
        Some(path) if path == "~" || (path.starts_with("~/") && !path.contains('\'')) => {
            format!("cd {path} && {rest}")
        }
        Some(path) => format!("cd {} && {}", posix_single_quote(&path), rest),
        None => rest,
    }
}

fn posix_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zsh() -> ShellSpec {
        ShellSpec {
            program: "/bin/zsh".into(),
            args: vec!["-l".into()],
            args_are_tty7_defaults: true,
        }
    }

    fn host_ssh() -> ShellSpec {
        ShellSpec {
            program: "ssh".into(),
            args: vec!["-p".into(), "22".into(), "carol@box".into()],
            args_are_tty7_defaults: false,
        }
    }

    fn leftover_si_remote() -> String {
        "export TTY7_SI_DIR=/tmp/tty7-si-old TTY7_SSH_BOOT=/tmp/tty7-si-old/boot.sh".into()
    }

    #[test]
    fn garbled_ps1_cwd_is_not_baked_into_ssh() {
        let plan = plan_from_facts(CloneFacts {
            shell: Some(host_ssh()),
            far_cwd: Some(PathBuf::from("~]$cd CODE")),
            spawnable_cwd: None,
            nested_ssh_argv: Some(vec!["ssh".into(), "carol@box".into()]),
        });
        let SpawnAs::Shell(Some(spec)) = plan.spawn else {
            panic!("ssh");
        };
        let remote = spec.args.last().cloned().unwrap_or_default();
        assert!(
            !remote.contains("~]$"),
            "must not ssh with the typed command as cwd: {remote}"
        );
    }

    #[test]
    fn local_pane_keeps_shell_and_cwd() {
        let plan = plan_from_facts(CloneFacts {
            shell: Some(zsh()),
            far_cwd: Some(PathBuf::from("/Users/me/src")),
            spawnable_cwd: Some(PathBuf::from("/Users/me/src")),
            nested_ssh_argv: None,
        });
        assert_eq!(plan.spawn, SpawnAs::Shell(Some(zsh())));
        assert_eq!(plan.cwd.as_deref(), Some(Path::new("/Users/me/src")));
        assert!(plan.follow_up.is_empty());
    }

    fn assert_direct_ssh(plan: &PaneClonePlan, dest: &str, cwd: &str) {
        assert!(
            plan.follow_up.is_empty(),
            "direct ssh is spawned, not typed"
        );
        assert_eq!(plan.cwd, None);
        let SpawnAs::Shell(Some(spec)) = &plan.spawn else {
            panic!("SSH hop must spawn ssh, not a local shell");
        };
        assert!(is_ssh_program(&spec.program));
        assert!(
            spec.args.iter().any(|a| a == dest),
            "dest {dest} missing from {:?}",
            spec.args
        );
        let remote = spec.args.last().expect("cd remote command");
        assert!(remote.contains(cwd), "{remote}");
        assert!(
            remote.contains("exec ${SHELL:-/bin/sh} -l"),
            "plain login shell, not SI bootstrap: {remote}"
        );
        assert!(
            !remote.contains("TTY7_SI_DIR"),
            "clone must not inject a second SI path: {remote}"
        );
    }

    #[test]
    fn typed_hop_clones_as_direct_ssh() {
        let plan = plan_from_facts(CloneFacts {
            shell: Some(zsh()),
            far_cwd: Some(PathBuf::from("/home/carol/src")),
            spawnable_cwd: None,
            nested_ssh_argv: Some(vec!["ssh".into(), "carol@box".into()]),
        });
        assert_direct_ssh(&plan, "carol@box", "/home/carol/src");
    }

    #[test]
    fn host_picker_ssh_bakes_cwd_into_argv() {
        let plan = plan_from_facts(CloneFacts {
            shell: Some(host_ssh()),
            far_cwd: Some(PathBuf::from("/home/carol/src")),
            spawnable_cwd: None,
            nested_ssh_argv: Some(vec![
                "/usr/bin/ssh".into(),
                "-p".into(),
                "22".into(),
                "carol@box".into(),
            ]),
        });
        let SpawnAs::Shell(Some(spec)) = plan.spawn else {
            panic!("host-picker clone must respawn ssh");
        };
        assert_eq!(spec.program, "ssh");
        assert!(spec.args.contains(&"-t".to_string()));
        assert!(
            spec.args.last().is_some_and(|c| {
                c.contains("/home/carol/src") && c.contains("exec ${SHELL:-/bin/sh} -l")
            }),
            "far cwd + login shell must be the remote command"
        );
        assert_eq!(plan.cwd, None);
        assert!(
            plan.follow_up.is_empty(),
            "must not type a second ssh into the session"
        );
    }

    #[test]
    fn host_picker_ssh_without_probe_still_respawns_ssh() {
        let plan = plan_from_facts(CloneFacts {
            shell: Some(host_ssh()),
            far_cwd: Some(PathBuf::from("/var/tmp")),
            spawnable_cwd: None,
            nested_ssh_argv: None,
        });
        let SpawnAs::Shell(Some(spec)) = plan.spawn else {
            panic!("host-picker clone must respawn ssh");
        };
        assert_eq!(spec.program, "ssh");
        assert!(spec.args.contains(&"-t".to_string()));
        assert!(plan.follow_up.is_empty());
    }

    #[test]
    fn further_hop_clones_as_direct_ssh_to_current_dest() {
        let plan = plan_from_facts(CloneFacts {
            shell: Some(host_ssh()),
            far_cwd: Some(PathBuf::from("/srv/app")),
            spawnable_cwd: None,
            nested_ssh_argv: Some(vec!["ssh".into(), "inner@db".into()]),
        });
        assert_direct_ssh(&plan, "inner@db", "/srv/app");
        let SpawnAs::Shell(Some(spec)) = &plan.spawn else {
            panic!("direct ssh");
        };
        assert!(
            !spec.args.iter().any(|a| a.contains("carol@box")),
            "must dial the current dest, not replay the jumper"
        );
    }

    #[test]
    fn host_picker_same_dest_does_not_retype_ssh() {
        let mut spec = host_ssh();
        spec.args = vec!["-t".into(), "carol@box".into(), leftover_si_remote()];
        let plan = plan_from_facts(CloneFacts {
            shell: Some(spec),
            far_cwd: Some(PathBuf::from("/home/carol/src")),
            spawnable_cwd: None,
            nested_ssh_argv: Some(vec!["ssh".into(), "-t".into(), "carol@box".into()]),
        });
        assert!(
            plan.follow_up.is_empty(),
            "same dest is a respawn, not a second hop typed into dest"
        );
        let SpawnAs::Shell(Some(s)) = plan.spawn else {
            panic!("same-dest clone must respawn ssh");
        };
        let remote = s.args.last().expect("baked remote command");
        assert!(remote.contains("/home/carol/src"));
        assert!(remote.contains("exec ${SHELL:-/bin/sh} -l"));
        assert!(!remote.contains("TTY7_SI_DIR"));
    }

    #[test]
    fn host_picker_rebakes_cd_over_existing_bootstrap() {
        let spec = ShellSpec {
            program: "ssh".into(),
            args: vec!["-t".into(), "carol@box".into(), leftover_si_remote()],
            args_are_tty7_defaults: false,
        };
        let landed = ssh_args_landing_in(spec.args, Some(Path::new("/srv/app")));
        let remote = landed.last().expect("remote command");
        assert!(
            remote.starts_with("cd '/srv/app' && "),
            "cd must wrap the login shell, not be skipped because -t is present: {remote}"
        );
        assert!(
            remote.contains("exec ${SHELL:-/bin/sh} -l"),
            "leftover SI bootstrap is dropped: {remote}"
        );
        assert_eq!(
            remote.matches("cd ").count(),
            1,
            "must not stack leftover cds"
        );
    }
}
