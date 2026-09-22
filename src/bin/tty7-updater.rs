//! macOS-only updater helper (verify / install / relaunch).

#[cfg(target_os = "macos")]
mod macos {
    use std::fs::{self, OpenOptions};
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::Duration;

    const PARENT_POLL: Duration = Duration::from_millis(100);
    const LAUNCH_GRACE: Duration = Duration::from_secs(1);

    pub fn run() -> Result<(), String> {
        let mut args = std::env::args_os().skip(1);
        let command = args
            .next()
            .and_then(|arg| arg.into_string().ok())
            .ok_or_else(usage)?;
        match command.as_str() {
            "verify" => {
                let current = next_path(&mut args)?;
                let archive = next_path(&mut args)?;
                let checksums = next_path(&mut args)?;
                let asset_name = next_string(&mut args)?;
                let stage = next_path(&mut args)?;
                let expected_version = next_string(&mut args)?;
                reject_extra(args)?;
                verify_archive(&archive, &checksums, &asset_name)?;
                let replacement = extract_archive(&archive, &stage)?;
                verify_update(&current, &replacement, &expected_version)
            }
            "install" => {
                let parent_pid = next_string(&mut args)?
                    .parse::<u32>()
                    .map_err(|_| "parent pid is not an unsigned integer".to_string())?;
                let current = next_path(&mut args)?;
                let archive = next_path(&mut args)?;
                let checksums = next_path(&mut args)?;
                let asset_name = next_string(&mut args)?;
                let stage = next_path(&mut args)?;
                let expected_version = next_string(&mut args)?;
                let log = next_path(&mut args)?;
                let options = tail_options(args)?;
                options.apply();
                install(InstallPlan {
                    parent_pid,
                    current,
                    archive,
                    checksums,
                    asset_name,
                    stage,
                    expected_version,
                    log,
                    result_file: options.result_file,
                })
            }
            _ => Err(usage()),
        }
    }

    fn usage() -> String {
        "usage: tty7-updater verify <current.app> <archive.zip> <checksums.txt> \
         <asset-name> <stage-dir> <version>\n\
         or: tty7-updater install <parent-pid> <current.app> <archive.zip> <checksums.txt> \
         <asset-name> <stage-dir> <version> <log-path> \
         [--config-dir <dir>] [--result-file <path>]"
            .to_string()
    }

    fn next_path(args: &mut impl Iterator<Item = std::ffi::OsString>) -> Result<PathBuf, String> {
        args.next().map(PathBuf::from).ok_or_else(usage)
    }

    fn next_string(args: &mut impl Iterator<Item = std::ffi::OsString>) -> Result<String, String> {
        args.next()
            .and_then(|arg| arg.into_string().ok())
            .ok_or_else(usage)
    }

