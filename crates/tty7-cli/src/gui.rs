use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

pub fn launch(path: Option<&Path>) -> Result<()> {
    let executable = find_executable()?;
    let mut command = Command::new(&executable);
    if let Some(path) = path {
        command.arg("--open-path").arg(path);
    }

    // The CLI must not keep a caller's redirected pipes alive after it exits.
    // The GUI owns its own logging and never needs this console's standard IO.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("launching {}", executable.display()))?;
    Ok(())
}

/// The bundled `xtty-app` this CLI belongs to: `XTTY_APP` / legacy `TTY7_APP`,
/// then the file next to this executable, then `PATH`.
pub fn find_executable() -> Result<PathBuf> {
    for key in ["XTTY_APP", "TTY7_APP"] {
        if let Some(explicit) = std::env::var_os(key) {
            let path = PathBuf::from(explicit);
            if path.is_file() {
                return Ok(path);
            }
            bail!(
                "{key} points to {}, but that file does not exist",
                path.display()
            );
        }
    }

    let name = executable_name();
    if let Ok(own) = std::env::current_exe()
        && let Some(dir) = own.parent()
    {
        for candidate_name in [name, "tty7-app"] {
            let sibling = dir.join(candidate_name);
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for candidate_name in [name, "tty7-app"] {
                let candidate = dir.join(candidate_name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    bail!("could not find {name} next to this CLI or on PATH — install xtty-app, or set XTTY_APP")
}

fn executable_name() -> &'static str {
    "xtty-app"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gui_executable_name_matches_the_platform() {
        assert_eq!(executable_name(), "xtty-app");
    }
}
