//! OpenSSH host picker — spawn local `ssh` argv panes.
//!
//! Native (russh) dial paths were abolished; saved hosts open via system `ssh`.

use uuid::Uuid;

use crate::core::config::Config;
use crate::core::ssh_profile::SshProfile;
use super::app::{SpawnWhere, Tty7App};

impl Tty7App {
    pub(crate) fn connect_ssh_profile(
        &mut self,
        profile_id: uuid::Uuid,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.connect_ssh_profile_at(profile_id, SpawnWhere::NewTab, window, cx);
    }

    /// A saved host opened where the caller asks for it — a tab of its own, or
    /// beside the pane in front of the user when the new-tab menu's row was
    /// taken with ⌥ held.
    ///
    /// Opens a local pane running system `ssh` (not the russh Native path).
    /// Typed `ssh` / jumper hops use the same dest facts and clone as this.
    pub(crate) fn connect_ssh_profile_at(
        &mut self,
        profile_id: uuid::Uuid,
        at: SpawnWhere,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(profile) = cx
            .global::<Config>()
            .ssh_profiles
            .iter()
            .find(|p| p.id == profile_id)
            .cloned()
        else {
            return;
        };
        self.bump_ssh_frecency(profile_id, cx);
        let profiles = cx.global::<Config>().ssh_profiles.clone();
        self.open_system_ssh(&profile, &profiles, at, window, cx);
    }

    pub(crate) fn quick_connect(
        &mut self,
        qc: crate::core::ssh_profile::QuickConnect,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(resolved) = crate::core::ssh_config::resolve_alias_to_profile(&qc.host) {
            let mut profile = resolved.profile;
            if let Some(user) = qc.user {
                profile.user = user;
            }
            if let Some(port) = qc.port {
                profile.port = port;
            }
            if let Some(jump) = resolved.proxy_jump.filter(|j| !j.is_empty()) {
                if profile.jump_host.is_none() && profile.proxy_command.is_none() {
                    profile.proxy_command = Some(format!("ssh -W %h:%p {jump}"));
                }
            }
            let profiles = cx.global::<Config>().ssh_profiles.clone();
            self.open_system_ssh(&profile, &profiles, SpawnWhere::NewTab, window, cx);
            return;
        }
        let port = qc.port_or_default();
        let mut profile = SshProfile::new(qc.host.clone());
        profile.host = qc.host;
        profile.port = port;
        if let Some(user) = qc.user {
            profile.user = user;
        }
        let profiles = cx.global::<Config>().ssh_profiles.clone();
        self.open_system_ssh(&profile, &profiles, SpawnWhere::NewTab, window, cx);
    }

    /// Spawn local PTY with `ssh` argv; seed the tab title from profile user@host.
    pub(crate) fn open_system_ssh(
        &mut self,
        profile: &SshProfile,
        profiles: &[SshProfile],
        at: SpawnWhere,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let mut args = crate::core::ssh_profile::openssh_argv(profile, profiles);
        if profile.shell_integration {
            if !args.iter().any(|a| a == "-t" || a == "-tt") {
                args.insert(0, "-t".into());
            }
            args.push(crate::daemon::hop_bootstrap());
        }
        let shell = Some(crate::daemon::protocol::ShellSpec {
            program: "ssh".into(),
            args,
            args_are_tty7_defaults: false,
        });
        let identity = tty7_core::core::tab_view::connection_identity(&profile.user, &profile.host);
        self.open_shell(shell, at, window, cx);
        if let Some(identity) = identity {
            if let Some(view) = self.focused_pane_view(window, cx) {
                view.update(cx, |v, cx| v.seed_connection_identity(identity, cx));
            }
        }
    }

    pub(crate) fn restart_ssh_session(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) {
        // Native SSH respawn abolished; OpenSSH panes are local PTYs.
    }

