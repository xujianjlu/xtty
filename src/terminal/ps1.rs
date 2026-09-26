//! Read-only parse of a visible shell prompt. Never writes the PTY.
//!
//! Only the prompt itself counts. Anything after `$` / `#` / `%` / `]` is the
//! command the user is typing and must not become cwd or identity.

use tty7_core::core::tab_view::{cwd_from_title, identity_from_title};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ps1Facts {
    pub identity: Option<String>,
    pub cwd: Option<String>,
}

pub(crate) fn looks_like_auth_prompt(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("password")
        || l.contains("passphrase")
        || l.contains("verification code")
        || l.contains("one-time")
        || l.contains("otp")
        || l.contains("(yes/no")
        || l.contains("are you sure you want to continue connecting")
}

const DEFAULT_HOP_ENDINGS: &[char] = &['$', '#', '%', '>'];

/// Last jumper line is ready for the destination hostname.
///
/// A custom suffix, when set, must be the tail of the line. Otherwise the
/// line must end with `$`, `#`, `%`, or `>`. Auth / host-key prompts never
/// count — typing the dest into those boxes breaks login.
pub(crate) fn hop_ready_line(line: &str, custom: Option<&str>) -> bool {
    let s = line.trim();
    if s.is_empty() || looks_like_auth_prompt(s) || looks_like_ssh_launch(s) {
        return false;
    }
    if let Some(needle) = custom.map(str::trim).filter(|n| !n.is_empty()) {
        return s.ends_with(needle);
    }
    s.chars()
        .last()
        .is_some_and(|c| DEFAULT_HOP_ENDINGS.contains(&c))
}

pub(crate) fn looks_like_ssh_launch(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains(" ssh ")
        || (l.contains('\t') && l.contains("ssh "))
        || l.trim_start().starts_with("ssh ")
}

fn is_prompt_glyph(c: char) -> bool {
    matches!(
        c,
        '$' | '#' | '%' | '>' | '❯' | '❱' | '❮' | '➜' | '→' | 'λ'
    )
}

/// Slice through the prompt terminator; drop the typed command after it.
pub(crate) fn prompt_prefix(raw: &str) -> Option<&str> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(close) = s.find(']') {
        let after = s[close + 1..].chars().next();
        if after.is_some_and(is_prompt_glyph) {
            let end = close + 1 + after.unwrap().len_utf8();
            return Some(s.get(..end)?);
        }
        if s[..close].contains('@') {
            return Some(s.get(..=close)?);
        }
    }
    let at = s.find('@')?;
    for (i, c) in s[at + 1..].char_indices() {
        if is_prompt_glyph(c) {
            let end = at + 1 + i + c.len_utf8();
            return Some(s.get(..end)?);
        }
    }
    None
}

/// Visible dest PS1 (prompt only, no command tail).
pub(crate) fn looks_like_shell_ps1(line: &str) -> bool {
    let Some(prompt) = prompt_prefix(line) else {
        return false;
    };
    if looks_like_auth_prompt(line) || looks_like_ssh_launch(line) {
        return false;
    }
    let t = prompt.trim_end();
    matches!(t.chars().last(), Some(c) if is_prompt_glyph(c) || c == ']')
        || (t.contains('@') && (t.contains(':') || t.contains('[') || t.contains(' ')))
}

/// Last line is still the leftover launch / MOTD from when the hop appeared.
pub(crate) fn still_pre_login(last_line: &str, line_at_detect: Option<&str>) -> bool {
    let Some(at) = line_at_detect else {
        return false;
    };
    if last_line != at {
        return false;
    }
    looks_like_ssh_launch(last_line) || !looks_like_shell_ps1(last_line)
}

fn strip_prompt_decor(s: &str) -> &str {
    s.trim()
        .trim_end_matches(is_prompt_glyph)
        .trim_matches(|c: char| matches!(c, '[' | ']' | ' '))
}

