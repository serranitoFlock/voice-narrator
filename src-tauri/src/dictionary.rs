//! `scripts/en-respellings.sed` model: parser, serializer and mutations.
//! Port of the Swift app's `RespellingDictionary`, keeping its on-disk rules:
//!
//! - Everything before the first blank-line separated paragraph that holds a
//!   sed line is an opaque header, preserved verbatim.
//! - Every following paragraph is a block: `# ...` comment lines followed by
//!   `s/[[:<:]]Word[[:>:]]/Respelling/g` lines.
//! - One entry per WORD, not per block: a block covering several words is
//!   split, and each entry gets its own copy of the block comment.
//! - Serialization writes every entry as its own block, one blank line apart.

use serde::Serialize;

const SED_PREFIX: &str = "s/[[:<:]]";
const SED_MIDDLE: &str = "[[:>:]]/";
const SED_SUFFIX: &str = "/g";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub word: String,
    pub respelling: String,
    pub comment_lines: Vec<String>,
    pub sed_lines: Vec<String>,
}

/// What the editor window shows per row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryView {
    pub word: String,
    pub respelling: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dictionary {
    pub header: String,
    pub entries: Vec<Entry>,
}

pub fn parse_sed_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix(SED_PREFIX)?.strip_suffix(SED_SUFFIX)?;
    let (word, respelling) = rest.split_once(SED_MIDDLE)?;
    if word.is_empty() || respelling.is_empty() {
        return None;
    }
    Some((word.to_string(), respelling.to_string()))
}

