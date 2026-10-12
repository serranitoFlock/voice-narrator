//! The single dispatcher: drains pending markers oldest-first, decides what
//! is still unsaid (dedup), and speaks it. Port of speak.sh's
//! `run_dispatcher`. One instance system-wide, guarded by an OS file lock
//! that is released automatically if the process dies.

use std::fs::{self, File, TryLockError};
use std::path::Path;
use std::time::Duration;

use crate::dedup::remaining;
use crate::markers::{is_older_than, Markers};
use crate::pipeline::{preview, Pipeline};

const ACTIVE_SESSION_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Runs the loop until the queue is empty. Returns immediately if another
/// dispatcher holds the lock. `speak` does the synthesis and playback.
pub fn run(pipeline: &Pipeline, speak: &mut dyn FnMut(&Pipeline, &str) -> bool) {
    let _ = fs::create_dir_all(pipeline.tmp());
    let Ok(lock) = File::create(pipeline.tmp().join("dispatcher.lock")) else {
        return;
    };
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return,
        Err(TryLockError::Error(e)) => {
            pipeline.log(&format!("dispatcher: could not lock: {e}"));
            return;
        }
    }
    let Ok(markers) = Markers::new(pipeline.dispatch_dir()) else {
        return;
    };
    let pid = std::process::id();
    pipeline.log(&format!("dispatcher: started (pid {pid})"));

    while let Some(pending) = markers.next_pending() {
        // Claim before anything slow: if two dispatchers ever coexisted,
        // only one wins the rename, so nothing is said twice.
        let Some(claimed) = markers.claim(&pending) else {
            pipeline.log(&format!("dispatcher: lost the claim race on {}, skipping", name(&pending)));
            continue;
        };
        let Some(marker) = Markers::read(&claimed).filter(|m| !m.text.is_empty()) else {
            pipeline.log(&format!("dispatcher: dropping unreadable/malformed marker {}", name(&claimed)));
            let _ = fs::remove_file(&claimed);
            continue;
        };

        let ts = Markers::timestamp(&claimed);
        let history = markers.finalized();
        let Some(to_say) = remaining(&history, &ts, &marker.session_id, marker.kind, &marker.text) else {
            pipeline.log(&format!(
                "dispatcher: skip (already covered, {} chars): {}...",
                marker.text.chars().count(),
                preview(&marker.text, 60)
            ));
            let _ = fs::remove_file(&claimed);
            continue;
        };
        if to_say != marker.text {
            pipeline.log(&format!(
                "dispatcher: saying unspoken remainder ({} of {} chars): {}...",
                to_say.chars().count(),
                marker.text.chars().count(),
                preview(&to_say, 60)
            ));
        }

        // Checked per item, not once at start: turning voice off cuts the
        // rest of a long backlog almost immediately.
        if pipeline.config.is_disabled() {
            let _ = markers.finalize(&claimed);
            pipeline.log(&format!(
                "dispatcher: registrado sin narrar (voz apagada), {} chars: {}...",
                marker.text.chars().count(),
                preview(&marker.text, 60)
            ));
            continue;
        }

        let text = tag_if_multi_session(pipeline, &to_say, &marker.cwd, &marker.session_id);
        speak(pipeline, &text);
        let _ = markers.finalize(&claimed);
        pipeline.log(&format!("dispatcher: finalized {}", name(&claimed)));
    }
    pipeline.log(&format!("dispatcher: queue empty, terminating (pid {pid})"));
}

fn name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().to_string()
}

/// With more than one session speaking in the last 15 minutes, prefix the
/// text with the session's folder name.
fn tag_if_multi_session(pipeline: &Pipeline, text: &str, cwd: &str, session_id: &str) -> String {
    let dir = pipeline.tmp().join("active-sessions");
    let _ = fs::create_dir_all(&dir);
    if !session_id.is_empty() {
        let _ = fs::write(dir.join(session_id), b"");
    }
    let mut active = 0;
    if let Ok(entries) = fs::read_dir(&dir) {
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            if is_older_than(&path, ACTIVE_SESSION_WINDOW) {
                let _ = fs::remove_file(path);
            } else {
                active += 1;
            }
        }
    }
    let folder = Path::new(cwd).file_name().map(|n| n.to_string_lossy().to_string());
    match folder {
        Some(folder) if active > 1 => {
            pipeline.log(&format!("multiple sessions active ({active}) — tagging with cwd: {folder}"));
            format!("Sesión {folder}: {text}")
        }
        _ => text.to_string(),
    }
}
