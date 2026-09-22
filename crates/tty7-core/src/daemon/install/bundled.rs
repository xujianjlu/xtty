//! Server binaries shipped next to the client for remote install.
//!
//! A macOS GUI can carry Linux `tty7-server` assets under a `server/` sibling
//! (or via `TTY7_BUNDLED_SERVER_DIR`) so the first SSH connect does not have to
//! hit GitHub. Discovery is path-only — no WSL.

use std::path::{Path, PathBuf};

use super::{InstallError, LoadedBinary, ServerBinarySource};

pub const BUNDLED_DIR_ENV: &str = "TTY7_BUNDLED_SERVER_DIR";

pub const BUNDLED_SUBDIR: &str = "server";

pub fn bundled_search_dirs(exe: Option<&Path>, override_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |d: PathBuf| {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    if let Some(dir) = override_dir {
        push(dir.to_path_buf());
    }
    if let Some(exe_dir) = exe.and_then(Path::parent) {
        push(exe_dir.join(BUNDLED_SUBDIR));
        push(exe_dir.to_path_buf());
        if let Some(parent) = exe_dir.parent() {
            push(parent.join("Resources").join(BUNDLED_SUBDIR));
        }
    }
    dirs
}

pub struct BundledServerBinary {
    dirs: Vec<PathBuf>,
}

impl BundledServerBinary {
    pub fn discover() -> Self {
        let exe = std::env::current_exe().ok();
        let over = std::env::var_os(BUNDLED_DIR_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self::in_dirs(bundled_search_dirs(exe.as_deref(), over.as_deref()))
    }

    pub fn in_dirs(dirs: Vec<PathBuf>) -> Self {
        Self { dirs }
    }

    pub fn from_env_only() -> Option<Self> {
        let dir = std::env::var_os(BUNDLED_DIR_ENV).filter(|v| !v.is_empty())?;
        Some(Self::in_dirs(vec![PathBuf::from(dir)]))
    }

    pub fn locate(&self, asset: &str) -> Option<PathBuf> {
        self.dirs
            .iter()
            .map(|dir| dir.join(asset))
            .find(|path| path.is_file())
    }
}

impl ServerBinarySource for BundledServerBinary {
    fn load(&self, _version: &str, asset: &'static str) -> Result<LoadedBinary, InstallError> {
        let missing = |extra: Option<String>| InstallError::MissingBundled {
            asset,
            searched: match extra {
                Some(one) => vec![one],
                None => self.dirs.iter().map(|d| d.display().to_string()).collect(),
            },
        };
        let Some(path) = self.locate(asset) else {
            return Err(missing(None));
        };
        let bytes =
            std::fs::read(&path).map_err(|e| missing(Some(format!("{} ({e})", path.display()))))?;
        if bytes.is_empty() {
            return Err(missing(Some(format!("{} (empty file)", path.display()))));
        }
        Ok(LoadedBinary {
            bytes,
            origin: path.display().to_string(),
        })
    }
}
