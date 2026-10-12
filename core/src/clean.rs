//! Turns raw assistant text into what the TTS engine should read, and splits
//! it into sentences. Port of `clean_for_tts`, `backtick-handler.pl` and the
//! chunking in `synthesize_and_play` (tts-common.sh), kept output-identical:
//! the parity test runs both over the real marker history.
//!
//! Quirks preserved on purpose, because the bash pipeline behaves this way:
//! - every sed/perl stage works line by line, so no pattern spans a newline;
//! - perl ran on raw bytes: `\w` is ASCII-only and lengths count bytes;
//! - `cut -c1-1500` truncates each LINE to 1500 characters, not the text;
//! - the sentence split is per line too, so a blank line after a sentence
//!   stays attached to the start of the next sentence.

use std::sync::OnceLock;

use regex::{Captures, Regex};

use crate::dictionary::Dictionary;

const MAX_LINE_CHARS: usize = 1500;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid built-in regex"))
}

macro_rules! regex {
    ($pattern:expr) => {{
        static CELL: OnceLock<Regex> = OnceLock::new();
        re(&CELL, $pattern)
    }};
}

/// Full cleanup: markdown, inline code, parentheses, then the respelling
/// dictionary (pass `None` to skip it).
pub fn clean_for_tts(text: &str, respellings: Option<&Dictionary>) -> String {
    let text = regex!(r"(?s)```.*?```").replace_all(text, "");
    let text = regex!(r"`([^`\n]*)`").replace_all(&text, |c: &Captures| resolve_backtick(&c[1]));
    let text = regex!(r"\*\*").replace_all(&text, "");
    let text = regex!(r"__").replace_all(&text, "");
    let text = speak_table_rows(&text);
    let text = regex!(r"(?m)^###+ ").replace_all(&text, "");
    let text = regex!(r"(?m)^## ").replace_all(&text, "");
    let text = regex!(r"(?m)^[^\S\n]*[-*][^\S\n]+").replace_all(&text, "");
    let text = regex!(r"(?m)^[^\S\n]*[0-9]+\.[^\S\n]+").replace_all(&text, "");
    let text = regex!(r"\(([^)\n]*)\)").replace_all(&text, ", $1,");
    let text = regex!(r" +,").replace_all(&text, ",");
    let text = regex!(r", *\.").replace_all(&text, ".");
    let text = regex!(r",,+").replace_all(&text, ",");
    let text = regex!(r" +\.").replace_all(&text, ".");
    let text = regex!(r"  +").replace_all(&text, " ");
    match respellings {
        Some(dict) => apply_respellings(&text, dict),
        None => text.into_owned(),
    }
}

/// Lines outside fenced code blocks (``` or ~~~), fences included. An
/// unclosed fence hides everything after it. Works per line, so it holds
/// even when a block is split into one marker per line.
pub fn prose_lines(text: &str) -> Vec<&str> {
    let mut in_fence = false;
    text.split('\n')
        .filter(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                return false;
            }
            !in_fence
        })
        .collect()
}

/// Markdown table rows (lines starting with `|`): every cell ends with a
/// period, so each column becomes its own sentence and gets a pause.
/// Separator rows (`|---|:---:|`) are dropped and empty cells skipped.
fn speak_table_rows(text: &str) -> String {
    text.split('\n').map(|line| table_row(line).unwrap_or_else(|| line.to_string())).collect::<Vec<_>>().join("\n")
}

fn table_row(line: &str) -> Option<String> {
    if !line.trim_start().starts_with('|') {
        return None;
    }
    // Split on unescaped pipes; `\|` is a literal pipe inside a cell.
    let mut cells = vec![String::new()];
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cells.last_mut().unwrap().push('|');
                chars.next();
            }
            '|' => cells.push(String::new()),
            _ => cells.last_mut().unwrap().push(c),
        }
    }
    let cells: Vec<&str> = cells.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
    if cells.iter().all(|c| regex!(r"^:?-+:?$").is_match(c)) {
        return Some(String::new());
    }
    let spoken: Vec<String> = cells
        .iter()
        .map(|cell| {
            let cell = cell.trim_end_matches([',', ';', ':']).trim_end();
            if cell.ends_with(['.', '!', '?']) {
                cell.to_string()
            } else {
                format!("{cell}.")
            }
        })
        .collect();
    Some(spoken.join(" "))
}

