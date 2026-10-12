//! Paths and logging shared by the hook, the dispatcher and `say`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;

pub struct Pipeline {
    pub config: Config,
}

impl Pipeline {
    /// `$VOICE_PIPELINE_DIR`, or `~/voice-pipeline`.
    pub fn from_env() -> Self {
        let root = std::env::var_os("VOICE_PIPELINE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::home_dir()
                    .expect("could not resolve the home directory")
                    .join("voice-pipeline")
            });
        Self::new(root)
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { config: Config::new(root) }
    }

    pub fn root(&self) -> &Path {
        self.config.root()
    }

    pub fn tmp(&self) -> PathBuf {
        self.root().join("tmp")
    }

    pub fn dispatch_dir(&self) -> PathBuf {
        self.tmp().join("dispatch")
    }

    pub fn log_file(&self) -> PathBuf {
        self.tmp().join("hook.log")
    }

    /// Appends `YYYY-MM-DD HH:MM:SS message` to hook.log. Never fails loudly:
    /// logging must not break narration.
    pub fn log(&self, message: &str) {
        let _ = fs::create_dir_all(self.tmp());
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(self.log_file()) {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(file, "{now} {message}");
        }
    }
}

/// First `n` characters, for log previews.
pub fn preview(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}
