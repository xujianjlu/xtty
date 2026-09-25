//! Stubs for abolished Native SSH / SFTP / managed-forward GUI surfaces.
//!
//! Daemon protocol types are gone; these exist only so leftover UI call sites
//! compile while the UI is stripped. Methods are no-ops.

use gpui::{App, Context, Div, IntoElement, Window, div};
use super::app::Tty7App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SshForwardKind {
    #[default]
    Local,
    Remote,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SshForwardRule {
    pub kind: SshForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardStatus {
    Listening,
    Error(String),
}

impl Default for ForwardStatus {
    fn default() -> Self {
        ForwardStatus::Listening
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedForward {
    pub id: u64,
    pub pane_id: u64,
    pub kind: SshForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub description: Option<String>,
    pub status: ForwardStatus,
}

#[derive(Debug, Clone, Default)]
pub struct ForwardFields {
    pub advanced: bool,
    pub kind: SshForwardKind,
    pub bind_host: String,
    pub bind_port: String,
    pub target_host: String,
    pub target_port: String,
    pub description: String,
}

impl ForwardFields {
    pub fn collect(&self) -> Option<SshForwardRule> {
        None
    }

    pub fn is_blank(&self) -> bool {
        true
    }
}

pub fn added_forward<'a>(
    before: &[u64],
    list: &'a [ManagedForward],
) -> Option<&'a ManagedForward> {
    list.iter().find(|m| !before.contains(&m.id))
}

pub fn rule_of(_forward: &ManagedForward) -> SshForwardRule {
    SshForwardRule::default()
}

/// Minimal stand-in so leftover Native dial signatures type-check; never dialled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeSshSpec {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub profile_id: Option<String>,
    pub password: Option<String>,
    pub key_passphrases: Option<std::collections::HashMap<String, String>>,
    pub login_script: Vec<String>,
    pub remote_clipboard_write: bool,
}

impl NativeSshSpec {
    pub fn without_secrets(&self) -> NativeSshSpec {
        let mut s = self.clone();
        s.password = None;
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshPhase {
    Connecting,
    Authenticating,
    Connected,
    Failed,
}

#[derive(Debug, Clone)]
pub enum AuthPromptKind {
    Password { prompt: String },
    Banner { text: String },
}

#[derive(Debug, Clone)]
pub enum AuthResponse {
    Password(String),
    Cancel,
}

#[derive(Debug, Clone, Default)]
pub struct SshAlgorithms;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SshAuthMode {
    #[default]
    Auto,
}

#[derive(Debug, Clone, Default)]
pub struct SshProxy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshTestNeed {
    Password,
    Passphrase,
    KeyPassphrase,
    KeyboardInteractive,
    HostKeyDecision,
    HostKeyChanged,
}

#[derive(Debug, Clone)]
pub enum SshTestReport {
    Authenticated { elapsed_ms: u32 },
    NeedsInput { need: SshTestNeed },
    Failed { reason: String },
}

#[derive(Debug, Clone)]
pub enum WorkspaceOp {
    ListForwards,
    AddForward { rule: SshForwardRule },
    TeardownForwards,
    EnsureLoopback { remote_host: String, remote_port: u16 },
    RemoveForward { forward_id: u64 },
}

#[derive(Debug, Clone)]
pub struct WorkspaceRequest {
    pub workspace: String,
    pub pane_id: u64,
    pub op: WorkspaceOp,
}

impl Tty7App {
    pub(crate) fn toggle_sftp(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Files-over-SFTP abolished; open the local Files tab instead.
        self.set_right_panel_tab(crate::core::config::RightPanelTab::Files, cx);
    }

    pub(crate) fn render_ssh_prompt_overlay(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Div> {
        None
    }

    pub(crate) fn render_ssh_status_strip(
        &self,
        _leaf: &gpui::Entity<crate::terminal::view::TerminalView>,
        _cx: &App,
    ) -> Option<gpui::AnyElement> {
        None
    }

    pub(crate) fn forward_row(
        &self,
        _forward: &ManagedForward,
        _mono: &gpui::SharedString,
        _cx: &mut Context<Self>,
    ) -> Div {
        div()
    }

    pub(crate) fn forward_form(&self, _pane_id: u64, _cx: &mut Context<Self>) -> Div {
        div()
    }

    pub(crate) fn on_auth_prompt_ready(
        &mut self,
        _view: gpui::Entity<crate::terminal::view::TerminalView>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(crate) fn raise_routed_auth(
        &mut self,
        pending: crate::ui::remote_connect::PendingAuth,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> crate::ui::remote_workspace::SheetOutcome {
        // Native auth sheet abolished — give the pending prompt back unhandled.
        crate::ui::remote_workspace::SheetOutcome::GiveBack(pending)
    }

    pub(crate) fn push_ssh_connect_error(&mut self, _reason: String, _cx: &mut Context<Self>) {}

    pub(crate) fn sftp_close_browser(&mut self, _cx: &mut Context<Self>) {}
    pub(crate) fn sftp_transfers_footer(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        None
    }
    pub(crate) fn sftp_sync_pane(
        &mut self,
        _pane_id: Option<u64>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }

    pub(crate) fn render_panel_sftp(
        &mut self,
        _host: String,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div().into_any_element()
    }

    pub(crate) fn native_ssh_spec_for_profile(
        &self,
        profile: &crate::core::ssh_profile::SshProfile,
        cx: &App,
    ) -> NativeSshSpec {
        crate::ui::ssh_connect::build_native_ssh_spec(
            profile,
            &cx.global::<crate::core::config::Config>().ssh_profiles,
            &crate::core::keychain::OsCredentialStore,
            true,
        )
    }
}


pub const NO_TARGET_FADE: f32 = 0.4;

/// Stub endpoint type formerly in ssh_prompt.
#[derive(Clone, Debug, Default)]
pub struct PromptEndpoint;