    fn focused_pane_view(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> Option<gpui::Entity<crate::terminal::view::TerminalView>> {
        self.tabs
            .get(self.active)?
            .pane
            .focused_or_first(window, cx)
    }

    pub(crate) fn open_new_ssh_host(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.open_settings_section(crate::ui::settings::SettingsSection::Ssh, window, cx);
        self.add_new_profile(window, cx);
    }

    pub(crate) fn edit_ssh_host_of_target(
        &mut self,
        target: &crate::core::session::RemoteTarget,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        use crate::core::session::RemoteTarget;
        match target {
            RemoteTarget::Profile { id } => self.open_ssh_profile_in_settings(*id, window, cx),
            RemoteTarget::Alias { alias } => match config_alias_resolver(alias) {
                Some((resolved, _)) => {
                    let mut profile = resolved;
                    profile.id = Uuid::new_v4();
                    profile.name = alias.clone();
                    profile.group = None;
                    self.open_settings_section(
                        crate::ui::settings::SettingsSection::Ssh,
                        window,
                        cx,
                    );
                    self.ssh_form_load(&profile, window, cx);
                }
                None => self.open_ssh_profile_new_from_target(alias.clone(), window, cx),
            },
            RemoteTarget::Direct { user, host, port } => {
                let mut profile = SshProfile::new(String::new());
                profile.user = user.clone();
                profile.host = host.clone();
                profile.port = *port;
                let target = crate::core::ssh_profile::to_connect_string(&profile);
                self.open_ssh_profile_new_from_target(target, window, cx);
            }
            RemoteTarget::LocalStdio { .. } => {}
        }
    }

    pub(crate) fn tab_ssh_host_form(
        &self,
        index: usize,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> Option<(TabHostForm, &'static str)> {
        let leaf = self.tabs.get(index)?.pane.focused_or_first(window, cx)?;
        let view = leaf.read(cx);
        let target = ssh_host_target_of_view(&view, &cx.global::<Config>().ssh_profiles)?;
        let label = crate::ui::switcher::host_form_label(&target)?;
        Some((TabHostForm::Saved(target), label))
    }

    pub(crate) fn open_tab_ssh_host_form(
        &mut self,
        form: &TabHostForm,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match form {
            TabHostForm::Saved(target) => self.edit_ssh_host_of_target(target, window, cx),
        }
    }

    fn bump_ssh_frecency(&mut self, profile_id: uuid::Uuid, cx: &mut gpui::Context<Self>) {
        self.update_config(cx, |cfg| {
            let entry = cfg.ssh_profile_frecency.entry(profile_id).or_default();
            entry.count = entry.count.saturating_add(1);
            entry.last_used = crate::core::config::unix_now();
        });
    }
}

/// Saved hosts, most likely first.
pub(crate) fn ssh_profiles_by_frecency(cx: &gpui::App) -> Vec<SshProfile> {
    let cfg = cx.global::<Config>();
    let now = crate::core::config::unix_now();
    let mut profiles = cfg.ssh_profiles.clone();
    profiles.sort_by(|a, b| {
        let score = |p: &SshProfile| {
            cfg.ssh_profile_frecency
                .get(&p.id)
                .map(|u| u.score(now))
                .unwrap_or(0.0)
        };
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    profiles
}

pub(crate) fn config_alias_resolver(alias: &str) -> Option<(SshProfile, Option<String>)> {
    crate::core::ssh_config::resolve_alias_to_profile(alias).map(|r| (r.profile, r.proxy_jump))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TabHostForm {
    Saved(crate::core::session::RemoteTarget),
}

pub(crate) fn ssh_host_target_of_view(
    view: &crate::terminal::view::TerminalView,
    profiles: &[SshProfile],
) -> Option<crate::core::session::RemoteTarget> {
    use crate::core::session::RemoteTarget;
    let dest = view.remote_context().and_then(|ctx| {
        (ctx.kind == crate::daemon::protocol::RemoteKind::Ssh).then_some(ctx.target)
    })?;
    let (user, host) = match dest.split_once('@') {
        Some((user, host)) => (user, host),
        None => ("", dest.as_str()),
    };
    let host = host.split(':').next().unwrap_or(host);
    profiles
        .iter()
        .find(|p| p.host == host && (user.is_empty() || p.user == user))
        .map(|p| RemoteTarget::Profile { id: p.id })
        .or_else(|| Some(RemoteTarget::direct(user, host, 22)))
}
