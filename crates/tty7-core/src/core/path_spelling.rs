//! The spelling tty7 stores a local path in.
//!
//! On macOS (and other Unix hosts) `/` is the only separator and there is no
//! extended-length form, so both helpers here are the identity. Remote hosts
//! keep the spelling of the machine the path is *on* — see [`spelling_on`].

use std::borrow::Cow;
use std::path::Path;

/// Re-spells a path on **this** machine with the separators this OS expects.
///
/// On Unix the OS separator is already `/`, and a backslash in a path is an
/// ordinary filename character — there is nothing to re-spell.
///
/// **Only for paths on the machine this window runs on.** A remote host's
/// `/home/u/src` is already native over there; re-spelling it would put a
/// path on the clipboard that names nothing on either machine.
pub fn native_separators(path: &Path) -> Cow<'_, Path> {
    Cow::Borrowed(path)
}

/// The spelling tty7 stores a local path in, so that two of them naming one
/// directory are one key.
///
/// On Unix there is no second spelling to fold into the first.
pub fn local_spelling(path: &Path) -> Cow<'_, Path> {
    Cow::Borrowed(path)
}

/// [`local_spelling`], for a caller that is building the path anyway and has
/// nothing to hand back borrowed.
pub fn local_spelling_buf(path: impl AsRef<Path>) -> std::path::PathBuf {
    local_spelling(path.as_ref()).into_owned()
}

/// The spelling a path that lives on `host` is stored in.
///
/// [`local_spelling`] answers for the machine this process runs on, and every
/// caller that keys a repository by its root has to ask this one instead: the
/// same caches, the same `git` probes and the same SCM panel serve a pane on
/// another machine, and `/home/u/src` from a Linux box is already native over
/// there.
///
/// A remote host is left exactly as it arrived. Path syntax is a property of
/// the machine the path is *on*, not of the one asking.
pub fn spelling_on(host: crate::host::HostId, path: &Path) -> Cow<'_, Path> {
    match host.is_local() {
        true => local_spelling(path),
        false => Cow::Borrowed(path),
    }
}

/// [`spelling_on`], for a caller with nothing to hand back borrowed.
pub fn spelling_on_buf(host: crate::host::HostId, path: impl AsRef<Path>) -> std::path::PathBuf {
    spelling_on(host, path.as_ref()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_with_nothing_to_fix_is_handed_back_borrowed() {
        for p in ["README.md", "/code/repo"] {
            let got = local_spelling(Path::new(p));
            assert_eq!(got.as_ref(), Path::new(p), "{p:?}");
            assert!(matches!(got, Cow::Borrowed(_)), "{p:?} should not allocate");
            assert!(matches!(native_separators(Path::new(p)), Cow::Borrowed(_)));
        }
    }

    #[test]
    fn every_spelling_of_one_directory_lands_on_the_same_key() {
        let want = local_spelling_buf(Path::new("/home/x/repo"));
        assert_eq!(
            local_spelling(Path::new("/home/x/repo")).as_ref(),
            want.as_path()
        );
    }

    #[test]
    fn a_path_on_another_machine_is_left_in_that_machines_spelling() {
        use crate::host::HostId;

        let remote = HostId::from_connection_key("ssh-direct:me@box:22");
        for posix in ["/home/u/src", "/home/u/a b/c", "/"] {
            let got = spelling_on(remote, Path::new(posix));
            assert_eq!(got.as_ref(), Path::new(posix), "{posix:?}");
            assert!(matches!(got, Cow::Borrowed(_)), "{posix:?}");
            assert_eq!(spelling_on_buf(remote, posix).to_string_lossy(), posix);
        }
        let win = r"C:/Users/x/repo";
        assert_eq!(spelling_on_buf(remote, win).to_string_lossy(), win);
    }

    #[test]
    fn both_helpers_are_the_identity() {
        // A backslash in a Unix path is an ordinary filename character, and a
        // remote host's paths pass through this same code unchanged.
        for p in ["/home/u/tty7", r"C:\Users\dev", r"mixed/path\here"] {
            let got = local_spelling(Path::new(p));
            assert_eq!(got.as_ref(), Path::new(p), "{p:?}");
            assert!(matches!(got, Cow::Borrowed(_)));
        }
    }
}
