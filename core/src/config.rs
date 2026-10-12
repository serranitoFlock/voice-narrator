//! File-based configuration shared with the shell pipeline (speak.sh,
//! tts-common.sh). Every setting is a dotfile under the pipeline directory;
//! this module mirrors the exact read/fallback rules of the original Swift
//! VoiceToggle app so both apps and the scripts agree on the same state.

use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_KOKORO_VOICE: &str = "em_santa";
pub const DEFAULT_MARKER_RETENTION_DAYS: u32 = 7;
pub const DEFAULT_LOG_RETENTION_DAYS: u32 = 14;
pub const RETENTION_OPTIONS: [u32; 5] = [1, 3, 7, 14, 30];

/// Fixed Kokoro voice catalog: (menu label, voice code).
pub const KOKORO_VOICES: [(&str, &str); 3] = [
    ("Dora (mujer)", "ef_dora"),
    ("Alex (hombre)", "em_alex"),
    ("Santa (hombre)", "em_santa"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Kokoro,
    Piper,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Kokoro => "kokoro",
            Engine::Piper => "piper",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// Marker history (`.retention-days`).
    Markers,
    /// Hook log (`.log-retention-days`).
    Log,
}

/// Snapshot of every setting the tray menu renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub disabled: bool,
    pub engine: Engine,
    pub kokoro_voice: String,
    pub piper_voices: Vec<String>,
    pub piper_voice: Option<String>,
    pub marker_retention_days: u32,
    pub log_retention_days: u32,
}

pub struct Config {
    root: PathBuf,
}

impl Config {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dictionary_path(&self) -> PathBuf {
        self.root.join("scripts").join("en-respellings.sed")
    }

    pub fn is_disabled(&self) -> bool {
        self.root.join(".disabled").exists()
    }

    /// Disabling creates an empty `.disabled` flag; enabling removes it.
    pub fn set_disabled(&self, disabled: bool) -> std::io::Result<()> {
        let flag = self.root.join(".disabled");
        if disabled {
            fs::write(flag, b"")
        } else {
            match fs::remove_file(flag) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        }
    }

    pub fn engine(&self) -> Engine {
        match self.read_trimmed(".engine").as_deref() {
            Some("piper") => Engine::Piper,
            _ => Engine::Kokoro,
        }
    }

    /// Same effect as `scripts/engine.sh <engine>`, without depending on bash.
    pub fn set_engine(&self, engine: Engine) -> std::io::Result<()> {
        self.write(".engine", engine.as_str())
    }

    pub fn kokoro_voice(&self) -> String {
        self.read_trimmed(".kokoro-voice")
            .unwrap_or_else(|| DEFAULT_KOKORO_VOICE.to_string())
    }

    pub fn set_kokoro_voice(&self, voice: &str) -> std::io::Result<()> {
        self.write(".kokoro-voice", voice)
    }

