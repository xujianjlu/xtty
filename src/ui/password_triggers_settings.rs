//! Settings UI for [`PasswordTrigger`] rules.
//!
//! Rules live in `config.json`; secrets live only in the OS keychain under
//! [`SERVICE_PASSWORD_TRIGGER`]. This module owns the form model, validation,
//! and the Settings page that edits them.

use gpui::{AnyElement, Context, Entity, Window, div, prelude::*, px};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};

use crate::core::config::{Config, PasswordTrigger};
use crate::core::keychain::{CredentialStore as _, OsCredentialStore, SERVICE_PASSWORD_TRIGGER};
use crate::ui::app::Tty7App;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::settings::{FIELD_W, SettingsSection};

/// In-memory editor for one password-trigger rule.
pub(crate) struct PasswordTriggerForm {
    /// `None` while creating; `Some(i)` while editing `password_triggers[i]`.
    pub(crate) edit_index: Option<usize>,
    pub(crate) name: Entity<InputState>,
    pub(crate) pattern: Entity<InputState>,
    pub(crate) credential: Entity<InputState>,
    pub(crate) secret: Entity<InputState>,
    pub(crate) cooldown_ms: Entity<InputState>,
    pub(crate) regex: bool,
    pub(crate) send_enter: bool,
    pub(crate) enabled: bool,
    /// Keychain account the form was opened against (for rename / migrate).
    pub(crate) loaded_credential: String,
    /// Whether the keychain already held a secret for `loaded_credential`.
    pub(crate) had_secret: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TriggerFormError {
    PatternRequired,
    CredentialRequired,
    BadRegex,
    BadCooldown,
    SecretRequired,
}

impl TriggerFormError {
    fn message(self) -> String {
        match self {
            Self::PatternRequired => t(L10nKey::SettingsPasswordTriggerPatternRequired).to_string(),
            Self::CredentialRequired => {
                t(L10nKey::SettingsPasswordTriggerCredentialRequired).to_string()
            }
            Self::BadRegex => t(L10nKey::SettingsPasswordTriggerBadRegex).to_string(),
            Self::BadCooldown => t(L10nKey::SettingsPasswordTriggerBadCooldown).to_string(),
            Self::SecretRequired => t(L10nKey::SettingsPasswordTriggerSecretRequired).to_string(),
        }
    }
}

/// Build a rule from form fields. Returns the rule and an optional secret to
/// write into the keychain (never into config).
pub(crate) fn collect_password_trigger(
    name: &str,
    pattern: &str,
    regex: bool,
    credential: &str,
    send_enter: bool,
    enabled: bool,
    cooldown_ms: &str,
    secret: &str,
    had_secret: bool,
    is_new: bool,
) -> Result<(PasswordTrigger, Option<String>), TriggerFormError> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err(TriggerFormError::PatternRequired);
    }
    let credential = credential.trim();
    if credential.is_empty() {
        return Err(TriggerFormError::CredentialRequired);
    }
    if regex && regex::Regex::new(pattern).is_err() {
        return Err(TriggerFormError::BadRegex);
    }
    let cooldown_ms = cooldown_ms.trim();
    let cooldown_ms = if cooldown_ms.is_empty() {
        PasswordTrigger::default().cooldown_ms
    } else {
        cooldown_ms
            .parse::<u64>()
            .map_err(|_| TriggerFormError::BadCooldown)?
    };
    let typed = secret.to_string();
    let write_secret = if typed.is_empty() {
        if is_new && !had_secret {
            return Err(TriggerFormError::SecretRequired);
        }
        None
    } else {
        Some(typed)
    };
    Ok((
        PasswordTrigger {
            name: name.trim().to_string(),
            pattern: pattern.to_string(),
            regex,
            credential: credential.to_string(),
            send_enter,
            enabled,
            cooldown_ms,
        },
        write_secret,
    ))
}

/// Whether a config JSON blob ever embeds a password-trigger secret value.
///
/// The UI must never put the typed secret into `password_triggers`; only the
/// keychain account name goes in `credential`.
#[cfg(test)]
fn config_json_hides_trigger_secrets(
    cfg: &tty7_core::core::config::Config,
    forbidden: &[&str],
) -> bool {
    let Ok(json) = serde_json::to_string(cfg) else {
        return false;
    };
    !forbidden.iter().any(|s| !s.is_empty() && json.contains(s))
}

fn trigger_has_secret(account: &str) -> bool {
    if account.trim().is_empty() {
        return false;
    }
    matches!(
        OsCredentialStore.get(SERVICE_PASSWORD_TRIGGER, account),
        Ok(Some(_))
    )
}

