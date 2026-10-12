//! What part of a dispatch marker still needs to be said. Port of
//! `scripts/dedup.jq` (see its header for the 2026-10-12 incident that
//! motivated these rules). Scoped to the marker's own session and turn:
//!
//! - A turn is everything a session finalized after its latest Stop marker.
//! - block: skipped if the exact text was already finalized in this turn, or
//!   if it is a non-trivial piece of the previous Stop message (a block that
//!   reached the transcript late and the Stop already said).
//! - stop: this turn's blocks are subtracted and only the remainder is said.

use std::sync::OnceLock;

use regex::Regex;

use crate::markers::{Finalized, Kind};

/// Late blocks shorter than this (after dropping punctuation) are said again
/// rather than risk swallowing a short new block ("Listo.").
const LATE_BLOCK_MIN: usize = 20;

/// Length of what is left after dropping whitespace, punctuation and
/// symbols: decides whether a remainder is worth saying.
fn meaningful_len(text: &str) -> usize {
    static NOISE: OnceLock<Regex> = OnceLock::new();
    let noise = NOISE.get_or_init(|| Regex::new(r"[\s\p{P}\p{S}]+").unwrap());
    noise.replace_all(text, "").chars().count()
}

/// Returns the text to speak, or `None` when the marker is already covered.
pub fn remaining(history: &[Finalized], ts: &str, session: &str, kind: Kind, text: &str) -> Option<String> {
    let earlier: Vec<&Finalized> = history
        .iter()
        .filter(|f| f.session_id == session && f.ts.as_str() < ts && !f.text.is_empty())
        .collect();
    let last_stop = earlier.iter().filter(|f| f.kind == Kind::Stop).max_by(|a, b| a.ts.cmp(&b.ts));
    let turn: Vec<&str> = earlier
        .iter()
        .filter(|f| f.kind != Kind::Stop && last_stop.is_none_or(|stop| f.ts > stop.ts))
        .map(|f| f.text.as_str())
        .collect();

    match kind {
        Kind::Stop => {
            let mut blocks = turn;
            blocks.sort_by_key(|b| std::cmp::Reverse(b.chars().count()));
            let mut rest = text.to_string();
            for block in blocks {
                rest = rest.replacen(block, "", 1);
            }
            if meaningful_len(&rest) == 0 {
                return None;
            }
            static BLANKS: OnceLock<Regex> = OnceLock::new();
            let blanks = BLANKS.get_or_init(|| Regex::new(r"\n{3,}").unwrap());
            Some(blanks.replace_all(&rest, "\n\n").trim().to_string())
        }
        Kind::Block => {
            if turn.contains(&text) {
                return None;
            }
            let late = last_stop.is_some_and(|stop| {
                meaningful_len(text) >= LATE_BLOCK_MIN && stop.text.contains(text)
            });
            (!late).then(|| text.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fin(ts: &str, session: &str, kind: Kind, text: &str) -> Finalized {
        Finalized {
            ts: ts.to_string(),
            session_id: session.to_string(),
            kind,
            text: text.to_string(),
        }
    }

    const PARA: &str =
        "Siguiente paso: portar el pipeline (T3) o hacer el autoarranque (T4). ¿Cuál preferís?";

    #[test]
    fn a_block_from_another_session_never_covers_a_new_one() {
        let h = [
            fin("100", "old", Kind::Block, "¿Cuál preferís?"),
            fin("150", "old", Kind::Stop, "Algo. ¿Cuál preferís?"),
        ];
        assert_eq!(remaining(&h, "200", "s1", Kind::Block, PARA).as_deref(), Some(PARA));
    }

    #[test]
    fn an_earlier_turn_of_the_same_session_never_covers_a_new_block() {
        let h = [
            fin("100", "s1", Kind::Block, "¿Cuál preferís?"),
            fin("150", "s1", Kind::Stop, "Algo. ¿Cuál preferís?"),
        ];
        assert_eq!(remaining(&h, "200", "s1", Kind::Block, PARA).as_deref(), Some(PARA));
    }

    #[test]
    fn exact_duplicate_in_the_same_turn_is_skipped() {
        let h = [fin("100", "s1", Kind::Block, "Hola.")];
        assert_eq!(remaining(&h, "200", "s1", Kind::Block, "Hola."), None);
    }

    #[test]
    fn stop_fully_covered_by_this_turns_blocks_is_skipped() {
        let h = [
            fin("100", "s1", Kind::Block, "Primer párrafo."),
            fin("110", "s1", Kind::Block, "Segundo párrafo."),
        ];
        let stop = "Primer párrafo.\n\nSegundo párrafo.";
        assert_eq!(remaining(&h, "200", "s1", Kind::Stop, stop), None);
    }

    #[test]
    fn stop_says_only_the_block_that_never_arrived() {
        let h = [
            fin("100", "s1", Kind::Block, "Primer párrafo."),
            fin("110", "s1", Kind::Block, "Segundo párrafo."),
        ];
        let stop = "Primer párrafo.\n\nSegundo párrafo.\n\nÚltimo párrafo.";
        assert_eq!(remaining(&h, "200", "s1", Kind::Stop, stop).as_deref(), Some("Último párrafo."));
    }

    #[test]
    fn blocks_from_earlier_turns_are_not_subtracted_from_a_stop() {
        let h = [
            fin("050", "s1", Kind::Stop, "Turno anterior. Primer párrafo."),
            fin("100", "s1", Kind::Block, "Otro."),
        ];
        let stop = "Otro.\n\nPrimer párrafo.";
        assert_eq!(remaining(&h, "200", "s1", Kind::Stop, stop).as_deref(), Some("Primer párrafo."));
    }

    #[test]
    fn stop_with_nothing_spoken_yet_is_said_whole() {
        assert_eq!(remaining(&[], "200", "s1", Kind::Stop, "Todo junto.").as_deref(), Some("Todo junto."));
    }

    #[test]
    fn late_block_already_said_by_the_previous_stop_is_skipped() {
        let h = [
            fin("100", "s1", Kind::Block, "Primer párrafo."),
            fin("110", "s1", Kind::Stop, "Primer párrafo.\n\nEste párrafo llegó tarde al transcript."),
        ];
        let late = "Este párrafo llegó tarde al transcript.";
        assert_eq!(remaining(&h, "200", "s1", Kind::Block, late), None);
        assert_eq!(remaining(&h, "200", "s1", Kind::Block, "Listo.").as_deref(), Some("Listo."));
    }

    #[test]
    fn markers_after_the_current_one_are_ignored() {
        let h = [fin("300", "s1", Kind::Block, "Hola.")];
        assert_eq!(remaining(&h, "200", "s1", Kind::Block, "Hola.").as_deref(), Some("Hola."));
    }

    #[test]
    fn empty_history_says_the_text() {
        assert_eq!(remaining(&[], "200", "s1", Kind::Block, "Hola.").as_deref(), Some("Hola."));
    }
}
