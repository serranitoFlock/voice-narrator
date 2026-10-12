//! Text -> sound with the configured engine. Port of `synthesize_and_play`
//! (tts-common.sh). Kokoro and Piper stay external processes; cleanup,
//! chunking, concatenation and playback happen here.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::audio;
use crate::clean::{clean_for_tts, split_sentences};
use crate::config::Engine;
use crate::dictionary::Dictionary;
use crate::pipeline::Pipeline;

const PIPER_DEFAULT_VOICE: &str = "es_AR-daniela-high.onnx";
// The user's tuned values (see engram #951): reduce VITS timing artifacts.
const PIPER_NOISE_W_SCALE: &str = "0.3";
const PIPER_SENTENCE_SILENCE: &str = "0.3";

/// Cleans and splits text the way the engines receive it.
pub fn prepare(pipeline: &Pipeline, text: &str) -> Vec<String> {
    let dict = fs::read_to_string(pipeline.config.dictionary_path())
        .ok()
        .map(|content| Dictionary::parse(&content));
    let clean = clean_for_tts(text, dict.as_ref());
    split_sentences(clean.trim_end_matches('\n'))
}

/// Synthesizes and plays `text`; blocks until playback ends. Returns
/// whether something was actually played.
pub fn synthesize_and_play(pipeline: &Pipeline, text: &str) -> bool {
    let sentences = prepare(pipeline, text);
    if sentences.iter().all(|s| s.trim().is_empty()) {
        pipeline.log("exit: clean_text ended up empty after markdown stripping");
        return false;
    }

    let engine = pipeline.config.engine();
    pipeline.log(&format!("engine: {}", engine.as_str()));
    let prefix = pipeline.tmp().join(format!("chunk-{}-", std::process::id()));
    let chunks = match engine {
        Engine::Kokoro => synthesize_kokoro(pipeline, &sentences, &prefix),
        Engine::Piper => synthesize_piper(pipeline, &sentences, &prefix),
    };
    if chunks.is_empty() {
        pipeline.log(&format!("exit: no chunks synthesized successfully (engine: {})", engine.as_str()));
        return false;
    }

    let (clip, errors) = audio::concat_wavs(&chunks);
    for chunk in &chunks {
        let _ = fs::remove_file(chunk);
    }
    for error in errors {
        pipeline.log(&format!("concat: {error}"));
    }
    let Some(clip) = clip else {
        pipeline.log("concat FAILED: no readable chunk");
        return false;
    };

    match audio::play(clip, &pipeline.tmp().join("playback.lock")) {
        Ok(()) => {
            pipeline.log("playback done");
            true
        }
        Err(e) => {
            pipeline.log(&format!("playback FAILED: {e}"));
            false
        }
    }
}

fn kokoro_python(root: &Path) -> PathBuf {
    if cfg!(windows) {
        root.join("kokoro-venv").join("Scripts").join("python.exe")
    } else {
        root.join("kokoro-venv").join("bin").join("python3")
    }
}

/// One process for all sentences: loading the ~325MB model costs ~1.5s.
fn synthesize_kokoro(pipeline: &Pipeline, sentences: &[String], prefix: &Path) -> Vec<PathBuf> {
    let root = pipeline.root();
    let child = Command::new(kokoro_python(root))
        .arg(root.join("scripts").join("kokoro-say.py"))
        .args(["--voice", &pipeline.config.kokoro_voice(), "--lang", "es", "--out-prefix"])
        .arg(prefix)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log_sink(pipeline))
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            pipeline.log(&format!("kokoro FAILED to start: {e}"));
            return Vec::new();
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(sentences.join("\0").as_bytes());
    }
    match child.wait_with_output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect(),
        Err(e) => {
            pipeline.log(&format!("kokoro FAILED: {e}"));
            Vec::new()
        }
    }
}

fn piper_bin() -> PathBuf {
    let name = if cfg!(windows) { "piper.exe" } else { "piper" };
    std::env::home_dir()
        .map(|home| home.join(".local").join("bin").join(name))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Configured model if it still exists on disk, else the default one.
fn piper_model(pipeline: &Pipeline) -> PathBuf {
    let dir = pipeline.root().join("models").join("piper");
    fs::read_to_string(pipeline.root().join(".piper-voice"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && dir.join(v).is_file())
        .map(|v| dir.join(v))
        .unwrap_or_else(|| dir.join(PIPER_DEFAULT_VOICE))
}

/// One process per sentence: Piper loads fast, so no batching wrapper.
fn synthesize_piper(pipeline: &Pipeline, sentences: &[String], prefix: &Path) -> Vec<PathBuf> {
    let model = piper_model(pipeline);
    let mut chunks = Vec::new();
    for (i, sentence) in sentences.iter().enumerate() {
        let out = PathBuf::from(format!("{}{:03}.wav", prefix.display(), i + 1));
        let child = Command::new(piper_bin())
            .arg("-m")
            .arg(&model)
            .arg("-f")
            .arg(&out)
            .args(["--noise-w-scale", PIPER_NOISE_W_SCALE, "--sentence-silence", PIPER_SENTENCE_SILENCE])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(log_sink(pipeline))
            .spawn();
        let ok = child.is_ok_and(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(sentence.as_bytes());
            }
            child.wait().is_ok_and(|status| status.success())
        });
        if ok {
            chunks.push(out);
        } else {
            pipeline.log(&format!("piper FAILED on sentence {}", i + 1));
        }
    }
    chunks
}

/// Engine stderr goes to hook.log, like `2>>"$LOG_FILE"` did.
fn log_sink(pipeline: &Pipeline) -> Stdio {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(pipeline.log_file())
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null())
}
