//! Finder / Explorer context-menu integration.
//!
//! macOS does not register shell verbs through this module; the public API is
//! retained so callers (settings language refresh, CLI flags) stay shared.

use anyhow::Result;

/// No-op on macOS — there is no Explorer verb registry to write.
pub fn register() -> Result<()> {
    Ok(())
}

/// No-op on macOS.
pub fn unregister() -> Result<()> {
    Ok(())
}

/// No-op on macOS — labels live in the app, not in a system registry.
pub fn refresh_labels() -> Result<()> {
    Ok(())
}