/// Applies every sed rule of the dictionary in file order. Rules are plain
/// words (`s/[[:<:]]Word[[:>:]]/Respelling/g`), so the BSD-only word
/// boundaries become `\b` around the escaped word.
pub fn apply_respellings(text: &str, dict: &Dictionary) -> String {
    let mut out = text.to_string();
    for entry in &dict.entries {
        for line in &entry.sed_lines {
            let Some((word, respelling)) = crate::dictionary::parse_sed_line(line) else {
                continue;
            };
            let Ok(rule) = Regex::new(&format!(r"\b{}\b", regex::escape(&word))) else {
                continue;
            };
            out = rule.replace_all(&out, regex::NoExpand(&respelling)).into_owned();
        }
    }
    out
}

/// Inline code span -> what to say instead (backtick-handler.pl).
fn resolve_backtick(content: &str) -> String {
    if content.len() == 1 {
        if let Some(name) = symbol_name(content) {
            return format!(" {name} ");
        }
    }
    if content == "-rf" {
        return " guión erre efe ".to_string();
    }
    // Bare filename: drop the internal "." so neural TTS does not treat it
    // as a sentence end ("tts-common.sh" -> "tts-common sh").
    if regex!(r"^[A-Za-z0-9_-]+\.[a-zA-Z0-9]{1,5}$").is_match(content) {
        return content.replacen('.', " ", 1).replace('_', " ");
    }
    if content.contains('/') || content.contains('~') || content.starts_with('-') || content.len() > 40 {
        return " eso ".to_string();
    }
    content.replace('_', " ")
}

fn symbol_name(symbol: &str) -> Option<&'static str> {
    Some(match symbol {
        "/" => "barra",
        "\\" => "barra invertida",
        "-" => "guión medio",
        "_" => "guión bajo",
        "^" => "circunflejo",
        "~" => "virgulilla",
        "." => "punto",
        "," => "coma",
        ":" => "dos puntos",
        ";" => "punto y coma",
        "|" => "barra vertical",
        "#" => "numeral",
        "@" => "arroba",
        "%" => "por ciento",
        "&" => "ampersand",
        "*" => "asterisco",
        "+" => "más",
        "=" => "igual",
        "<" => "menor que",
        ">" => "mayor que",
        "!" => "signo de exclamación",
        "?" => "signo de interrogación",
        "$" => "signo de dólar",
        "'" => "comilla simple",
        "\"" => "comilla doble",
        "(" | ")" => "paréntesis",
        "[" | "]" => "corchete",
        "{" | "}" => "llave",
        _ => return None,
    })
}

