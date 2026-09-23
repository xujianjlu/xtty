use super::fuzzy;
use std::collections::HashSet;

const FRECENCY_WEIGHT: f64 = 2.0;

pub(super) struct ReverseSearch {
    /// Snapshot of the lines this search session ranks against. Owned so a
    /// mid-search history-scope switch (nested ssh/jumper) cannot invalidate
    /// match indices, and so a fallback corpus from stashed scopes can live
    /// here without mutating the pane's active history.
    corpus: Vec<String>,
    frecency: Vec<f64>,
    query: String,
    matches: Vec<Match>,
    selected: usize,
}

pub(super) struct Match {
    pub index: usize,
    pub positions: Vec<usize>,
}

pub(super) enum Action {
    Redraw,
    Cancel,
    Accept(Option<String>),
    Run(String),
}

impl ReverseSearch {
    pub(super) fn new(history: &[String], frecency: &[f64]) -> Self {
        let mut scores = frecency.to_vec();
        scores.resize(history.len(), 0.0);
        let mut rs = Self {
            corpus: history.to_vec(),
            frecency: scores,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
        };
        rs.recompute();
        rs
    }

    pub(super) fn query(&self) -> &str {
        &self.query
    }

    pub(super) fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub(super) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn corpus(&self) -> &[String] {
        &self.corpus
    }

    pub(super) fn selected_line(&self) -> Option<&str> {
        self.match_line(self.selected)
    }

    pub(super) fn match_line(&self, match_idx: usize) -> Option<&str> {
        self.matches
            .get(match_idx)
            .map(|m| self.corpus[m.index].as_str())
    }