    pub fn piper_voices(&self) -> Vec<String> {
        let dir = self.root.join("models").join("piper");
        let mut files: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|name| name.ends_with(".onnx"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    /// The configured model if it still exists; otherwise the only model on
    /// disk when there is exactly one (tts-common.sh's own fallback).
    pub fn piper_voice(&self) -> Option<String> {
        let files = self.piper_voices();
        if let Some(configured) = self.read_trimmed(".piper-voice") {
            if files.contains(&configured) {
                return Some(configured);
            }
        }
        match files.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        }
    }

    pub fn set_piper_voice(&self, file: &str) -> std::io::Result<()> {
        self.write(".piper-voice", file)
    }

    pub fn retention_days(&self, kind: Retention) -> u32 {
        let (file, default) = Self::retention_file(kind);
        self.read_trimmed(file)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    pub fn set_retention_days(&self, kind: Retention, days: u32) -> std::io::Result<()> {
        let (file, _) = Self::retention_file(kind);
        self.write(file, &days.to_string())
    }

    fn retention_file(kind: Retention) -> (&'static str, u32) {
        match kind {
            Retention::Markers => (".retention-days", DEFAULT_MARKER_RETENTION_DAYS),
            Retention::Log => (".log-retention-days", DEFAULT_LOG_RETENTION_DAYS),
        }
    }

    fn read_trimmed(&self, name: &str) -> Option<String> {
        let content = fs::read_to_string(self.root.join(name)).ok()?;
        let trimmed = content.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn write(&self, name: &str, value: &str) -> std::io::Result<()> {
        fs::write(self.root.join(name), value)
    }

    pub fn state(&self) -> State {
        State {
            disabled: self.is_disabled(),
            engine: self.engine(),
            kokoro_voice: self.kokoro_voice(),
            piper_voices: self.piper_voices(),
            piper_voice: self.piper_voice(),
            marker_retention_days: self.retention_days(Retention::Markers),
            log_retention_days: self.retention_days(Retention::Log),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Config) {
        let dir = TempDir::new().unwrap();
        let config = Config::new(dir.path());
        (dir, config)
    }

    fn write(dir: &TempDir, name: &str, content: &str) {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(dir: &TempDir, name: &str) -> String {
        fs::read_to_string(dir.path().join(name)).unwrap()
    }

    #[test]
    fn disabled_flag_is_the_presence_of_the_file() {
        let (dir, config) = setup();
        assert!(!config.is_disabled());

        config.set_disabled(true).unwrap();
        assert!(dir.path().join(".disabled").exists());
        assert!(config.is_disabled());

        config.set_disabled(false).unwrap();
        assert!(!dir.path().join(".disabled").exists());
        assert!(!config.is_disabled());
    }

    #[test]
    fn enabling_twice_is_not_an_error() {
        let (_dir, config) = setup();
        config.set_disabled(false).unwrap();
        assert!(!config.is_disabled());
    }

    #[test]
    fn engine_defaults_to_kokoro_unless_file_says_piper() {
        let (dir, config) = setup();
        assert_eq!(config.engine(), Engine::Kokoro);

        write(&dir, ".engine", "  piper\n");
        assert_eq!(config.engine(), Engine::Piper);

        write(&dir, ".engine", "something-else");
        assert_eq!(config.engine(), Engine::Kokoro);
    }

    #[test]
    fn set_engine_writes_bare_value_like_engine_sh() {
        let (dir, config) = setup();
        config.set_engine(Engine::Piper).unwrap();
        assert_eq!(read(&dir, ".engine"), "piper");
        assert_eq!(config.engine(), Engine::Piper);
    }

    #[test]
    fn kokoro_voice_defaults_and_round_trips() {
        let (dir, config) = setup();
        assert_eq!(config.kokoro_voice(), DEFAULT_KOKORO_VOICE);

        write(&dir, ".kokoro-voice", "   \n");
        assert_eq!(config.kokoro_voice(), DEFAULT_KOKORO_VOICE);

        config.set_kokoro_voice("ef_dora").unwrap();
        assert_eq!(read(&dir, ".kokoro-voice"), "ef_dora");
        assert_eq!(config.kokoro_voice(), "ef_dora");
    }

    #[test]
    fn piper_voices_lists_sorted_onnx_files_only() {
        let (dir, config) = setup();
        assert!(config.piper_voices().is_empty());

        write(&dir, "models/piper/b.onnx", "");
        write(&dir, "models/piper/a.onnx", "");
        write(&dir, "models/piper/a.onnx.json", "");
        assert_eq!(config.piper_voices(), vec!["a.onnx", "b.onnx"]);
    }

    #[test]
    fn piper_voice_uses_configured_file_only_if_it_exists() {
        let (dir, config) = setup();
        write(&dir, "models/piper/a.onnx", "");
        write(&dir, "models/piper/b.onnx", "");
        assert_eq!(config.piper_voice(), None);

        config.set_piper_voice("b.onnx").unwrap();
        assert_eq!(read(&dir, ".piper-voice"), "b.onnx");
        assert_eq!(config.piper_voice().as_deref(), Some("b.onnx"));

        write(&dir, ".piper-voice", "gone.onnx");
        assert_eq!(config.piper_voice(), None);
    }

    #[test]
    fn piper_voice_falls_back_to_the_only_model_on_disk() {
        let (dir, config) = setup();
        write(&dir, "models/piper/only.onnx", "");
        write(&dir, ".piper-voice", "gone.onnx");
        assert_eq!(config.piper_voice().as_deref(), Some("only.onnx"));
    }

    #[test]
    fn retention_defaults_when_missing_or_not_an_integer() {
        let (dir, config) = setup();
        assert_eq!(config.retention_days(Retention::Markers), 7);
        assert_eq!(config.retention_days(Retention::Log), 14);

        write(&dir, ".retention-days", "abc");
        write(&dir, ".log-retention-days", "");
        assert_eq!(config.retention_days(Retention::Markers), 7);
        assert_eq!(config.retention_days(Retention::Log), 14);
    }

    #[test]
    fn retention_writes_bare_integer_to_its_own_file() {
        let (dir, config) = setup();
        config.set_retention_days(Retention::Markers, 30).unwrap();
        config.set_retention_days(Retention::Log, 1).unwrap();
        assert_eq!(read(&dir, ".retention-days"), "30");
        assert_eq!(read(&dir, ".log-retention-days"), "1");
        assert_eq!(config.retention_days(Retention::Markers), 30);
        assert_eq!(config.retention_days(Retention::Log), 1);
    }
}
