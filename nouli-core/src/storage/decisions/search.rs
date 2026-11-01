//! Saved-answer text matching and contextual excerpts.
use super::{SavedDecision, visible_text};

impl SavedDecision {
    #[must_use]
    pub fn matches(&self, query: &str) -> bool {
        let searchable = format!(
            "{} {} {} {} {}",
            visible_text(&self.question.prompt),
            self.source.question,
            self.session_name,
            self.source.turn,
            self.text.join("\n")
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| searchable.contains(&word.to_lowercase()))
    }

    /// A short contextual match when the question title alone does not explain
    /// why this saved answer was found. Keep Unicode boundaries intact.
    pub fn excerpt(&self, query: &str) -> Option<String> {
        let words = query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        if words.is_empty()
            || words
                .iter()
                .all(|word| self.question.prompt.to_lowercase().contains(word))
        {
            return None;
        }
        let line = self
            .text
            .iter()
            .find(|line| words.iter().any(|word| line.to_lowercase().contains(word)))?;
        let line = visible_text(line);
        let lower = line.to_lowercase();
        let byte = words
            .iter()
            .filter_map(|word| lower.find(word))
            .min()
            .unwrap_or(0);
        let mut lowered = 0;
        let mut at = 0;
        for (index, ch) in line.chars().enumerate() {
            let size = ch.to_lowercase().map(char::len_utf8).sum::<usize>();
            if lowered + size > byte {
                at = index;
                break;
            }
            lowered += size;
            at = index + 1;
        }
        let start = at.saturating_sub(24);
        let shown = line.chars().skip(start).take(100).collect::<String>();
        Some(format!(
            "{}{}{}",
            if start > 0 { "…" } else { "" },
            shown,
            if line.chars().count() > start + 100 {
                "…"
            } else {
                ""
            }
        ))
    }
}