    fn recompute(&mut self) {
        self.selected = 0;
        let list_all = self.query.trim().is_empty();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut scored: Vec<(f64, Match)> = Vec::new();
        for i in (0..self.corpus.len()).rev() {
            let line = self.corpus[i].as_str();
            if !seen.insert(line) {
                continue;
            }
            let f = self.frecency.get(i).copied().unwrap_or(0.0);
            if list_all {
                scored.push((
                    f,
                    Match {
                        index: i,
                        positions: Vec::new(),
                    },
                ));
            } else if let Some(m) = fuzzy::match_line(line, &self.query) {
                scored.push((
                    f64::from(m.score) + FRECENCY_WEIGHT * f,
                    Match {
                        index: i,
                        positions: m.positions,
                    },
                ));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        self.matches = scored.into_iter().map(|(_, m)| m).collect();
    }

    fn step(&mut self, delta: isize) {
        let last = self.matches.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub(super) fn push_query(&mut self, text: &str) {
        self.query.push_str(text);
        self.recompute();
    }

    pub(super) fn handle_key(&mut self, ks: &gpui::Keystroke) -> Action {
        let m = &ks.modifiers;
        let key = ks.key.as_str();
        // Cmd+R opens the same menu as Ctrl+R; once open, either chord steps.
        let history_step_fwd =
            key == "r" && !m.alt && ((m.control && !m.platform) || (m.platform && !m.control));
        let history_step_back = key == "s" && m.control && !m.platform && !m.alt;
        if history_step_fwd || key == "down" {
            self.step(1);
            Action::Redraw
        } else if history_step_back || key == "up" {
            self.step(-1);
            Action::Redraw
        } else if (m.control && (key == "g" || key == "c")) || key == "escape" {
            Action::Cancel
        } else if key == "enter" || (m.control && (key == "j" || key == "m")) {
            let line = self.selected_line().map(str::to_string);
            match (m.platform, line) {
                (true, Some(line)) => Action::Run(line),
                (_, line) => Action::Accept(line),
            }
        } else if key == "backspace" {
            self.query.pop();
            self.recompute();
            Action::Redraw
        } else {
            Action::Redraw
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> Vec<String> {
        ["git status", "cargo build", "git commit -m x", "cargo test"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    fn flat(h: &[String]) -> Vec<f64> {
        vec![0.0; h.len()]
    }

    fn key(spec: &str) -> gpui::Keystroke {
        gpui::Keystroke::parse(spec).expect("valid keystroke spec")
    }

    #[test]
    fn empty_query_lists_everything_newest_first_under_flat_frecency() {
        let h = history();
        let rs = ReverseSearch::new(&h, &flat(&h));
        let order: Vec<usize> = rs.matches().iter().map(|m| m.index).collect();
        assert_eq!(order, [3, 2, 1, 0]);
        assert_eq!(rs.selected_line(), Some("cargo test"));
    }

    #[test]
    fn query_ranks_the_most_recent_equal_match_first() {
        let h = history();
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        rs.push_query("git");
        assert_eq!(rs.selected_line(), Some("git commit -m x"));
        assert_eq!(rs.matches().len(), 2);
    }

    #[test]
    fn fuzzy_matching_spans_words() {
        let h = history();
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        rs.push_query("gst");
        assert_eq!(rs.selected_line(), Some("git status"));
        assert_eq!(rs.matches()[0].positions, vec![0, 4, 5]);
    }

    #[test]
    fn frecency_outranks_recency_between_equal_text_matches() {
        let h = history();
        let frecency = vec![5.0, 0.0, 0.0, 0.0];
        let mut rs = ReverseSearch::new(&h, &frecency);
        rs.push_query("git");
        assert_eq!(rs.selected_line(), Some("git status"));
    }

    #[test]
    fn duplicates_collapse_to_their_most_recent_occurrence() {
        let h: Vec<String> = ["ls", "make", "ls"].into_iter().map(String::from).collect();
        let rs = ReverseSearch::new(&h, &flat(&h));
        let idx: Vec<usize> = rs.matches().iter().map(|m| m.index).collect();
        assert_eq!(idx, [2, 1]);
    }

    #[test]
    fn ctrl_r_and_arrows_step_through_matches_and_stick_at_the_ends() {
        let h = history();
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        rs.push_query("git");
        assert_eq!(rs.selected(), 0);
        assert!(matches!(rs.handle_key(&key("ctrl-r")), Action::Redraw));
        assert_eq!(rs.selected_line(), Some("git status"));
        rs.handle_key(&key("ctrl-s"));
        assert_eq!(rs.selected(), 0);
        assert!(matches!(rs.handle_key(&key("cmd-r")), Action::Redraw));
        assert_eq!(
            rs.selected_line(),
            Some("git status"),
            "Cmd+R steps the same way Ctrl+R does"
        );
        rs.handle_key(&key("up"));
        assert_eq!(rs.selected(), 0);
        rs.handle_key(&key("down"));
        assert_eq!(rs.selected(), 1);
    }

    #[test]
    fn handle_key_cancel_keys() {
        let h = history();
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        assert!(matches!(rs.handle_key(&key("ctrl-g")), Action::Cancel));
        assert!(matches!(rs.handle_key(&key("ctrl-c")), Action::Cancel));
        assert!(matches!(rs.handle_key(&key("escape")), Action::Cancel));
    }

    #[test]
    fn enter_accepts_and_cmd_enter_runs_the_selection() {
        let h = history();
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        rs.push_query("cargo");
        match rs.handle_key(&key("enter")) {
            Action::Accept(Some(line)) => assert_eq!(line, "cargo test"),
            _ => panic!("expected Accept(Some) with the selected line"),
        }
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        rs.push_query("cargo");
        match rs.handle_key(&key("cmd-enter")) {
            Action::Run(line) => assert_eq!(line, "cargo test"),
            _ => panic!("expected Run with the selected line"),
        }
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        rs.push_query("zzz_nope");
        assert!(rs.matches().is_empty());
        match rs.handle_key(&key("enter")) {
            Action::Accept(None) => {}
            _ => panic!("expected Accept(None) with no match"),
        }
    }

    #[test]
    fn ctrl_j_and_ctrl_m_accept_like_enter() {
        let h = history();
        for chord in ["ctrl-j", "ctrl-m"] {
            let mut rs = ReverseSearch::new(&h, &flat(&h));
            rs.push_query("cargo");
            match rs.handle_key(&key(chord)) {
                Action::Accept(Some(line)) => assert_eq!(line, "cargo test"),
                _ => panic!("{chord} should accept the selected line"),
            }
        }
    }

    #[test]
    fn handle_key_backspace_pops_query_and_re_ranks() {
        let h = history();
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        rs.push_query("gitq");
        assert!(rs.matches().is_empty());
        assert!(matches!(rs.handle_key(&key("backspace")), Action::Redraw));
        assert_eq!(rs.query(), "git");
        assert_eq!(rs.selected_line(), Some("git commit -m x"));
    }

    #[test]
    fn handle_key_other_keys_are_ignored_with_redraw() {
        let h = history();
        let mut rs = ReverseSearch::new(&h, &flat(&h));
        assert!(matches!(rs.handle_key(&key("a")), Action::Redraw));
    }

    #[test]
    fn owns_its_corpus_so_callers_can_drop_the_source_slice() {
        let rs = ReverseSearch::new(&["echo hi".into()], &[1.0]);
        assert_eq!(rs.corpus(), &["echo hi".to_string()]);
        assert_eq!(rs.selected_line(), Some("echo hi"));
    }
}
