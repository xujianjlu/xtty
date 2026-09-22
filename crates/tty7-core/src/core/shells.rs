use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedShell {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    /// Marks arguments supplied by tty7's shell detection rather than by the user.
    /// Older peers omit this field, and their inventory arguments were all treated
    /// as tty7 defaults, so `true` preserves the previous protocol behavior.
    #[serde(default = "default_true")]
    pub args_are_tty7_defaults: bool,
    /// Marks a row the user wrote into `custom_shells` rather than one tty7
    /// found. Detection is what Settings offers to stand in for the platform
    /// default; an entry the user added is a menu extra, and a picker that
    /// carries only a program would quietly drop the arguments that make it
    /// what it is. Older peers omit this field and had no such rows, so `false`
    /// is the honest reading of their inventory.
    #[serde(default)]
    pub user_authored: bool,
}

impl DetectedShell {
    fn bare(label: impl Into<String>, program: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            program: program.into(),
            args: Vec::new(),
            args_are_tty7_defaults: true,
            user_authored: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellInventory {
    pub shells: Vec<DetectedShell>,
    pub default_name: String,
}

pub fn inventory() -> ShellInventory {
    // One read for both halves: `shell_command()` would open and parse
    // `config.json` a second time, and this runs every time the menu is built.
    let config = crate::core::config::Config::load();
    let configured = config
        .shell
        .as_ref()
        .map(|s| (s.program.clone(), s.args.clone()));
    let mut inventory = inventory_from(detect_shells(), configured, &login_shell());
    append_custom(&mut inventory, &config.custom_shells);
    inventory
}

/// Adds the user's own menu entries after everything that was detected.
///
/// After, not among: the detected list is ordered so the shell a new tab
/// actually opens with sits at the top, and that ordering is the menu's answer
/// to "what do I get if I just click". A user-authored entry is an extra, not a
/// candidate for that position — so this runs once the default has already been
/// settled and cannot move it.
fn append_custom(inventory: &mut ShellInventory, custom: &[crate::core::config::CustomShell]) {
    for (index, entry) in custom.iter().enumerate() {
        let program = entry.program.trim();
        if program.is_empty() {
            // Nothing to launch. The rest of the entry may be perfectly well
            // formed, but a menu row that opens nothing is worse than no row.
            // Say which one, though: a misspelled key deserializes to an entry
            // that is simply empty, and a row that never appears is otherwise
            // indistinguishable from the feature not working.
            log::warn!("custom_shells[{index}] has no program; skipping it");
            continue;
        }
        let mut label = entry.label.trim().to_string();
        if label.is_empty() {
            label = basename(program);
        }
        // The menu marks its default row by matching the label
        // (`tab_strip::shell_menu`), so a custom entry that borrows a name
        // already in the list would wear a mark meant for another row. Same
        // answer the configured shell already gets when it collides — and it
        // has to keep answering until the name is actually free, since the
        // suffixed name can collide in its turn with a second entry that
        // borrowed the same one, or with a label the user wrote suffix and
        // all. `default_name` counts even when no row carries it: a hole
        // `inventory_from` can leave, and the one name a custom row must never
        // occupy.
        while inventory.default_name == label
            || inventory.shells.iter().any(|shell| shell.label == label)
        {
            label.push_str(" (Custom)");
        }
        inventory.shells.push(DetectedShell {
            label,
            program: program.to_string(),
            args: entry.args.clone(),
            // tty7 chose none of this, so none of it is a tty7 default: the
            // arguments have to survive into the pane exactly as written.
            args_are_tty7_defaults: false,
            user_authored: true,
        });
    }
}

pub fn detect_shells() -> Vec<DetectedShell> {
    detect_unix()
}

pub fn default_shell_name(configured: Option<&str>) -> String {
    let program = match configured {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => login_shell(),
    };
    basename(&program)
}

/// Combines the detected shells with the explicit shell from Settings.
///
/// A bare configured command such as `nu` matches the detected executable with
/// the same basename because both resolve through PATH. Explicit paths only
/// match the same path, so choosing a second installation of the same shell
/// still creates a useful, distinct menu entry. The detected label wins for a
/// match, preserving friendly names such as "Nushell" and "PowerShell 7".
fn inventory_from(
    mut shells: Vec<DetectedShell>,
    configured: Option<(String, Vec<String>)>,
    fallback_program: &str,
) -> ShellInventory {
    let configured = configured.filter(|(program, _)| !program.trim().is_empty());
    let default_program = configured
        .as_ref()
        .map_or(fallback_program, |(program, _)| program.as_str());

    if let Some(default_index) = shells
        .iter()
        .position(|shell| same_shell_program(&shell.program, default_program))
    {
        // A configured command may resolve to an already detected executable.
        // Keep the detected friendly label, but retain the user's command,
        // launch arguments, and their origin so local and remote menus behave
        // identically.
        if let Some((program, args)) = configured.as_ref() {
            // Keep a bare command bare: it must continue to resolve through PATH.
            // The detected entry can be the login shell (which intentionally wins
            // inventory deduplication), and that absolute path is not necessarily
            // the executable the configured bare command would resolve to.
            shells[default_index].program.clone_from(program);
            shells[default_index].args.clone_from(args);
            shells[default_index].args_are_tty7_defaults = false;
        }
        let default_name = shells[default_index].label.clone();
        return ShellInventory {
            shells,
            default_name,
        };
    }

    let mut default_name = basename(default_program);
    if let Some((program, args)) = configured {
        if shells.iter().any(|shell| shell.label == default_name) {
            default_name.push_str(" (Configured)");
        }
        // The configured shell is the platform default, so keep it at the top
        // just like the detected login shell while retaining its custom args.
        shells.insert(
            0,
            DetectedShell {
                label: default_name.clone(),
                program,
                args,
                args_are_tty7_defaults: false,
                user_authored: false,
            },
        );
    }

    ShellInventory {
        shells,
        default_name,
    }
}

/// Compares executable identities without collapsing two explicit installs.
fn same_shell_program(detected: &str, configured: &str) -> bool {
    let detected = detected.trim();
    let configured = configured.trim();
    if detected.is_empty() || configured.is_empty() {
        return false;
    }

    let detected_path = Path::new(detected);
    let configured_path = Path::new(configured);
    if is_bare_program(configured_path) {
        return basename(detected) == basename(configured);
    }
    if is_bare_program(detected_path) {
        return false;
    }

    comparable_program_path(detected_path) == comparable_program_path(configured_path)
}

fn is_bare_program(program: &Path) -> bool {
    program.components().count() <= 1
}

/// Canonicalization folds harmless `.` and `..` differences when the target
/// exists. The textual fallback still gives stable behavior for configured
/// paths that have not been installed yet.
fn comparable_program_path(program: &Path) -> String {
    let resolved = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    resolved.to_string_lossy().into_owned()
}

/// The user's login shell, straight from the passwd database.
///
/// `$SHELL` is a snapshot taken when the session logged in, so `chsh` does not
/// move it — a GUI launch inherits whatever was current at login and keeps
/// reporting it until the user logs out. passwd is the live value; `$SHELL` is
/// only the fallback for the rare setup where the lookup fails (a directory
/// service that is down, a uid with no passwd entry).
pub fn login_shell() -> String {
    pick_login_shell(passwd_shell(), std::env::var("SHELL").ok())
}

fn pick_login_shell(passwd: Option<String>, env: Option<String>) -> String {
    passwd
        .into_iter()
        .chain(env)
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "sh".into())
}

/// `getpwuid_r` — the reentrant form, because `getpwuid` hands back a pointer
/// into a shared static that another thread's lookup can overwrite under us.
#[cfg(unix)]
fn passwd_shell() -> Option<String> {
    let mut buf_len = match unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) } {
        n if n > 0 => n as usize,
        _ => 1024,
    };
    loop {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut buf = vec![0 as libc::c_char; buf_len];
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            )
        };
        // ERANGE just means the buffer was too small; anything else is fatal.
        if rc == libc::ERANGE && buf_len < 64 * 1024 {
            buf_len *= 2;
            continue;
        }
        if rc != 0 || found.is_null() || pwd.pw_shell.is_null() {
            return None;
        }
        let shell = unsafe { std::ffi::CStr::from_ptr(pwd.pw_shell) }
            .to_str()
            .ok()?
            .to_string();
        return Some(shell);
    }
}

