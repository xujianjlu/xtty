//! Application-modal Finder dialogs.
//!
//! gpui's `prompt_for_paths` calls `NSOpenPanel.beginWithCompletionHandler`,
//! which attaches as a sheet on the key window. That sheet inherits xtty's
//! forced DarkAqua (a black panel that is not Finder), and a directories-only
//! sheet often never appears — so `sz` looked like it wrote straight into
//! `~/Downloads`.
//!
//! [`pick_paths`] uses `runModal` instead: a standalone Open dialog with the
//! system appearance.

use std::path::PathBuf;

pub(crate) struct Options {
    pub files: bool,
    pub directories: bool,
    pub multiple: bool,
}

/// Block on a Finder-style Open dialog. Must run on the main thread, and must
/// not be called while a gpui entity update is holding the view lock.
pub(crate) fn pick_paths(options: Options) -> Option<Vec<PathBuf>> {
    pick_paths_macos(options)
}

#[cfg(target_os = "macos")]
fn pick_paths_macos(options: Options) -> Option<Vec<PathBuf>> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAppearanceCustomization, NSApplication, NSModalResponseOK, NSOpenPanel};

    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    app.activate();

    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(options.files);
    panel.setCanChooseDirectories(options.directories);
    panel.setAllowsMultipleSelection(options.multiple);
    panel.setCanCreateDirectories(options.directories);
    panel.setResolvesAliases(false);
    // Follow the system (Finder) appearance, not the app's forced DarkAqua.
    panel.setAppearance(None);

    if panel.runModal() != NSModalResponseOK {
        return None;
    }

    let mut paths = Vec::new();
    for url in panel.URLs() {
        if let Some(path) = url.path() {
            paths.push(PathBuf::from(path.to_string()));
        }
    }
    if paths.is_empty() { None } else { Some(paths) }
}

#[cfg(not(target_os = "macos"))]
fn pick_paths_macos(_options: Options) -> Option<Vec<PathBuf>> {
    None
}