fn is_cwd_token(cwd: &str) -> bool {
    if cwd.is_empty()
        || cwd.contains('$')
        || cwd.contains(']')
        || cwd.contains('[')
        || cwd.chars().any(char::is_whitespace)
    {
        return false;
    }
    cwd.starts_with('/')
        || cwd.starts_with('~')
        || cwd == "."
        || cwd == ".."
        || (cwd.len() >= 2
            && cwd.as_bytes()[0].is_ascii_alphabetic()
            && cwd.as_bytes()[1] == b':')
        || cwd
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// `user@host` and optional path from a prompt line. `None` if this is not a PS1.
pub(crate) fn parse_ps1_line(raw: &str) -> Option<Ps1Facts> {
    if looks_like_auth_prompt(raw) || looks_like_ssh_launch(raw) {
        return None;
    }
    let prompt = prompt_prefix(raw)?;
    if !looks_like_shell_ps1(raw) {
        return None;
    }
    facts_from_user_host_rest(strip_prompt_decor(prompt))
}

fn facts_from_user_host_rest(s: &str) -> Option<Ps1Facts> {
    if let Some(identity) = identity_from_title(s) {
        return Some(Ps1Facts {
            identity: Some(identity),
            cwd: cwd_from_title(s).filter(|c| is_cwd_token(c)),
        });
    }
    let (head, tail) = s.split_once(char::is_whitespace)?;
    let (user, host) = head.split_once('@')?;
    if user.is_empty()
        || host.is_empty()
        || user.chars().any(char::is_whitespace)
        || host
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | '\\'))
    {
        return None;
    }
    let identity = format!("{user}@{host}");
    let cwd = tail.trim();
    Some(Ps1Facts {
        identity: Some(identity),
        cwd: is_cwd_token(cwd).then(|| cwd.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dest_ps1_shapes() {
        assert_eq!(
            parse_ps1_line("carol@dev-box:~/src$").unwrap(),
            Ps1Facts {
                identity: Some("carol@dev-box".into()),
                cwd: Some("~/src".into()),
            }
        );
        assert_eq!(
            parse_ps1_line("[carol@jumper ~]$").unwrap(),
            Ps1Facts {
                identity: Some("carol@jumper".into()),
                cwd: Some("~".into()),
            }
        );
        assert_eq!(
            parse_ps1_line("root@box#").unwrap().identity.as_deref(),
            Some("root@box")
        );
        assert_eq!(
            parse_ps1_line("xujian@cd02 ~/CODE/retr %").unwrap(),
            Ps1Facts {
                identity: Some("xujian@cd02".into()),
                cwd: Some("~/CODE/retr".into()),
            }
        );
        assert_eq!(
            parse_ps1_line("xujian6@cd02.adsys.tjcorp.qihoo.net:~/retr$")
                .unwrap()
                .identity
                .as_deref(),
            Some("xujian6@cd02.adsys.tjcorp.qihoo.net")
        );
    }

    #[test]
    fn command_after_prompt_is_not_cwd() {
        assert_eq!(
            parse_ps1_line("[xujian6@cd02 ~]$ cd CODE").unwrap(),
            Ps1Facts {
                identity: Some("xujian6@cd02".into()),
                cwd: Some("~".into()),
            }
        );
        assert_eq!(
            parse_ps1_line("[xujian6@cd02 CODE]$").unwrap(),
            Ps1Facts {
                identity: Some("xujian6@cd02".into()),
                cwd: Some("CODE".into()),
            }
        );
        assert_eq!(
            parse_ps1_line("[xujian6@cd02 ~/CODE]$ ls -la").unwrap(),
            Ps1Facts {
                identity: Some("xujian6@cd02".into()),
                cwd: Some("~/CODE".into()),
            }
        );
        assert_eq!(
            parse_ps1_line("carol@dev-box:~/src$ git status").unwrap(),
            Ps1Facts {
                identity: Some("carol@dev-box".into()),
                cwd: Some("~/src".into()),
            }
        );
        assert_eq!(
            parse_ps1_line("xujian@cd02 ~/retr % cd CODE").unwrap(),
            Ps1Facts {
                identity: Some("xujian@cd02".into()),
                cwd: Some("~/retr".into()),
            }
        );
    }

    #[test]
    fn hop_ready_default_endings() {
        assert!(hop_ready_line("Opt>", None));
        assert!(hop_ready_line("  user@jumper Opt>  ", None));
        assert!(hop_ready_line("root@box#", None));
        assert!(hop_ready_line("user@host ~ $", None));
        assert!(hop_ready_line("user@host %", None));
        assert!(!hop_ready_line("", None));
        assert!(!hop_ready_line("Welcome to Ubuntu", None));
        assert!(!hop_ready_line("Last login: Sat from 10.0.0.1", None));
        assert!(!hop_ready_line("carol@dev-box's password:", None));
        assert!(!hop_ready_line("Enter OTP:", None));
        assert!(!hop_ready_line("Are you sure you want to continue connecting (yes/no)?", None));
    }

    #[test]
    fn hop_ready_custom_suffix() {
        assert!(hop_ready_line("user@jumper Opt>", Some("Opt>")));
        assert!(hop_ready_line("Opt>", Some("Opt>")));
        assert!(!hop_ready_line("user@host $", Some("Opt>")));
        assert!(!hop_ready_line("password: Opt>", Some("Opt>")));
        assert!(hop_ready_line("ready$", Some("")));
        assert!(hop_ready_line("ready$", Some("   ")));
    }

    #[test]
    fn launch_password_motd_are_not_ps1() {
        assert!(parse_ps1_line("xujian@mbp ~ % ssh cd02").is_none());
        assert!(parse_ps1_line("carol@dev-box's password:").is_none());
        assert!(parse_ps1_line("Enter passphrase for key:").is_none());
        assert!(parse_ps1_line(
            "Are you sure you want to continue connecting (yes/no)?"
        )
        .is_none());
        assert!(parse_ps1_line("Last login: Sat from 10.0.0.1").is_none());
        assert!(parse_ps1_line("Welcome to Ubuntu").is_none());
        assert!(parse_ps1_line("").is_none());
    }

    #[test]
    fn pre_login_holds_until_the_line_changes() {
        assert!(still_pre_login(
            "xujian@mbp ~ % ssh cd02",
            Some("xujian@mbp ~ % ssh cd02")
        ));
        assert!(!still_pre_login(
            "xujian@cd02 ~/CODE/retr %",
            Some("xujian@mbp ~ % ssh cd02")
        ));
        assert!(
            !still_pre_login("xujian@cd02 ~/src$", Some("xujian@cd02 ~/src$")),
            "process table lagged: last line is already dest PS1"
        );
    }
}
