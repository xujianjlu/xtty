//! Rebuild a pane's connection for Copy Tab / split-as-clone.
//!
//! Three cases, now that Native SSH is gone:
//!
//! 1. Local pane — respawn the same shell in the same cwd.
//! 2. Local shell that hopped with `ssh` / jumper — respawn the local shell,
//!    then type the hop (with `cd` when known) after the first prompt.
//! 3. Host-picker / `+` OpenSSH pane (`program` is `ssh`) — respawn that same
//!    argv and bake the far cwd into `ssh -t`, never type a second `ssh`.

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

pub(crate) fn plan_from_facts(facts: CloneFacts) -> PaneClonePlan {
    let far_cwd = facts.far_cwd.as_deref();

    if let Some(spec) = facts.shell.as_ref().filter(|s| is_ssh_program(&s.program)) {
        // Host-picker ssh, then a further hop: keep the outer argv, type the inner.
        if let Some(inner) = facts
            .nested_ssh_argv
            .as_ref()
            .filter(|argv| !same_ssh_invocation(spec, argv))
        {
            return PaneClonePlan {
                spawn: SpawnAs::Shell(Some(spec.clone())),
                cwd: None,
                follow_up: vec![ssh_landing_command(inner, far_cwd)],
            };
        }
        return PaneClonePlan {
            spawn: SpawnAs::Shell(Some(ssh_spec_landing_in(spec.clone(), far_cwd))),
            cwd: None,
            follow_up: Vec::new(),
        };
    }

    if let Some(argv) = facts.nested_ssh_argv {
        return PaneClonePlan {
            spawn: SpawnAs::Shell(facts.shell),
            cwd: None,
            follow_up: vec![ssh_landing_command(&argv, far_cwd)],
        };
    }

    let cwd = facts.spawnable_cwd.or(facts.far_cwd);
    PaneClonePlan {
        spawn: SpawnAs::Shell(facts.shell),
        cwd,
        follow_up: Vec::new(),
    }
}

fn is_ssh_program(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "ssh")
}

fn same_ssh_invocation(spec: &ShellSpec, argv: &[String]) -> bool {
    let Some((prog, rest)) = argv.split_first() else {
        return false;
    };
    is_ssh_program(prog) && rest == spec.args.as_slice()
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

fn ssh_args_landing_in(mut args: Vec<String>, cwd: Option<&Path>) -> Vec<String> {
    let Some(cwd) = cwd else {
        return args;
    };
    let path = cwd.to_string_lossy();
    if path.is_empty() || argv_has_tty_request(&args) {
        return args;
    }
    args.push("-t".into());
    args.push(format!(
        "cd {} && exec \"${{SHELL:-bash}}\" -l",
        posix_single_quote(&path)
    ));
    args
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

/// Rebuild `ssh …` for typing into a local shell, optionally with `-t` + `cd`.
fn ssh_landing_command(argv: &[String], cwd: Option<&Path>) -> String {
    let Some((prog, rest)) = argv.split_first() else {
        return String::new();
    };
    let mut typed = vec![prog.clone()];
    typed.extend(ssh_args_landing_in(rest.to_vec(), cwd));
    join_argv_for_typing(&typed)
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

    #[test]
    fn in_shell_hop_types_ssh_after_local_shell() {
        let plan = plan_from_facts(CloneFacts {
            shell: Some(zsh()),
            far_cwd: Some(PathBuf::from("/home/carol/src")),
            spawnable_cwd: None,
            nested_ssh_argv: Some(vec!["ssh".into(), "carol@box".into()]),
        });
        assert_eq!(plan.spawn, SpawnAs::Shell(Some(zsh())));
        assert_eq!(plan.cwd, None);
        assert_eq!(plan.follow_up.len(), 1);
        assert!(plan.follow_up[0].starts_with("ssh carol@box -t "));
        assert!(plan.follow_up[0].contains("/home/carol/src"));
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
        match plan.spawn {
            SpawnAs::Shell(Some(spec)) => {
                assert_eq!(spec.program, "ssh");
                assert!(spec.args.windows(2).any(|w| {
                    w[0] == "-t" && w[1].contains("/home/carol/src")
                }));
            }
        }
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
        match plan.spawn {
            SpawnAs::Shell(Some(spec)) => {
                assert_eq!(spec.program, "ssh");
                assert!(spec.args.contains(&"-t".to_string()));
            }
        }
        assert!(plan.follow_up.is_empty());
    }

    #[test]
    fn host_picker_then_inner_hop_types_only_the_inner_ssh() {
        let plan = plan_from_facts(CloneFacts {
            shell: Some(host_ssh()),
            far_cwd: Some(PathBuf::from("/srv/app")),
            spawnable_cwd: None,
            nested_ssh_argv: Some(vec!["ssh".into(), "inner@db".into()]),
        });
        assert_eq!(plan.spawn, SpawnAs::Shell(Some(host_ssh())));
        assert_eq!(plan.follow_up.len(), 1);
        assert!(plan.follow_up[0].starts_with("ssh inner@db -t "));
        assert!(!plan.follow_up[0].contains("carol@box"));
    }

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
}