fn basename(program: &str) -> String {
    let base = Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string());
    base
}

/// Keeps conventional executable names for POSIX shells while giving Nushell
/// the same product name in the menu on every supported desktop platform.
fn shell_label(name: &str) -> String {
    if name.eq_ignore_ascii_case("nu") {
        "Nushell".to_string()
    } else {
        name.to_string()
    }
}

fn parse_etc_shells(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn unix_shells_from(
    candidates: impl IntoIterator<Item = String>,
    exists: impl Fn(&str) -> bool,
) -> Vec<DetectedShell> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for path in candidates {
        if !exists(&path) {
            continue;
        }
        let name = basename(&path);
        if seen.insert(name.clone()) {
            out.push(DetectedShell::bare(shell_label(&name), path));
        }
    }
    out
}

/// Shell names worth probing along `$PATH`.
///
/// The POSIX-y ones are here as well as the newer shells: `/etc/shells` lists
/// only what the system ships, so on a box with a Homebrew `bash` the entry
/// that wins the name is `/bin/bash` — macOS's 3.2 from 2007, which is old
/// enough that bash-completion 2.x refuses to load. Probing `$PATH` first makes
/// the menu's "bash" the same binary typing `bash` would reach.
const PATH_PROBED_SHELLS: [&str; 12] = [
    "bash", "zsh", "fish", "nu", "pwsh", "elvish", "xonsh", "sh", "ksh", "dash", "tcsh", "csh",
];

