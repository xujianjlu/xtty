use std::collections::HashMap;
use std::time::{Duration, Instant};

use tty7_core::core::config::PasswordTrigger;

const TAIL_LIMIT: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriggerMatch {
    pub credential: String,
    pub send_enter: bool,
}

#[derive(Default)]
pub(crate) struct PasswordTriggerMatcher {
    tail: Vec<u8>,
    last_fire: HashMap<String, Instant>,
}

impl PasswordTriggerMatcher {
    pub(crate) fn feed(&mut self, bytes: &[u8], rules: &[PasswordTrigger]) -> Option<TriggerMatch> {
        self.feed_at(bytes, rules, Instant::now())
    }

    fn feed_at(
        &mut self,
        bytes: &[u8],
        rules: &[PasswordTrigger],
        now: Instant,
    ) -> Option<TriggerMatch> {
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > TAIL_LIMIT {
            self.tail.drain(..self.tail.len() - TAIL_LIMIT);
        }
        let visible = visible_terminal_text(&self.tail);

        for rule in rules {
            if !rule.enabled
                || rule.pattern.is_empty()
                || rule.credential.is_empty()
                || self.last_fire.get(&rule.credential).is_some_and(|last| {
                    now.saturating_duration_since(*last)
                        < Duration::from_millis(rule.cooldown_ms.max(250))
                })
            {
                continue;
            }

            let matched = if rule.regex {
                regex::Regex::new(&rule.pattern).is_ok_and(|pattern| pattern.is_match(&visible))
            } else {
                visible.contains(&rule.pattern)
            };
            if matched {
                self.last_fire.insert(rule.credential.clone(), now);
                self.tail.clear();
                return Some(TriggerMatch {
                    credential: rule.credential.clone(),
                    send_enter: rule.send_enter,
                });
            }
        }
        None
    }
}

/// Remove common terminal control sequences while retaining printable prompt
/// text, so a coloured `Password:` behaves like a plain one.
fn visible_terminal_text(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            if bytes[i] == b'\r' || bytes[i] == b'\n' || bytes[i] == b'\t' || bytes[i] >= 0x20 {
                out.push(bytes[i]);
            }
            i += 1;
            continue;
        }
        i += 1;
        match bytes.get(i).copied() {
            Some(b'[') => {
                i += 1;
                while i < bytes.len() {
                    let byte = bytes[i];
                    i += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            Some(b']') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            Some(_) => i += 1,
            None => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, regex: bool) -> PasswordTrigger {
        PasswordTrigger {
            pattern: pattern.into(),
            regex,
            credential: "su-search".into(),
            ..PasswordTrigger::default()
        }
    }

    #[test]
    fn matches_literal_across_frames_and_ignores_colour() {
        let mut matcher = PasswordTriggerMatcher::default();
        assert_eq!(
            matcher.feed(b"\x1b[31mPass", &[rule("Password:", false)]),
            None
        );
        assert_eq!(
            matcher.feed(b"word:\x1b[0m ", &[rule("Password:", false)]),
            Some(TriggerMatch {
                credential: "su-search".into(),
                send_enter: true,
            })
        );
    }

    #[test]
    fn regex_matches_a_dynamic_sudo_prompt() {
        let mut matcher = PasswordTriggerMatcher::default();
        assert!(
            matcher
                .feed(
                    b"[sudo] password for search: ",
                    &[rule(r"password for [^:]+:", true)]
                )
                .is_some()
        );
    }

    #[test]
    fn invalid_regex_is_ignored() {
        let mut matcher = PasswordTriggerMatcher::default();
        assert_eq!(matcher.feed(b"Password:", &[rule("(", true)]), None);
    }
}