fn write_trigger_secret(account: &str, secret: &str) -> Result<(), String> {
    OsCredentialStore
        .set(SERVICE_PASSWORD_TRIGGER, account, secret)
        .map_err(|e| e.to_string())
}

fn delete_trigger_secret(account: &str) -> Result<(), String> {
    OsCredentialStore
        .delete(SERVICE_PASSWORD_TRIGGER, account)
        .map_err(|e| e.to_string())
}

fn credential_still_used(cfg: &Config, account: &str, skip: Option<usize>) -> bool {
    cfg.password_triggers.iter().enumerate().any(|(i, rule)| {
        Some(i) != skip && rule.credential == account && !rule.credential.is_empty()
    })
}

impl Tty7App {
    pub(crate) fn render_settings_password_triggers(&self, cx: &mut Context<Self>) -> AnyElement {
        let rules = cx.global::<Config>().password_triggers.clone();
        let form = self
            .active_settings()
            .and_then(|s| s.password_trigger_form.as_ref());

        let mut page = v_flex().child(self.section_intro(
            t(L10nKey::SettingsPasswordTriggersIntro),
            t(L10nKey::SettingsPasswordTriggersIntroDesc),
            cx,
        ));

        if let Some(form) = form {
            page = page.child(self.render_password_trigger_form(form, cx));
        } else {
            page = page
                .child(self.render_password_trigger_list(&rules, cx))
                .child(
                    h_flex().mt_4().child(
                        Button::new("password-trigger-add")
                            .label(t(L10nKey::SettingsAddRule))
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.password_trigger_begin_new(window, cx);
                            })),
                    ),
                );
        }

        page.into_any_element()
    }

    fn render_password_trigger_list(
        &self,
        rules: &[PasswordTrigger],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        if rules.is_empty() {
            return div()
                .py_4()
                .text_sm()
                .text_color(muted)
                .child(t(L10nKey::SettingsPasswordTriggerEmpty))
                .into_any_element();
        }

        let mut list = v_flex().gap_1();
        for (i, rule) in rules.iter().enumerate() {
            let title = if rule.name.trim().is_empty() {
                rule.pattern.clone()
            } else {
                rule.name.clone()
            };
            let desc = {
                let mut parts = Vec::new();
                parts.push(if rule.regex {
                    t(L10nKey::SettingsPasswordTriggerRegex).to_string()
                } else {
                    t(L10nKey::SettingsPasswordTriggerLiteral).to_string()
                });
                if rule.send_enter {
                    parts.push(t(L10nKey::SettingsPasswordTriggerSendEnterShort).to_string());
                }
                parts.push(rule.credential.clone());
                parts.join(" · ")
            };
            let enabled = rule.enabled;
            let control = h_flex()
                .items_center()
                .gap_2()
                .child(
                    crate::ui::theme::switch(("password-trigger-enabled", i), cx)
                        .checked(enabled)
                        .on_click(cx.listener(move |this, on: &bool, _w, cx| {
                            this.password_trigger_set_enabled(i, *on, cx);
                        })),
                )
                .child(
                    Button::new(("password-trigger-edit", i))
                        .label(t(L10nKey::EditorEdit))
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.password_trigger_begin_edit(i, window, cx);
                        })),
                )
                .child(
                    Button::new(("password-trigger-delete", i))
                        .label(t(L10nKey::Delete))
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.password_trigger_ask_delete(i, window, cx);
                        })),
                )
                .into_any_element();
            list = list.child(self.settings_row(title, desc, control, cx));
        }
        list.into_any_element()
    }

    fn render_password_trigger_form(
        &self,
        form: &PasswordTriggerForm,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let heading = if form.edit_index.is_some() {
            t(L10nKey::SettingsEditPasswordTrigger)
        } else {
            t(L10nKey::SettingsNewPasswordTrigger)
        };
        let secret_note = if form.had_secret {
            t(L10nKey::SettingsPasswordTriggerSecretKept)
        } else {
            t(L10nKey::SettingsPasswordTriggerSecretNeeded)
        };

        v_flex()
            .gap_1()
            .child(self.section_header(heading, cx))
            .child(
                self.settings_row(
                    t(L10nKey::SettingsName),
                    "",
                    Input::new(&form.name)
                        .small()
                        .w(px(FIELD_W))
                        .into_any_element(),
                    cx,
                ),
            )
            .child(
                self.settings_row(
                    t(L10nKey::SettingsPasswordTriggerPattern),
                    t(L10nKey::SettingsPasswordTriggerPatternDesc),
                    Input::new(&form.pattern)
                        .small()
                        .w(px(FIELD_W))
                        .into_any_element(),
                    cx,
                ),
            )
            .child(
                self.settings_row(
                    t(L10nKey::SettingsPasswordTriggerRegex),
                    t(L10nKey::SettingsPasswordTriggerRegexDesc),
                    crate::ui::theme::switch("password-trigger-regex", cx)
                        .checked(form.regex)
                        .on_click(cx.listener(|this, on: &bool, _w, cx| {
                            if let Some(f) = this.password_trigger_form_mut() {
                                f.regex = *on;
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                    cx,
                ),
            )
            .child(
                self.settings_row(
                    t(L10nKey::SettingsPasswordTriggerCredential),
                    t(L10nKey::SettingsPasswordTriggerCredentialDesc),
                    Input::new(&form.credential)
                        .small()
                        .w(px(FIELD_W))
                        .into_any_element(),
                    cx,
                ),
            )
            .child(
                self.settings_row(
                    t(L10nKey::SettingsPassword),
                    secret_note,
                    Input::new(&form.secret)
                        .small()
                        .mask_toggle()
                        .w(px(FIELD_W))
                        .into_any_element(),
                    cx,
                ),
            )
            .child(
                self.settings_row(
                    t(L10nKey::SettingsPasswordTriggerSendEnter),
                    t(L10nKey::SettingsPasswordTriggerSendEnterDesc),
                    crate::ui::theme::switch("password-trigger-send-enter", cx)
                        .checked(form.send_enter)
                        .on_click(cx.listener(|this, on: &bool, _w, cx| {
                            if let Some(f) = this.password_trigger_form_mut() {
                                f.send_enter = *on;
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                    cx,
                ),
            )
            .child(
                self.settings_row(
                    t(L10nKey::SettingsPasswordTriggerEnabled),
                    "",
                    crate::ui::theme::switch("password-trigger-form-enabled", cx)
                        .checked(form.enabled)
                        .on_click(cx.listener(|this, on: &bool, _w, cx| {
                            if let Some(f) = this.password_trigger_form_mut() {
                                f.enabled = *on;
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                    cx,
                ),
            )
            .child(
                self.settings_row(
                    t(L10nKey::SettingsPasswordTriggerCooldown),
                    t(L10nKey::SettingsPasswordTriggerCooldownDesc),
                    Input::new(&form.cooldown_ms)
                        .small()
                        .w(px(FIELD_W))
                        .into_any_element(),
                    cx,
                ),
            )
            .child(
                h_flex()
                    .mt_4()
                    .gap_2()
                    .child(
                        Button::new("password-trigger-save")
                            .label(t(L10nKey::Save))
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.password_trigger_save(window, cx);
                            })),
                    )
                    .child(
                        Button::new("password-trigger-cancel")
                            .label(t(L10nKey::Cancel))
                            .small()
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.password_trigger_cancel(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn password_trigger_form_mut(&mut self) -> Option<&mut PasswordTriggerForm> {
        self.active_settings_mut()
            .and_then(|s| s.password_trigger_form.as_mut())
    }

    fn password_trigger_begin_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = self.seed_password_trigger_form(None, &PasswordTrigger::default(), window, cx);
        if let Some(s) = self.active_settings_mut() {
            s.password_trigger_form = Some(form);
            s.section = SettingsSection::PasswordTriggers;
        }
        cx.notify();
    }

    fn password_trigger_begin_edit(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rule) = cx.global::<Config>().password_triggers.get(index).cloned() else {
            return;
        };
        let form = self.seed_password_trigger_form(Some(index), &rule, window, cx);
        if let Some(s) = self.active_settings_mut() {
            s.password_trigger_form = Some(form);
        }
        cx.notify();
    }

    fn seed_password_trigger_form(
        &mut self,
        edit_index: Option<usize>,
        rule: &PasswordTrigger,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PasswordTriggerForm {
        let had_secret = trigger_has_secret(&rule.credential);
        let name = seed_trigger_input(window, cx, &rule.name, t(L10nKey::SettingsNameHint));
        let pattern = seed_trigger_input(
            window,
            cx,
            &rule.pattern,
            t(L10nKey::SettingsPasswordTriggerPatternHint),
        );
        let credential = seed_trigger_input(
            window,
            cx,
            &rule.credential,
            t(L10nKey::SettingsPasswordTriggerCredentialHint),
        );
        let secret = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(if had_secret {
                    t(L10nKey::SettingsPasswordTriggerSecretHintKeep)
                } else {
                    t(L10nKey::SettingsPasswordTriggerSecretHintNew)
                })
        });
        let cooldown_ms = seed_trigger_input(window, cx, &rule.cooldown_ms.to_string(), "1500");
        // Re-render on typing so Save stays responsive to validation state.
        for input in [&name, &pattern, &credential, &secret, &cooldown_ms] {
            if let Some(s) = self.active_settings_mut() {
                s._subs
                    .push(cx.subscribe_in(input, window, |_this, _i, ev, _w, cx| {
                        if matches!(ev, InputEvent::Change) {
                            cx.notify();
                        }
                    }));
            }
        }
        PasswordTriggerForm {
            edit_index,
            name,
            pattern,
            credential,
            secret,
            cooldown_ms,
            regex: rule.regex,
            send_enter: rule.send_enter,
            enabled: rule.enabled,
            loaded_credential: rule.credential.clone(),
            had_secret,
        }
    }

    fn password_trigger_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.password_trigger_form = None;
        }
        cx.notify();
    }

    fn password_trigger_set_enabled(&mut self, index: usize, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            if let Some(rule) = cfg.password_triggers.get_mut(index) {
                rule.enabled = on;
            }
        });
    }

    fn password_trigger_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self
            .active_settings()
            .and_then(|s| s.password_trigger_form.as_ref())
        else {
            return;
        };
        let name = form.name.read(cx).value().to_string();
        let pattern = form.pattern.read(cx).value().to_string();
        let credential = form.credential.read(cx).value().to_string();
        let secret = form.secret.read(cx).value().to_string();
        let cooldown_ms = form.cooldown_ms.read(cx).value().to_string();
        let regex = form.regex;
        let send_enter = form.send_enter;
        let enabled = form.enabled;
        let edit_index = form.edit_index;
        let loaded_credential = form.loaded_credential.clone();
        let had_secret = form.had_secret;

        let (rule, write_secret) = match collect_password_trigger(
            &name,
            &pattern,
            regex,
            &credential,
            send_enter,
            enabled,
            &cooldown_ms,
            &secret,
            had_secret,
            edit_index.is_none(),
        ) {
            Ok(v) => v,
            Err(err) => {
                window.push_notification(err.message(), cx);
                return;
            }
        };

        let new_account = rule.credential.clone();
        if let Some(secret) = write_secret.as_ref() {
            if let Err(e) = write_trigger_secret(&new_account, secret) {
                window.push_notification(
                    t_fmt(
                        L10nKey::SettingsCouldntSaveTriggerSecret,
                        &[("account", new_account.as_str()), ("error", e.as_str())],
                    ),
                    cx,
                );
                return;
            }
        } else if !loaded_credential.is_empty() && loaded_credential != new_account && had_secret {
            // Credential renamed without retyping the secret — migrate the
            // keychain entry so the matcher still finds it.
            match OsCredentialStore.get(SERVICE_PASSWORD_TRIGGER, &loaded_credential) {
                Ok(Some(existing)) => {
                    if let Err(e) = write_trigger_secret(&new_account, &existing) {
                        window.push_notification(
                            t_fmt(
                                L10nKey::SettingsCouldntSaveTriggerSecret,
                                &[("account", new_account.as_str()), ("error", e.as_str())],
                            ),
                            cx,
                        );
                        return;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    window.push_notification(
                        t_fmt(
                            L10nKey::SettingsCouldntSaveTriggerSecret,
                            &[
                                ("account", loaded_credential.as_str()),
                                ("error", &e.to_string()),
                            ],
                        ),
                        cx,
                    );
                    return;
                }
            }
        }

        let skip = edit_index;
        let drop_old = !loaded_credential.is_empty() && loaded_credential != new_account && {
            let cfg = cx.global::<Config>();
            !credential_still_used(cfg, &loaded_credential, skip)
        };
        if drop_old {
            let _ = delete_trigger_secret(&loaded_credential);
        }

        self.update_config(cx, |cfg| match edit_index {
            Some(i) if i < cfg.password_triggers.len() => {
                cfg.password_triggers[i] = rule;
            }
            _ => cfg.password_triggers.push(rule),
        });

        if let Some(s) = self.active_settings_mut() {
            s.password_trigger_form = None;
        }
        cx.notify();
    }

    fn password_trigger_ask_delete(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rule) = cx.global::<Config>().password_triggers.get(index).cloned() else {
            return;
        };
        let label = if rule.name.trim().is_empty() {
            rule.pattern.clone()
        } else {
            rule.name.clone()
        };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &t_fmt(
                L10nKey::SettingsPasswordTriggerDeleteTitle,
                &[("name", &label)],
            ),
            Some(t(L10nKey::SettingsPasswordTriggerDeleteBody)),
            &crate::ui::confirm_answers(t(L10nKey::Delete), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(0) = answer.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.password_trigger_delete_confirmed(index, cx)
            });
        })
        .detach();
    }

    fn password_trigger_delete_confirmed(&mut self, index: usize, cx: &mut Context<Self>) {
        let account = cx
            .global::<Config>()
            .password_triggers
            .get(index)
            .map(|r| r.credential.clone())
            .unwrap_or_default();
        let drop_secret = !account.is_empty()
            && !credential_still_used(cx.global::<Config>(), &account, Some(index));
        self.update_config(cx, |cfg| {
            if index < cfg.password_triggers.len() {
                cfg.password_triggers.remove(index);
            }
        });
        if drop_secret {
            let _ = delete_trigger_secret(&account);
        }
        if let Some(s) = self.active_settings_mut() {
            if let Some(form) = s.password_trigger_form.as_ref() {
                match form.edit_index {
                    Some(i) if i == index => s.password_trigger_form = None,
                    Some(i) if i > index => {
                        if let Some(f) = s.password_trigger_form.as_mut() {
                            f.edit_index = Some(i - 1);
                        }
                    }
                    _ => {}
                }
            }
        }
        cx.notify();
    }
}

fn seed_trigger_input(
    window: &mut Window,
    cx: &mut Context<Tty7App>,
    value: &str,
    placeholder: &str,
) -> Entity<InputState> {
    let value = value.to_string();
    let placeholder = placeholder.to_string();
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_requires_pattern_and_credential() {
        assert_eq!(
            collect_password_trigger("", "", false, "", true, true, "1500", "x", false, true)
                .unwrap_err(),
            TriggerFormError::PatternRequired
        );
        assert_eq!(
            collect_password_trigger(
                "su",
                "Password:",
                false,
                "",
                true,
                true,
                "1500",
                "x",
                false,
                true
            )
            .unwrap_err(),
            TriggerFormError::CredentialRequired
        );
    }

    #[test]
    fn collect_requires_secret_for_new_rules() {
        assert_eq!(
            collect_password_trigger(
                "su",
                "Password:",
                false,
                "su-search",
                true,
                true,
                "1500",
                "",
                false,
                true
            )
            .unwrap_err(),
            TriggerFormError::SecretRequired
        );
        let (rule, secret) = collect_password_trigger(
            "su",
            "Password:",
            false,
            "su-search",
            true,
            true,
            "1500",
            "hunter2",
            false,
            true,
        )
        .unwrap();
        assert_eq!(rule.credential, "su-search");
        assert!(rule.send_enter);
        assert_eq!(secret.as_deref(), Some("hunter2"));
    }

    #[test]
    fn collect_keeps_existing_secret_when_field_blank() {
        let (rule, secret) = collect_password_trigger(
            "su",
            "Password:",
            false,
            "su-search",
            false,
            true,
            "2000",
            "",
            true,
            false,
        )
        .unwrap();
        assert!(!rule.send_enter);
        assert_eq!(rule.cooldown_ms, 2000);
        assert!(secret.is_none());
    }

    #[test]
    fn collect_rejects_bad_regex_and_cooldown() {
        assert_eq!(
            collect_password_trigger(
                "",
                "(unterminated",
                true,
                "acc",
                true,
                true,
                "1500",
                "x",
                false,
                true
            )
            .unwrap_err(),
            TriggerFormError::BadRegex
        );
        assert_eq!(
            collect_password_trigger(
                "",
                "Password:",
                false,
                "acc",
                true,
                true,
                "nope",
                "x",
                false,
                true
            )
            .unwrap_err(),
            TriggerFormError::BadCooldown
        );
    }

    #[test]
    fn config_serialization_never_embeds_typed_secret() {
        let rule = PasswordTrigger {
            name: "su".into(),
            pattern: "Password:".into(),
            regex: false,
            credential: "su-search".into(),
            send_enter: true,
            enabled: true,
            cooldown_ms: 1500,
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("su-search"));
        assert!(json.contains("send_enter"));
        assert!(!json.contains("hunter2"));
        assert!(!json.contains("super-secret"));

        let mut cfg = tty7_core::core::config::Config::default();
        cfg.password_triggers.push(rule);
        assert!(config_json_hides_trigger_secrets(
            &cfg,
            &["hunter2", "super-secret"]
        ));
    }
}