    fn reject_extra(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<(), String> {
        if args.next().is_some() {
            Err(usage())
        } else {
            Ok(())
        }
    }

    /// The named options an install verb takes after its positional
    /// arguments. Passed as argv (not the environment) so a hand-run
    /// updater and the in-app spawn share one parser.
    #[derive(Default)]
    struct TailOptions {
        config_dir: Option<PathBuf>,
        result_file: Option<PathBuf>,
    }

    fn tail_options(
        mut args: impl Iterator<Item = std::ffi::OsString>,
    ) -> Result<TailOptions, String> {
        let mut options = TailOptions::default();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--config-dir") => options.config_dir = Some(next_path(&mut args)?),
                Some("--result-file") => options.result_file = Some(next_path(&mut args)?),
                _ => return Err(usage()),
            }
        }
        Ok(options)
    }

    impl TailOptions {
        fn apply(&self) {
            let Some(dir) = &self.config_dir else { return };
            tty7_core::core::config::set_config_dir(dir.clone());
            // Re-exported so the relaunched app — a child of this process —
            // keeps answering for the same config directory. Safe here:
            // argument parsing runs before any thread exists.
            unsafe { std::env::set_var("TTY7_CONFIG_DIR", dir) };
        }
    }

    /// The terminal outcome of the attempt, for the next GUI launch to merge
    /// into the update state (#540). Best-effort, like every log line here.
    fn report_outcome(
        result_file: Option<&Path>,
        log: &Path,
        version: &str,
        result: &Result<(), String>,
    ) {
        let Some(path) = result_file else { return };
        let outcome = tty7_core::daemon::install::outcome::UpdateOutcome {
            version: version.to_string(),
            ok: result.is_ok(),
            detail: result.as_ref().err().cloned(),
        };
        if let Err(error) = tty7_core::daemon::install::outcome::write_outcome(path, &outcome) {
            log_line(
                log,
                &format!(
                    "could not record the update outcome at {}: {error}",
                    path.display()
                ),
            );
        }
    }

    struct InstallPlan {
        parent_pid: u32,
        current: PathBuf,
        archive: PathBuf,
        checksums: PathBuf,
        asset_name: String,
        stage: PathBuf,
        expected_version: String,
        log: PathBuf,
        result_file: Option<PathBuf>,
    }

    fn install(plan: InstallPlan) -> Result<(), String> {
        install_inner(&plan)
    }

    fn install_inner(plan: &InstallPlan) -> Result<(), String> {
        let replacement = plan.stage.join("unpacked/tty7.app");
        wait_for_exit(plan.parent_pid);
        log_line(&plan.log, "re-verifying staged tty7 update");
        let verification = verify_archive(&plan.archive, &plan.checksums, &plan.asset_name)
            .and_then(|()| verify_update(&plan.current, &replacement, &plan.expected_version));
        if let Err(error) = verification {
            log_line(&plan.log, &error);
            let _ = fs::remove_dir_all(&plan.stage);
            let result = Err(error);
            // The outcome lands before the old app does: the relaunched GUI
            // merges it at startup, and a write afterward races that merge
            // (#540).
            report_outcome(
                plan.result_file.as_deref(),
                &plan.log,
                &plan.expected_version,
                &result,
            );
            let _ = launch_app(&plan.current);
            return result;
        }
        log_line(&plan.log, &format!("replacing {}", plan.current.display()));
        let report = |result: &Result<(), String>| {
            report_outcome(
                plan.result_file.as_deref(),
                &plan.log,
                &plan.expected_version,
                result,
            );
        };
        replace_and_relaunch(&plan.current, &replacement, &plan.stage, launch_app, report)
            .inspect_err(|error| log_line(&plan.log, error))
    }

    fn verify_archive(archive: &Path, checksums: &Path, asset_name: &str) -> Result<(), String> {
        let bytes =
            fs::read(archive).map_err(|error| format!("reading {}: {error}", archive.display()))?;
        let manifest = fs::read_to_string(checksums)
            .map_err(|error| format!("reading {}: {error}", checksums.display()))?;
        tty7_core::daemon::install::checksums::verify(&manifest, asset_name, &bytes)
            .map_err(|error| error.to_string())
    }

    fn extract_archive(archive: &Path, stage: &Path) -> Result<PathBuf, String> {
        let unpacked = stage.join("unpacked");
        fs::create_dir(&unpacked)
            .map_err(|error| format!("creating {}: {error}", unpacked.display()))?;
        run_checked(
            Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(archive)
                .arg(&unpacked),
            "extracting the update archive",
        )?;
        Ok(unpacked.join("tty7.app"))
    }

    fn verify_update(
        current: &Path,
        replacement: &Path,
        expected_version: &str,
    ) -> Result<(), String> {
        let executable = replacement.join("Contents/MacOS/tty7-app");
        let updater = replacement.join("Contents/MacOS/tty7-updater");
        if !replacement.is_dir() || !executable.is_file() || !updater.is_file() {
            return Err(
                "the staged bundle is missing tty7-app or tty7-updater under Contents/MacOS"
                    .to_string(),
            );
        }
        let actual_version = bundle_version(replacement)?;
        if actual_version != expected_version {
            return Err(format!(
                "the staged app reports version {actual_version}, expected {expected_version}"
            ));
        }
        run_checked(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(replacement),
            "verifying the staged app's code signature",
        )?;
        let current_requirement = signing_requirement(current)?;
        let replacement_requirement = signing_requirement(replacement)?;
        if current_requirement != replacement_requirement {
            return Err(format!(
                "the staged app has a different designated requirement: current \
                 {current_requirement:?}, staged {replacement_requirement:?}"
            ));
        }
        Ok(())
    }

    fn bundle_version(app: &Path) -> Result<String, String> {
        let output = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleShortVersionString"])
            .arg(app.join("Contents/Info.plist"))
            .output()
            .map_err(|error| format!("reading the staged app version: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "reading the staged app version: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// The designated requirement out of what `codesign -d -r-` printed.
    ///
    /// Split from the call below so the parse can be exercised without a
    /// bundle to point at — which is why nothing caught it reading the wrong
    /// stream. `codesign` writes the requirement to **stdout** and puts only
    /// the `-d` display header (`Executable=…`) on stderr, so a parse that
    /// read stderr could never match: every in-app update on macOS failed
    /// with "codesign did not report a designated requirement", on every
    /// build and every release, with nothing a user could do about it (#708).
    ///
    /// Both streams are read, stdout first. Which stream carries which half is
    /// codesign's own business and has moved before; a requirement found
    /// anywhere in the output is the requirement, and the updater has no
    /// reason to be the stricter party about where it was printed.
    fn designated_requirement(stdout: &str, stderr: &str) -> Option<String> {
        [stdout, stderr]
            .into_iter()
            .flat_map(str::lines)
            .find_map(|line| line.strip_prefix("designated => ").map(str::to_string))
    }

    fn signing_requirement(app: &Path) -> Result<String, String> {
        let output = Command::new("/usr/bin/codesign")
            .args(["-d", "-r-"])
            .arg(app)
            .output()
            .map_err(|error| {
                format!(
                    "reading the code-signing requirement for {}: {error}",
                    app.display()
                )
            })?;
        if !output.status.success() {
            return Err(format!(
                "reading the code-signing requirement for {}: {}",
                app.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        designated_requirement(
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        )
        .ok_or_else(|| "codesign did not report a designated requirement".to_string())
    }

    fn replace_and_relaunch(
        current: &Path,
        replacement: &Path,
        stage: &Path,
        launch: impl Fn(&Path) -> Result<(), String>,
        report: impl Fn(&Result<(), String>),
    ) -> Result<(), String> {
        // The staging directory is a fresh TempDir created beside the current
        // bundle, so a backup here stays on the same filesystem without using a
        // predictable sibling path.  In particular, never delete a fixed-name
        // path beside the app: it may be a recovery copy left by an interrupted
        // update (or simply an unrelated user-owned path).
        let backup = stage.join("previous.app");
        if backup.exists() {
            let result = Err(format!(
                "the update staging backup already exists: {}",
                backup.display()
            ));
            report(&result);
            return result;
        }
        if let Err(error) = fs::rename(current, &backup) {
            let result = Err(format!("moving the current app aside: {error}"));
            report(&result);
            return result;
        }

        if let Err(error) = fs::rename(replacement, current) {
            let _ = fs::rename(&backup, current);
            let _ = fs::remove_dir_all(stage);
            let result = Err(format!("putting the staged app in place: {error}"));
            report(&result);
            return result;
        }

        match launch(current) {
            Ok(()) => {
                let _ = remove_path(&backup);
                let _ = fs::remove_dir_all(stage);
                let result = Ok(());
                report(&result);
                result
            }
            Err(error) => {
                let _ = remove_path(current);
                let (result, relaunch) = match fs::rename(&backup, current) {
                    Ok(()) => {
                        let _ = fs::remove_dir_all(stage);
                        (Err(error), true)
                    }
                    Err(restore) => (
                        Err(format!("{error}; restoring the previous app: {restore}")),
                        false,
                    ),
                };
                // The outcome lands before the old app does: the relaunched GUI
                // merges it at startup, and a write afterward races that merge
                // (#540).
                report(&result);
                if relaunch {
                    let _ = launch(current);
                }
                result
            }
        }
    }

    fn launch_app(app: &Path) -> Result<(), String> {
        let executable = app.join("Contents/MacOS/tty7-app");
        let mut child = Command::new(&executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("launching {}: {error}", executable.display()))?;
        healthy_after_grace(&mut child)
    }

    fn healthy_after_grace(child: &mut Child) -> Result<(), String> {
        thread::sleep(LAUNCH_GRACE);
        match child
            .try_wait()
            .map_err(|error| format!("checking the relaunched app: {error}"))?
        {
            None => Ok(()),
            Some(status) => Err(format!(
                "the relaunched app exited immediately with {status}"
            )),
        }
    }

    fn wait_for_exit(pid: u32) {
        // The updater is spawned directly by the app it waits for, so while
        // that app lives it *is* this process's parent, and the kernel
        // reparents us to launchd the moment it exits. Watching getppid() is
        // therefore immune to pid reuse, which `kill(pid, 0)` is not: a
        // recycled pid keeps answering 0 forever.
        let pid = pid as libc::pid_t;
        if unsafe { libc::getppid() } == pid {
            while unsafe { libc::getppid() } == pid {
                thread::sleep(PARENT_POLL);
            }
            return;
        }
        // Not our parent — a hand-run updater. The polling fallback keeps
        // that invocation working, pid-reuse caveat and all.
        while process_alive(pid) {
            thread::sleep(PARENT_POLL);
        }
    }

    fn process_alive(pid: libc::pid_t) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn remove_path(path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
        .map_err(|error| format!("removing {}: {error}", path.display()))
    }

    fn run_checked(command: &mut Command, context: &str) -> Result<(), String> {
        let output = command
            .output()
            .map_err(|error| format!("{context}: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{context}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn log_line(path: &Path, message: &str) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{message}");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The stream split, held against `codesign` itself rather than
        /// against what the updater believes about it.
        ///
        /// This is the test #708 was missing. The parse read stderr, where
        /// `codesign` puts only the display header, so every in-app update on
        /// macOS failed — and no unit test could see it, because the parse was
        /// fused to the process call and the process needs a signed bundle.
        ///
        /// `/bin/ls` is that bundle: Apple-signed, present on every macOS, and
        /// it answers `-d -r-` with a designated requirement of its own. If
        /// this ever fails because the requirement moved streams again, the
        /// function under test already reads both — so it failing means
        /// `codesign` stopped printing one at all, which the updater must not
        /// discover from a user's failed update.
        #[test]
        fn the_designated_requirement_is_read_off_the_stream_codesign_uses() {
            let out = Command::new("/usr/bin/codesign")
                .args(["-d", "-r-", "/bin/ls"])
                .output()
                .expect("codesign is part of macOS");
            assert!(out.status.success(), "codesign refused /bin/ls");

            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let requirement = designated_requirement(&stdout, &stderr)
                .expect("codesign reports a designated requirement for /bin/ls");
            assert!(
                requirement.contains("identifier"),
                "a designated requirement names an identifier: {requirement:?}"
            );

            // Named rather than merely relied on: the updater used to read
            // only the stream that carries none of it.
            assert!(
                stdout.contains("designated => "),
                "the requirement is on stdout; if this moved, so must the doc above"
            );
        }

        /// Reading both streams is what keeps the choice above from being a
        /// guess about a future macOS.
        #[test]
        fn a_requirement_on_either_stream_is_found_and_neither_is_an_error() {
            let line = "designated => identifier \"com.example.app\" and anchor apple";
            let want = Some("identifier \"com.example.app\" and anchor apple".to_string());

            assert_eq!(designated_requirement(line, "Executable=/x"), want);
            assert_eq!(designated_requirement("Executable=/x", line), want);
            assert_eq!(designated_requirement("Executable=/x", ""), None);
            assert_eq!(designated_requirement("", ""), None);
        }

        fn bundle(path: &Path, marker: &str) {
            fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
            fs::write(path.join("marker"), marker).unwrap();
        }

        #[test]
        fn successful_launch_commits_the_replacement() {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("tty7.app");
            let stage = root.path().join("stage");
            let replacement = stage.join("tty7.app");
            bundle(&current, "old");
            bundle(&replacement, "new");

            replace_and_relaunch(&current, &replacement, &stage, |_| Ok(()), |_| ()).unwrap();

            assert_eq!(fs::read_to_string(current.join("marker")).unwrap(), "new");
            assert!(!stage.exists());
            assert!(!root.path().join(".tty7.app.tty7-update-backup").exists());
        }

        #[test]
        fn failed_launch_restores_and_relaunches_the_previous_app() {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("tty7.app");
            let stage = root.path().join("stage");
            let replacement = stage.join("tty7.app");
            bundle(&current, "old");
            bundle(&replacement, "new");
            let launches = std::cell::Cell::new(0);
            let reported_after_launches = std::cell::Cell::new(usize::MAX);

            let error = replace_and_relaunch(
                &current,
                &replacement,
                &stage,
                |_| {
                    launches.set(launches.get() + 1);
                    if launches.get() == 1 {
                        Err("new app failed".to_string())
                    } else {
                        Ok(())
                    }
                },
                |_| reported_after_launches.set(launches.get()),
            )
            .unwrap_err();

            assert_eq!(error, "new app failed");
            assert_eq!(launches.get(), 2);
            // The outcome is reported after the failed first launch but before
            // the old app comes back — the relaunched GUI must find it already
            // on disk at startup (#540).
            assert_eq!(reported_after_launches.get(), 1);
            assert_eq!(fs::read_to_string(current.join("marker")).unwrap(), "old");
            assert!(!stage.exists());
        }

        #[test]
        fn replacement_does_not_remove_a_fixed_name_sibling() {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("tty7.app");
            let stage = root.path().join("stage");
            let replacement = stage.join("tty7.app");
            let sibling = root.path().join(".tty7.app.tty7-update-backup");
            bundle(&current, "old");
            bundle(&replacement, "new");
            bundle(&sibling, "keep");

            replace_and_relaunch(&current, &replacement, &stage, |_| Ok(()), |_| ()).unwrap();

            assert_eq!(fs::read_to_string(current.join("marker")).unwrap(), "new");
            assert_eq!(fs::read_to_string(sibling.join("marker")).unwrap(), "keep");
        }

        #[test]
        fn verify_rejects_a_bundle_without_the_helper() {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("current.app");
            let replacement = root.path().join("replacement.app");
            bundle(&current, "old");
            fs::create_dir_all(replacement.join("Contents/MacOS")).unwrap();
            fs::write(replacement.join("Contents/MacOS/tty7-app"), b"app").unwrap();

            let error = verify_update(&current, &replacement, "1.0.0").unwrap_err();
            assert!(
                error.contains("missing tty7-app or tty7-updater"),
                "{error}"
            );
        }

        #[test]
        fn archive_verification_rejects_bytes_that_do_not_match_the_manifest() {
            let root = tempfile::tempdir().unwrap();
            let archive = root.path().join("tty7.zip");
            let manifest = root.path().join("checksums.txt");
            fs::write(&archive, b"downloaded bytes").unwrap();
            fs::write(
                &manifest,
                format!(
                    "{}  tty7.zip\n",
                    tty7_core::daemon::install::checksums::hex(
                        &tty7_core::daemon::install::checksums::sha256(b"published bytes")
                    )
                ),
            )
            .unwrap();

            let error = verify_archive(&archive, &manifest, "tty7.zip").unwrap_err();
            assert!(error.contains("failed sha256 verification"), "{error}");
        }

        #[test]
        fn bundle_version_preserves_the_complete_nightly_identity() {
            let root = tempfile::tempdir().unwrap();
            let app = root.path().join("tty7.app");
            let contents = app.join("Contents");
            fs::create_dir_all(&contents).unwrap();
            fs::write(
                contents.join("Info.plist"),
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>CFBundleShortVersionString</key>
    <string>26.8.2-nightly.20260803</string>
</dict>
</plist>
"#,
            )
            .unwrap();

            assert_eq!(bundle_version(&app).unwrap(), "26.8.2-nightly.20260803");
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = macos::run() {
        eprintln!("tty7-updater: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("tty7-updater is only available on macOS");
    std::process::exit(1);
}