/// Truncates each line to 1500 characters, then splits into sentences after
/// `.`, `!` or `?` followed by whitespace (within a line, including its
/// newline). Empty pieces are dropped.
pub fn split_sentences(clean: &str) -> Vec<String> {
    let truncated: String = clean
        .split_inclusive('\n')
        .map(|line| {
            let (body, newline) = match line.strip_suffix('\n') {
                Some(body) => (body, "\n"),
                None => (line, ""),
            };
            let body: String = body.chars().take(MAX_LINE_CHARS).collect();
            body + newline
        })
        .collect();

    let mut sentences = Vec::new();
    let mut current = String::new();
    for line in truncated.split_inclusive('\n') {
        let mut rest = line;
        while let Some(m) = regex!(r"[.!?][\t\n\x0B\x0C\r ]+").find(rest) {
            // Keep the punctuation, drop the whitespace run after it.
            current.push_str(&rest[..m.start() + 1]);
            sentences.push(std::mem::take(&mut current));
            rest = &rest[m.end()..];
        }
        current.push_str(rest);
    }
    sentences.push(current);
    sentences.retain(|s| !s.is_empty());
    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_code_fences_bold_headings_and_lists() {
        let text = "## Title\n**bold** __x__\n- item\n* other\n  1. first\n```bash\nls\n```\nend";
        assert_eq!(clean_for_tts(text, None), "Title\nbold x\nitem\nother\nfirst\n\nend");
    }

    #[test]
    fn inline_code_cases() {
        assert_eq!(clean_for_tts("usa `/` y `-rf`", None), "usa barra y guión erre efe ");
        assert_eq!(clean_for_tts("abrí `tts-common.sh`", None), "abrí tts-common sh");
        assert_eq!(clean_for_tts("en `~/voice-pipeline` o `--force`", None), "en eso o eso ");
        assert_eq!(clean_for_tts("el `last_assistant_message`", None), "el last assistant message");
    }

    #[test]
    fn parentheses_become_commas() {
        assert_eq!(clean_for_tts("Hola (che) mundo.", None), "Hola, che, mundo.");
        assert_eq!(clean_for_tts("Fin (nota).", None), "Fin, nota.");
    }

    #[test]
    fn respellings_respect_word_boundaries() {
        let dict = Dictionary::parse(
            "# h\n\ns/[[:<:]]Hook[[:>:]]/Juk/g\ns/[[:<:]]hook[[:>:]]/juk/g\n",
        );
        assert_eq!(apply_respellings("el hook y Hooks, hook.", &dict), "el juk y Hooks, juk.");
    }

    #[test]
    fn table_cells_end_with_a_period() {
        assert_eq!(clean_for_tts("| Sistema | Tamaño | Contenido |", None), "Sistema. Tamaño. Contenido.");
        assert_eq!(clean_for_tts("| **macOS** | 4,4 MB | `voice-speak` |", None), "macOS. 4,4 MB. voice-speak.");
    }

    #[test]
    fn table_separator_rows_are_dropped() {
        assert_eq!(clean_for_tts("| a | b |\n|---|:---:|\n| c | d |", None), "a. b.\n\nc. d.");
    }

    #[test]
    fn table_cells_keep_their_own_final_punctuation_and_skip_empty_ones() {
        assert_eq!(clean_for_tts("| ¿Listo? | **Sí.** |  | Fin: |", None), "¿Listo? Sí. Fin.");
    }

    #[test]
    fn table_cells_keep_escaped_pipes() {
        assert_eq!(clean_for_tts(r"| a \| b | c |", None), "a | b. c.");
    }

    #[test]
    fn lines_not_starting_with_a_pipe_are_not_tables() {
        assert_eq!(clean_for_tts("elegí a | b", None), "elegí a | b");
    }

    #[test]
    fn table_columns_become_separate_sentences() {
        let clean = clean_for_tts("| macOS | 4,4 MB | dmg |", None);
        assert_eq!(split_sentences(&clean), vec!["macOS.", "4,4 MB.", "dmg."]);
    }

    #[test]
    fn prose_lines_skip_fenced_code() {
        assert_eq!(prose_lines("a\n```rust\nx\n```\nb\n~~~\ny\n~~~\nc"), vec!["a", "b", "c"]);
        assert_eq!(prose_lines("a\n  ```\nopen"), vec!["a"]);
    }

    #[test]
    fn splits_sentences_per_line() {
        assert_eq!(split_sentences("Uno. Dos! Tres"), vec!["Uno.", "Dos!", "Tres"]);
        assert_eq!(split_sentences("A.\n\nB"), vec!["A.", "\nB"]);
        assert_eq!(split_sentences("sin punto\notra"), vec!["sin punto\notra"]);
    }

    #[test]
    fn truncates_each_line_not_the_whole_text() {
        let long = "a".repeat(1600);
        let text = format!("{long}\n{long}");
        let joined = split_sentences(&text).join("");
        assert_eq!(joined.chars().count(), 1500 * 2 + 1);
    }

    /// Parity with the bash pipeline over real history. Generate the oracle
    /// with the bash oracle script, then:
    /// `VOICE_ORACLE=path/oracle.jsonl cargo test -p voice-core parity -- --ignored`
    #[test]
    #[ignore]
    fn parity_with_bash_pipeline() {
        let path = std::env::var("VOICE_ORACLE").expect("set VOICE_ORACLE");
        let dict_path = std::env::home_dir()
            .unwrap()
            .join("voice-pipeline/scripts/en-respellings.sed");
        let dict = Dictionary::parse(&std::fs::read_to_string(dict_path).unwrap());
        let mut total = 0;
        let mut skipped = 0;
        let mut mismatches = Vec::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let row: serde_json::Value = serde_json::from_str(line).unwrap();
            let text = row["text"].as_str().unwrap();
            // Tables are read column by column on purpose (bash read rows
            // as one run-on line): those texts are not expected to match.
            if text.lines().any(|l| l.trim_start().starts_with('|')) {
                skipped += 1;
                continue;
            }
            let expected_clean = row["clean"].as_str().unwrap();
            let expected: Vec<String> = serde_json::from_value(row["sentences"].clone()).unwrap();
            let clean = clean_for_tts(text, Some(&dict));
            // Bash's $(...) strips trailing newlines from the cleaned text.
            let clean = clean.trim_end_matches('\n');
            let sentences: Vec<String> = split_sentences(clean);
            let expected: Vec<String> = expected.into_iter().filter(|s| !s.is_empty()).collect();
            total += 1;
            if clean != expected_clean || sentences != expected {
                mismatches.push((text.to_string(), expected_clean.to_string(), clean.to_string()));
            }
        }
        for (text, expected, actual) in mismatches.iter().take(3) {
            println!("--- text: {:?}\n--- bash: {:?}\n--- rust: {:?}", &text[..text.len().min(300)], &expected[..expected.len().min(300)], &actual[..actual.len().min(300)]);
        }
        println!("{} of {total} texts differ ({skipped} with tables skipped)", mismatches.len());
        assert!(mismatches.is_empty());
    }
}
