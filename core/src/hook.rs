//! Claude Code hook entry point (Stop, PostToolUse, PreToolUse on
//! AskUserQuestion). Port of speak.sh's writer side: decide WHAT to say and
//! drop markers; the dispatcher decides whether it was already said.
//! Never fails: every error is logged and swallowed.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::config::Retention;
use crate::markers::{is_older_than, Kind, Marker, Markers};
use crate::pipeline::{preview, Pipeline};

const STALE_AUDIO_AGE: Duration = Duration::from_secs(10 * 60);
const STRANDED_MARKER_AGE: Duration = Duration::from_secs(90 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Payload {
    pub session_id: String,
    pub cwd: String,
    pub transcript_path: String,
    pub last_assistant_message: String,
    pub tool_name: String,
    pub tool_input: Value,
}

pub fn run(pipeline: &Pipeline, input: &str) {
    pipeline.log("invoked (voice-speak hook)");
    housekeeping(pipeline);

    let payload: Payload = match serde_json::from_str(input) {
        Ok(payload) => payload,
        Err(e) => {
            pipeline.log(&format!("exit: unreadable hook payload: {e}"));
            return;
        }
    };
    let markers = match Markers::new(pipeline.dispatch_dir()) {
        Ok(markers) => markers,
        Err(e) => {
            pipeline.log(&format!("exit: dispatch dir unavailable: {e}"));
            return;
        }
    };

    if !payload.last_assistant_message.is_empty() {
        pipeline.log(&format!("branch: Stop ({} chars)", payload.last_assistant_message.chars().count()));
        // Catch up first: the most reliable point to pick up blocks a
        // PostToolUse missed. Whatever the transcript still lacks, the Stop
        // marker's remainder covers.
        catch_up_new_blocks(pipeline, &markers, &payload);
        create_marker(pipeline, &markers, &payload, &payload.last_assistant_message, Kind::Stop);
    } else if payload.tool_name == "AskUserQuestion" {
        pipeline.log("branch: PreToolUse AskUserQuestion");
        catch_up_new_blocks(pipeline, &markers, &payload);
        let question = question_text(&payload.tool_input);
        if !question.is_empty() {
            create_marker(pipeline, &markers, &payload, &question, Kind::Block);
        }
    } else {
        pipeline.log("branch: PostToolUse");
        catch_up_new_blocks(pipeline, &markers, &payload);
    }
}

fn create_marker(pipeline: &Pipeline, markers: &Markers, payload: &Payload, text: &str, kind: Kind) {
    if text.is_empty() {
        return;
    }
    let marker = Marker {
        text: text.to_string(),
        cwd: payload.cwd.clone(),
        session_id: payload.session_id.clone(),
        kind,
    };
    match markers.create(&marker, pipeline.config.is_disabled()) {
        Ok(path) => pipeline.log(&format!(
            "marker: created {} ({} chars): {}...",
            path.file_name().unwrap_or_default().to_string_lossy(),
            text.chars().count(),
            preview(text, 60)
        )),
        Err(e) => pipeline.log(&format!("marker: could not create: {e}")),
    }
}

/// "Pregunta: <q>. Opciones: a, b." per question, blank-line separated.
fn question_text(tool_input: &Value) -> String {
    let Some(questions) = tool_input.get("questions").and_then(Value::as_array) else {
        return String::new();
    };
    questions
        .iter()
        .map(|q| {
            let question = q.get("question").and_then(Value::as_str).unwrap_or_default();
            let labels: Vec<&str> = q
                .get("options")
                .and_then(Value::as_array)
                .map(|opts| opts.iter().filter_map(|o| o.get("label").and_then(Value::as_str)).collect())
                .unwrap_or_default();
            if labels.is_empty() {
                format!("Pregunta: {question}.")
            } else {
                format!("Pregunta: {question}. Opciones: {}.", labels.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Every assistant text block in the transcript, in order. Unreadable
/// lines are skipped instead of discarding the whole transcript.
fn transcript_blocks(path: &Path) -> Option<Vec<String>> {
    let content = fs::read_to_string(path).ok()?;
    Some(
        content
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("assistant"))
            .flat_map(|entry| {
                entry
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|item| item.get("text").and_then(Value::as_str).map(str::to_string))
            .collect(),
    )
}

/// Queues the transcript text blocks this session has not looked at yet,
/// tracked with a per-session counter. One marker per LINE of each block,
/// as speak.sh did (`while read -r`), so paragraphs are dispatched and
/// deduplicated separately.
fn catch_up_new_blocks(pipeline: &Pipeline, markers: &Markers, payload: &Payload) {
    if payload.session_id.is_empty() {
        pipeline.log("exit: no session_id for catch-up");
        return;
    }
    let transcript = PathBuf::from(&payload.transcript_path);
    if payload.transcript_path.is_empty() || !transcript.is_file() {
        pipeline.log(&format!("exit: transcript_path unavailable: {}", payload.transcript_path));
        return;
    }

    let position_dir = pipeline.tmp().join("spoken-position");
    if fs::create_dir_all(&position_dir).is_err() {
        return;
    }
    let position_file = position_dir.join(&payload.session_id);

    // Serialize read -> compute -> write -> dispatch per session: the three
    // hooks run this concurrently against the same counter.
    let Ok(lock) = File::create(position_dir.join(format!(".{}.flock", payload.session_id))) else {
        return;
    };
    let mut locked = false;
    for _ in 0..20 {
        if lock.try_lock().is_ok() {
            locked = true;
            break;
        }
        sleep(Duration::from_millis(100));
    }
    if !locked {
        pipeline.log(&format!("catch-up: per-session lock busy for {}, skipping this pass", payload.session_id));
        return;
    }

    let Some(blocks) = transcript_blocks(&transcript) else {
        return;
    };
    let total = blocks.len();
    let Some(spoken) = fs::read_to_string(&position_file).ok() else {
        // First sighting: only narrate going forward, never the backlog.
        let _ = fs::write(&position_file, total.to_string());
        pipeline.log(&format!(
            "first sighting of session {} — baselining at {total}, nothing to speak yet",
            payload.session_id
        ));
        return;
    };
    let spoken: usize = spoken.trim().parse().unwrap_or(0);
    if total <= spoken {
        return;
    }
    // Record the position before dispatching so an overlapping call does not
    // queue the same blocks again.
    let _ = fs::write(&position_file, total.to_string());
    pipeline.log(&format!("catch-up: {} new block(s) for session {}", total - spoken, payload.session_id));

    for block in &blocks[spoken..] {
        for line in block.split('\n') {
            create_marker(pipeline, markers, payload, line, Kind::Block);
        }
    }
}

/// Age-based cleanups, cheap enough to run on every hook call.
fn housekeeping(pipeline: &Pipeline) {
    // Orphaned audio chunks from a killed synthesis.
    if let Ok(entries) = fs::read_dir(pipeline.tmp()) {
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let audio = (name.starts_with("chunk-") || name.starts_with("last-response-")) && name.ends_with(".wav");
            if audio && is_older_than(&path, STALE_AUDIO_AGE) {
                let _ = fs::remove_file(path);
            }
        }
    }

    if let Ok(markers) = Markers::new(pipeline.dispatch_dir()) {
        let days = pipeline.config.retention_days(Retention::Markers);
        markers.sweep_old(DAY * days);
        for stranded in markers.retire_stranded(STRANDED_MARKER_AGE) {
            pipeline.log(&format!(
                "sweep: retired stranded marker {} — 90+ min old, no live dispatcher",
                stranded.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }

    trim_log(pipeline);
}

/// Drops hook.log lines older than the configured retention. Lines start
/// with an ISO date, so a string comparison on the first 10 chars works.
fn trim_log(pipeline: &Pipeline) {
    let days = pipeline.config.retention_days(Retention::Log);
    let cutoff = (chrono::Local::now() - chrono::Duration::days(days as i64)).format("%Y-%m-%d").to_string();
    let path = pipeline.log_file();
    let Ok(content) = fs::read_to_string(&path) else {
        return;
    };
    if content.get(..10).is_none_or(|first| first >= cutoff.as_str()) {
        return;
    }
    let kept: String = content
        .split_inclusive('\n')
        .filter(|line| line.get(..10).is_some_and(|date| date >= cutoff.as_str()))
        .collect();
    let tmp = path.with_extension("log.tmp");
    if fs::write(&tmp, kept).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}