fn path_shell_candidates(path_var: &str) -> Vec<String> {
    let dirs: Vec<&str> = path_var.split(':').filter(|d| d.starts_with('/')).collect();
    PATH_PROBED_SHELLS
        .iter()
        .flat_map(|name| {
            dirs.iter()
                .map(move |dir| format!("{}/{name}", dir.trim_end_matches('/')))
        })
        .collect()
}

#[cfg(unix)]
fn detect_unix() -> Vec<DetectedShell> {
    let etc = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    let path_var = std::env::var("PATH").unwrap_or_default();
    // Order decides who wins a name, since dedupe keeps the first: the login
    // shell must be reachable, then whatever `$PATH` resolves each name to,
    // and `/etc/shells` last to catch shells installed outside `$PATH`.
    let candidates = std::iter::once(login_shell())
        .chain(path_shell_candidates(&path_var))
        .chain(parse_etc_shells(&etc));
    unix_shells_from(candidates, |p| Path::new(p).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_etc_shells_skips_comments_and_blanks() {
        let content = "# /etc/shells\n\n/bin/sh\n/bin/bash\n  /bin/zsh  \n# trailing\n";
        assert_eq!(
            parse_etc_shells(content),
            vec!["/bin/sh", "/bin/bash", "/bin/zsh"]
        );
    }

    #[test]
    fn unix_shells_dedupe_by_basename_keeping_first() {
        let candidates = [
            "/opt/homebrew/bin/zsh",
            "/bin/zsh",
            "/bin/bash",
            "/usr/local/bin/fish",
        ]
        .map(String::from);
        let exists = |p: &str| p != "/usr/local/bin/fish";
        let got = unix_shells_from(candidates, exists);
        assert_eq!(
            got,
            vec![
                DetectedShell::bare("zsh", "/opt/homebrew/bin/zsh"),
                DetectedShell::bare("bash", "/bin/bash"),
            ]
        );
    }

    #[test]
    fn path_shell_candidates_expand_dirs_in_order_skipping_relative() {
        let cands = path_shell_candidates("/opt/homebrew/bin:relative:.:/usr/bin/:");
        // Each name walks $PATH in order, so the earlier dir gets first refusal.
        let first = PATH_PROBED_SHELLS[0];
        assert_eq!(cands[0], format!("/opt/homebrew/bin/{first}"));
        assert_eq!(cands[1], format!("/usr/bin/{first}"));
        assert!(cands.contains(&"/opt/homebrew/bin/nu".to_string()));
        assert!(cands.iter().all(|c| c.starts_with('/')));
        assert_eq!(cands.len(), PATH_PROBED_SHELLS.len() * 2);
    }

    #[test]
    fn nushell_is_probed_from_every_unix_path_directory() {
        let candidates = path_shell_candidates("/opt/homebrew/bin:/home/user/.local/bin");
        let nushell: Vec<_> = candidates
            .iter()
            .filter(|candidate| candidate.ends_with("/nu"))
            .map(String::as_str)
            .collect();

        assert_eq!(
            nushell,
            ["/opt/homebrew/bin/nu", "/home/user/.local/bin/nu"]
        );

        let detected = unix_shells_from(candidates, |candidate| {
            candidate == "/home/user/.local/bin/nu"
        });
        assert_eq!(
            detected,
            [DetectedShell::bare("Nushell", "/home/user/.local/bin/nu")]
        );
    }

    #[test]
    fn etc_shells_still_contributes_what_path_does_not_reach() {
        let etc = ["/bin/zsh".to_string(), "/opt/weird/ksh".to_string()];
        let candidates = path_shell_candidates("/opt/homebrew/bin:/usr/bin")
            .into_iter()
            .chain(etc);
        let exists = |p: &str| {
            matches!(
                p,
                "/bin/zsh" | "/usr/bin/zsh" | "/opt/homebrew/bin/fish" | "/opt/weird/ksh"
            )
        };
        let got = unix_shells_from(candidates, exists);
        assert_eq!(
            got,
            vec![
                // $PATH resolves zsh to /usr/bin/zsh, so /bin/zsh loses the name
                DetectedShell::bare("zsh", "/usr/bin/zsh"),
                DetectedShell::bare("fish", "/opt/homebrew/bin/fish"),
                // never on $PATH, so only /etc/shells knows about it
                DetectedShell::bare("ksh", "/opt/weird/ksh"),
            ]
        );
    }

    /// The bug: `/etc/shells` lists `/bin/bash` (macOS 3.2) before a Homebrew
    /// `bash`, so dedupe-by-name handed the menu entry to the 2007 build.
    #[test]
    fn a_path_shell_beats_the_same_name_in_etc_shells() {
        let etc = [
            "/bin/bash".to_string(),
            "/opt/homebrew/bin/bash".to_string(),
        ];
        let candidates = path_shell_candidates("/opt/homebrew/bin:/usr/bin")
            .into_iter()
            .chain(etc.iter().cloned());
        let exists = |p: &str| matches!(p, "/bin/bash" | "/opt/homebrew/bin/bash");
        let got = unix_shells_from(candidates, exists);
        assert_eq!(
            got,
            vec![DetectedShell::bare("bash", "/opt/homebrew/bin/bash")]
        );

        // …and with the old ordering the stale one would have won.
        let old_order = etc
            .into_iter()
            .chain(path_shell_candidates("/opt/homebrew/bin"));
        assert_eq!(
            unix_shells_from(old_order, exists),
            vec![DetectedShell::bare("bash", "/bin/bash")]
        );
    }

    #[test]
    fn the_login_shell_outranks_path_for_its_own_name() {
        // A login shell that $PATH would otherwise resolve elsewhere still has
        // to be the entry the menu offers.
        let candidates =
            std::iter::once("/opt/custom/bin/zsh".to_string()).chain(path_shell_candidates("/bin"));
        let exists = |p: &str| matches!(p, "/opt/custom/bin/zsh" | "/bin/zsh" | "/bin/bash");
        let got = unix_shells_from(candidates, exists);
        assert_eq!(got[0], DetectedShell::bare("zsh", "/opt/custom/bin/zsh"));
        assert!(!got.iter().any(|s| s.program == "/bin/zsh"));
    }

    /// passwd is the live value; `$SHELL` is a login-time snapshot that `chsh`
    /// cannot move, so it must never win.
    #[test]
    fn login_shell_prefers_passwd_over_a_stale_env() {
        assert_eq!(
            pick_login_shell(
                Some("/opt/homebrew/bin/bash".into()),
                Some("/bin/zsh".into())
            ),
            "/opt/homebrew/bin/bash"
        );
        // passwd unreadable, or the entry is blank — fall back to $SHELL
        assert_eq!(pick_login_shell(None, Some("/bin/zsh".into())), "/bin/zsh");
        assert_eq!(
            pick_login_shell(Some("  ".into()), Some("/bin/zsh".into())),
            "/bin/zsh"
        );
        // neither available
        assert_eq!(pick_login_shell(None, None), "sh");
        assert_eq!(
            pick_login_shell(Some(String::new()), Some(String::new())),
            "sh"
        );
    }

    #[cfg(unix)]
    #[test]
    fn passwd_shell_reads_this_users_entry() {
        // Every account running the suite has a shell in passwd; the point is
        // that the lookup works and returns an absolute path, not a specific one.
        let got = passwd_shell().expect("passwd lookup should succeed");
        assert!(
            got.starts_with('/'),
            "expected an absolute path, got {got:?}"
        );
    }



    #[test]
    fn basename_reduces_paths_to_shell_names() {
        assert_eq!(basename("/usr/local/bin/fish"), "fish");
        assert_eq!(basename("zsh"), "zsh");
    }


    #[test]
    fn a_unique_configured_shell_is_added_first_with_its_args() {
        let detected = vec![DetectedShell::bare("System Shell", "system-shell")];
        let inventory = inventory_from(
            detected,
            Some((
                "custom-shell".into(),
                vec!["--login".into(), "--verbose".into()],
            )),
            "system-shell",
        );

        assert_eq!(inventory.default_name, "custom-shell");
        assert_eq!(inventory.shells.len(), 2);
        assert_eq!(inventory.shells[0].label, "custom-shell");
        assert_eq!(inventory.shells[0].program, "custom-shell");
        assert_eq!(inventory.shells[0].args, ["--login", "--verbose"]);
        assert!(!inventory.shells[0].args_are_tty7_defaults);
    }

    #[test]
    fn a_bare_configured_name_reuses_the_detected_friendly_entry() {
        let detected_program = "/opt/nushell/bin/nu";
        let inventory = inventory_from(
            vec![DetectedShell::bare("Nushell", detected_program)],
            Some(("nu".into(), vec!["--login".into()])),
            "fallback-shell",
        );

        assert_eq!(inventory.shells.len(), 1, "the same shell was duplicated");
        assert_eq!(inventory.default_name, "Nushell");
        assert_eq!(inventory.shells[0].program, "nu");
        assert_eq!(inventory.shells[0].args, ["--login"]);
        assert!(!inventory.shells[0].args_are_tty7_defaults);
    }

    #[test]
    fn a_bare_configured_command_keeps_path_resolution_after_deduplication() {
        let inventory = inventory_from(
            vec![DetectedShell::bare("bash", "/bin/bash")],
            Some(("bash".into(), Vec::new())),
            "fallback-shell",
        );

        assert_eq!(inventory.shells.len(), 1);
        assert_eq!(inventory.shells[0].program, "bash");
        assert_eq!(inventory.default_name, "bash");
    }

    #[test]
    fn explicit_same_named_shells_at_different_paths_stay_distinct() {
        let first = "/opt/shells/first/custom";
        let second = "/opt/shells/second/custom";
        let inventory = inventory_from(
            vec![DetectedShell::bare("custom", first)],
            Some((second.into(), Vec::new())),
            "fallback-shell",
        );

        assert_eq!(inventory.shells.len(), 2);
        assert_eq!(inventory.shells[0].program, second);
        assert_eq!(inventory.shells[0].label, "custom (Configured)");
        assert_eq!(inventory.shells[1].label, "custom");
        assert_eq!(inventory.default_name, "custom (Configured)");
    }

    #[test]
    fn an_explicit_configured_path_does_not_collapse_a_bare_detected_name() {
        let configured = "/opt/shells/custom";
        let detected = "custom";
        let inventory = inventory_from(
            vec![DetectedShell::bare("custom", detected)],
            Some((configured.into(), Vec::new())),
            "fallback-shell",
        );

        assert_eq!(inventory.shells.len(), 2);
        assert_eq!(inventory.shells[0].program, configured);
    }

    #[test]
    fn detected_label_names_the_unconfigured_platform_default() {
        let program = "/opt/homebrew/bin/zsh";
        let label = "zsh";
        let inventory = inventory_from(vec![DetectedShell::bare(label, program)], None, program);

        assert_eq!(inventory.default_name, label);
        assert_eq!(inventory.shells.len(), 1);
        assert!(inventory.shells[0].args_are_tty7_defaults);
    }

    #[test]
    fn inventories_from_older_peers_treat_arguments_as_tty7_defaults() {
        let inventory: ShellInventory = serde_json::from_str(
            r#"{"shells":[{"label":"Git Bash","program":"bash","args":["-l"]}],"default_name":"Git Bash"}"#,
        )
        .expect("an inventory without argument-origin metadata should remain compatible");

        assert!(inventory.shells[0].args_are_tty7_defaults);
    }

    #[test]
    fn default_shell_name_prefers_the_configured_program() {
        assert_eq!(default_shell_name(Some("/usr/bin/fish")), "fish");
        assert_eq!(default_shell_name(Some("pwsh")), "pwsh");
        assert!(!default_shell_name(None).is_empty());
        assert!(!default_shell_name(Some("  ")).is_empty());
    }

    fn custom(label: &str, program: &str, args: &[&str]) -> crate::core::config::CustomShell {
        crate::core::config::CustomShell {
            label: label.to_string(),
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn menu(shells: &[&str]) -> ShellInventory {
        ShellInventory {
            shells: shells.iter().map(|s| DetectedShell::bare(*s, *s)).collect(),
            default_name: shells.first().map(|s| s.to_string()).unwrap_or_default(),
        }
    }

    #[test]
    fn a_custom_entry_joins_the_menu_with_its_arguments_intact() {
        let mut inventory = menu(&["zsh"]);
        append_custom(
            &mut inventory,
            &[custom("Dev container", "docker", &["exec", "-it", "dev", "bash"])],
        );

        let added = inventory.shells.last().expect("the entry");
        assert_eq!(added.label, "Dev container");
        assert_eq!(added.program, "docker");
        assert_eq!(
            added.args,
            vec!["exec".to_string(), "-it".to_string(), "dev".to_string(), "bash".to_string()]
        );
        assert!(
            !added.args_are_tty7_defaults,
            "tty7 contributed none of this command, so none of it may be replaced as a default"
        );
        assert_eq!(
            inventory.default_name, "zsh",
            "an extra entry does not change what a new tab opens with"
        );
    }

    #[test]
    fn a_custom_entry_with_nothing_to_launch_is_not_offered() {
        let mut inventory = menu(&["zsh"]);
        append_custom(&mut inventory, &[custom("Broken", "   ", &[])]);

        assert_eq!(inventory.shells.len(), 1);
    }

    #[test]
    fn a_custom_entry_with_no_label_is_named_after_what_it_runs() {
        let mut inventory = menu(&["zsh"]);
        append_custom(&mut inventory, &[custom("  ", "/opt/homebrew/bin/nu", &[])]);

        assert_eq!(inventory.shells.last().expect("the entry").label, "nu");
    }

    #[test]
    fn a_custom_entry_cannot_take_another_rows_name() {
        let mut inventory = menu(&["zsh", "bash"]);
        append_custom(&mut inventory, &[custom("zsh", "/usr/local/bin/zsh", &[])]);

        // The menu tells its default row apart by label alone, so two rows
        // called "zsh" would put that mark on whichever came first.
        assert_eq!(
            inventory.shells.last().expect("the entry").label,
            "zsh (Custom)"
        );
        assert_eq!(
            inventory
                .shells
                .iter()
                .filter(|s| s.label == inventory.default_name)
                .count(),
            1
        );
    }

    #[test]
    fn custom_entries_that_all_want_the_same_name_still_get_one_each() {
        let mut inventory = menu(&["zsh"]);
        append_custom(
            &mut inventory,
            &[
                custom("zsh", "/a", &[]),
                custom("zsh", "/b", &[]),
                custom("zsh (Custom)", "/c", &[]),
            ],
        );

        // Two rows with the same name launching different programs is the one
        // outcome the menu cannot present: `conformance` fails a host whose
        // inventory names a label twice, and the user cannot tell the rows
        // apart to pick between them.
        let labels: Vec<_> = inventory.shells.iter().map(|s| &s.label).collect();
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(labels.len(), unique.len(), "{labels:?}");
    }

    #[test]
    fn a_custom_entry_cannot_claim_a_default_name_no_row_carries() {
        // `inventory_from` can name a default that is in no row — a login shell
        // recorded in passwd whose file is gone. The menu marks its default by
        // label, so a custom entry landing on that name would wear the mark
        // while a plain new tab opened something else entirely.
        let mut inventory = menu(&["zsh"]);
        inventory.default_name = "ksh".into();
        append_custom(&mut inventory, &[custom("ksh", "/usr/bin/ksh", &[])]);

        assert_eq!(
            inventory.shells.last().expect("the entry").label,
            "ksh (Custom)"
        );
    }

    #[test]
    fn no_custom_entries_leaves_the_menu_exactly_as_it_was() {
        let before = menu(&["zsh", "bash"]);
        let mut after = before.clone();
        append_custom(&mut after, &[]);

        assert_eq!(before, after);
    }
}