fn make_sed_line(word: &str, respelling: &str) -> String {
    format!("{SED_PREFIX}{word}{SED_MIDDLE}{respelling}{SED_SUFFIX}")
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Capitalized + lowercase pair. When the word was typed in lowercase, both
/// lines would be identical, so a capitalized variant is generated instead.
pub fn make_sed_line_pair(word: &str, respelling: &str) -> Vec<String> {
    let lower_word = word.to_lowercase();
    let lower_resp = respelling.to_lowercase();
    let first = if word == lower_word {
        make_sed_line(&capitalize_first(word), &capitalize_first(respelling))
    } else {
        make_sed_line(word, respelling)
    };
    vec![first, make_sed_line(&lower_word, &lower_resp)]
}

/// Rejects input that would produce an invalid sed line and break the
/// whole respelling pass.
pub fn validate(word: &str, respelling: &str) -> Result<(), String> {
    for (label, value) in [("palabra", word), ("pronunciación", respelling)] {
        if value.trim().is_empty() {
            return Err(format!("La {label} no puede estar vacía."));
        }
        if value.contains('/') || value.contains('\n') {
            return Err(format!("La {label} no puede contener \"/\" ni saltos de línea."));
        }
    }
    Ok(())
}

impl Dictionary {
    pub fn parse(content: &str) -> Self {
        let mut header_parts: Vec<&str> = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        let mut header_done = false;

        for paragraph in content.split("\n\n") {
            let lines: Vec<&str> = paragraph.split('\n').collect();
            if !lines.iter().any(|l| l.starts_with("s/")) {
                // A stray non-sed paragraph after the blocks never occurs in
                // the real file; it is dropped rather than guessed at.
                if !header_done {
                    header_parts.push(paragraph);
                }
                continue;
            }
            header_done = true;

            let comment_lines: Vec<String> = lines
                .iter()
                .filter(|l| l.starts_with('#'))
                .map(|l| l.to_string())
                .collect();

            // Group sed lines by base word (case-insensitive), in order of
            // first appearance: one entry per word.
            let mut groups: Vec<(String, Vec<String>)> = Vec::new();
            for line in lines.iter().filter(|l| l.starts_with("s/")) {
                let Some((word, _)) = parse_sed_line(line) else { continue };
                let key = word.to_lowercase();
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, group)) => group.push(line.to_string()),
                    None => groups.push((key, vec![line.to_string()])),
                }
            }
            for (_, sed_lines) in groups {
                let (word, respelling) = parse_sed_line(&sed_lines[0]).expect("parsed above");
                entries.push(Entry {
                    word,
                    respelling,
                    comment_lines: comment_lines.clone(),
                    sed_lines,
                });
            }
        }

        Self {
            header: header_parts.join("\n\n"),
            entries,
        }
    }

    pub fn serialize(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.header.is_empty() {
            parts.push(self.header.clone());
        }
        parts.extend(self.entries.iter().map(|e| {
            e.comment_lines
                .iter()
                .chain(e.sed_lines.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        }));
        parts.join("\n\n") + "\n"
    }

    /// Alphabetical, case-insensitive by word.
    pub fn sort(&mut self) {
        self.entries.sort_by_key(|e| e.word.to_lowercase());
    }

    pub fn views(&self) -> Vec<EntryView> {
        self.entries
            .iter()
            .map(|e| EntryView {
                word: e.word.clone(),
                respelling: e.respelling.clone(),
            })
            .collect()
    }

    /// Add, or update in place when the word already exists (case-insensitive).
    /// Updating keeps the entry's comment lines: they hold testing history.
    pub fn upsert(&mut self, word: &str, respelling: &str) {
        let sed_lines = make_sed_line_pair(word, respelling);
        let key = word.to_lowercase();
        match self.entries.iter_mut().find(|e| e.word.to_lowercase() == key) {
            Some(entry) => {
                entry.word = word.to_string();
                entry.respelling = respelling.to_string();
                entry.sed_lines = sed_lines;
            }
            None => self.entries.push(Entry {
                word: word.to_string(),
                respelling: respelling.to_string(),
                comment_lines: vec![format!(
                    "# \"{word}\" -> \"{respelling}\": added via VoiceToggle app."
                )],
                sed_lines,
            }),
        }
    }

    /// Edit from the window: renaming the word drops the old entry first so no
    /// stale duplicate is left behind.
    pub fn edit(&mut self, original: &str, word: &str, respelling: &str) {
        if original.to_lowercase() != word.to_lowercase() {
            self.delete(original);
        }
        self.upsert(word, respelling);
    }

    pub fn delete(&mut self, word: &str) {
        let key = word.to_lowercase();
        self.entries.retain(|e| e.word.to_lowercase() != key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# header line 1\n# header line 2\n\n# more header\n\n# \"art\" -> \"a r t\"\ns/[[:<:]]Art[[:>:]]/A r t/g\ns/[[:<:]]art[[:>:]]/a r t/g\n\n# shared comment\ns/[[:<:]]Spoken[[:>:]]/Spouken/g\ns/[[:<:]]spoken[[:>:]]/spouken/g\ns/[[:<:]]Position[[:>:]]/Posiyion/g\ns/[[:<:]]position[[:>:]]/posiyion/g\n\ns/[[:<:]]Piper[[:>:]]/Páiper/g\ns/[[:<:]]piper[[:>:]]/páiper/g\n";

    fn sed_lines(text: &str) -> Vec<&str> {
        text.lines().filter(|l| l.starts_with("s/")).collect()
    }

    #[test]
    fn parses_a_sed_line() {
        assert_eq!(
            parse_sed_line("s/[[:<:]]Login[[:>:]]/Loguin/g"),
            Some(("Login".into(), "Loguin".into()))
        );
        assert_eq!(parse_sed_line("s/foo/bar/g"), None);
        assert_eq!(parse_sed_line("s/[[:<:]][[:>:]]/x/g"), None);
    }

    #[test]
    fn lowercase_input_gets_a_capitalized_variant() {
        assert_eq!(
            make_sed_line_pair("login", "loguin"),
            vec!["s/[[:<:]]Login[[:>:]]/Loguin/g", "s/[[:<:]]login[[:>:]]/loguin/g"]
        );
    }

    #[test]
    fn mixed_case_input_keeps_typed_form_plus_lowercase() {
        assert_eq!(
            make_sed_line_pair("JSON", "Yeison"),
            vec!["s/[[:<:]]JSON[[:>:]]/Yeison/g", "s/[[:<:]]json[[:>:]]/yeison/g"]
        );
    }

    #[test]
    fn header_is_preserved_verbatim() {
        let dict = Dictionary::parse(SAMPLE);
        assert_eq!(dict.header, "# header line 1\n# header line 2\n\n# more header");
    }

    #[test]
    fn a_block_with_two_words_becomes_two_entries_with_copied_comment() {
        let dict = Dictionary::parse(SAMPLE);
        let words: Vec<&str> = dict.entries.iter().map(|e| e.word.as_str()).collect();
        assert_eq!(words, vec!["Art", "Spoken", "Position", "Piper"]);
        assert_eq!(dict.entries[1].comment_lines, vec!["# shared comment"]);
        assert_eq!(dict.entries[2].comment_lines, vec!["# shared comment"]);
        assert_eq!(dict.entries[2].sed_lines.len(), 2);
        assert!(dict.entries[3].comment_lines.is_empty());
    }

    #[test]
    fn round_trip_keeps_every_sed_rule_in_order() {
        let dict = Dictionary::parse(SAMPLE);
        let out = dict.serialize();
        assert_eq!(sed_lines(&out), sed_lines(SAMPLE));
        assert!(out.starts_with("# header line 1\n# header line 2\n\n# more header\n\n"));
        assert!(out.ends_with("/g\n"));
    }

    #[test]
    fn upsert_adds_a_new_entry_with_an_app_comment() {
        let mut dict = Dictionary::parse(SAMPLE);
        dict.upsert("login", "loguin");
        let entry = dict.entries.last().unwrap();
        assert_eq!(entry.word, "login");
        assert_eq!(
            entry.comment_lines,
            vec!["# \"login\" -> \"loguin\": added via VoiceToggle app."]
        );
        assert_eq!(entry.sed_lines.len(), 2);
    }

    #[test]
    fn upsert_of_existing_word_updates_in_place_and_keeps_comment() {
        let mut dict = Dictionary::parse(SAMPLE);
        dict.upsert("ART", "arte");
        assert_eq!(dict.entries.len(), 4);
        let art = &dict.entries[0];
        assert_eq!(art.respelling, "arte");
        assert_eq!(art.comment_lines, vec!["# \"art\" -> \"a r t\""]);
        assert_eq!(
            art.sed_lines,
            vec!["s/[[:<:]]ART[[:>:]]/arte/g", "s/[[:<:]]art[[:>:]]/arte/g"]
        );
    }

    #[test]
    fn edit_with_renamed_word_leaves_no_duplicate() {
        let mut dict = Dictionary::parse(SAMPLE);
        dict.edit("Piper", "Pipers", "Páipers");
        let words: Vec<&str> = dict.entries.iter().map(|e| e.word.as_str()).collect();
        assert_eq!(words, vec!["Art", "Spoken", "Position", "Pipers"]);
    }

    #[test]
    fn delete_only_removes_that_word() {
        let mut dict = Dictionary::parse(SAMPLE);
        dict.delete("spoken");
        let words: Vec<&str> = dict.entries.iter().map(|e| e.word.as_str()).collect();
        assert_eq!(words, vec!["Art", "Position", "Piper"]);
        assert_eq!(dict.entries[1].comment_lines, vec!["# shared comment"]);
    }

    #[test]
    fn sort_is_case_insensitive() {
        let mut dict = Dictionary::parse(SAMPLE);
        dict.upsert("bash", "bash");
        dict.sort();
        let words: Vec<String> = dict.views().into_iter().map(|v| v.word).collect();
        assert_eq!(words, vec!["Art", "bash", "Piper", "Position", "Spoken"]);
    }

    #[test]
    fn validation_rejects_input_that_breaks_sed() {
        assert!(validate("login", "loguin").is_ok());
        assert!(validate("", "x").is_err());
        assert!(validate("x", "  ").is_err());
        assert!(validate("a/b", "x").is_err());
        assert!(validate("x", "a/b").is_err());
        assert!(validate("x\ny", "z").is_err());
    }

    /// Manual check against the real file: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn real_file_round_trip() {
        let path = std::env::home_dir()
            .unwrap()
            .join("voice-pipeline/scripts/en-respellings.sed");
        let content = std::fs::read_to_string(path).unwrap();
        let dict = Dictionary::parse(&content);
        assert_eq!(sed_lines(&dict.serialize()), sed_lines(&content));
        assert_eq!(dict.header, content.split("\n\n").next().unwrap());
        println!("{} entries, {} sed rules", dict.entries.len(), sed_lines(&content).len());
    }
}
