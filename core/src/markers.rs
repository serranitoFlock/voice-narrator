//! Dispatch markers in `tmp/dispatch`, same on-disk format as speak.sh:
//! `<20-digit µs timestamp>.txt.<state>` holding `{text, cwd, session_id,
//! kind}` as JSON. States: `not_dispatched` -> `claimed` -> `finalized`.
//! The zero-padded timestamp makes a plain string sort chronological.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A transcript text block or a question (also markers written before
    /// `kind` existed).
    #[default]
    Block,
    /// The Stop hook's `last_assistant_message`, bundling the whole turn.
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub kind: Kind,
}

/// A finalized marker as the dedup rule sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finalized {
    pub ts: String,
    pub session_id: String,
    pub kind: Kind,
    pub text: String,
}

pub struct Markers {
    dir: PathBuf,
}

fn timestamp_of(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('.').next())
        .unwrap_or_default()
        .to_string()
}

fn has_state(path: &Path, state: &str) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(&format!(".txt.{state}")))
}

fn with_state(path: &Path, state: &str) -> PathBuf {
    let ts = timestamp_of(path);
    path.with_file_name(format!("{ts}.txt.{state}"))
}

/// Microseconds since the epoch, zero-padded to 20 digits.
pub fn now_timestamp() -> String {
    let micros = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_micros();
    format!("{micros:020}")
}

impl Markers {
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn files(&self, state: &str) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| has_state(p, state))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    /// Writes a new marker. With `disabled`, it goes straight to `finalized`:
    /// it seeds the dedup trail but is never narrated (so a backlog made while
    /// voice is off is not read out when it comes back on).
    pub fn create(&self, marker: &Marker, disabled: bool) -> io::Result<PathBuf> {
        let state = if disabled { "finalized" } else { "not_dispatched" };
        let path = self.dir.join(format!("{}.txt.{state}", now_timestamp()));
        fs::write(&path, serde_json::to_vec(marker)?)?;
        Ok(path)
    }

    /// Oldest pending marker, if any.
    pub fn next_pending(&self) -> Option<PathBuf> {
        self.files("not_dispatched").into_iter().next()
    }

    /// Atomically claims a pending marker; `None` if someone else got it.
    pub fn claim(&self, pending: &Path) -> Option<PathBuf> {
        let claimed = with_state(pending, "claimed");
        fs::rename(pending, &claimed).ok().map(|_| claimed)
    }

    pub fn finalize(&self, claimed: &Path) -> io::Result<()> {
        fs::rename(claimed, with_state(claimed, "finalized"))
    }

    pub fn read(path: &Path) -> Option<Marker> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }

    pub fn timestamp(path: &Path) -> String {
        timestamp_of(path)
    }

    /// Every finalized marker with a readable text, oldest first.
    pub fn finalized(&self) -> Vec<Finalized> {
        self.files("finalized")
            .into_iter()
            .filter_map(|path| {
                let marker = Self::read(&path)?;
                (!marker.text.is_empty()).then(|| Finalized {
                    ts: timestamp_of(&path),
                    session_id: marker.session_id,
                    kind: marker.kind,
                    text: marker.text,
                })
            })
            .collect()
    }

    /// Hygiene: deletes finalized/claimed markers older than `retention`.
    pub fn sweep_old(&self, retention: Duration) {
        for state in ["finalized", "claimed"] {
            for path in self.files(state) {
                if is_older_than(&path, retention) {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }

    /// Pending markers this old have no live dispatcher (orphaned by a hard
    /// kill): retire them to finalized so they are never read out in bulk.
    pub fn retire_stranded(&self, age: Duration) -> Vec<PathBuf> {
        let mut retired = Vec::new();
        for path in self.files("not_dispatched") {
            if is_older_than(&path, age) && fs::rename(&path, with_state(&path, "finalized")).is_ok() {
                retired.push(path);
            }
        }
        retired
    }
}

pub fn is_older_than(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn marker(text: &str, kind: Kind) -> Marker {
        Marker { text: text.into(), cwd: "/x".into(), session_id: "s1".into(), kind }
    }

    #[test]
    fn lifecycle_pending_claimed_finalized() {
        let dir = TempDir::new().unwrap();
        let markers = Markers::new(dir.path()).unwrap();
        let created = markers.create(&marker("Hola.", Kind::Block), false).unwrap();
        assert!(created.to_str().unwrap().ends_with(".txt.not_dispatched"));

        let pending = markers.next_pending().unwrap();
        let claimed = markers.claim(&pending).unwrap();
        assert!(markers.claim(&pending).is_none(), "a marker can only be claimed once");
        markers.finalize(&claimed).unwrap();

        assert!(markers.next_pending().is_none());
        let fin = markers.finalized();
        assert_eq!(fin.len(), 1);
        assert_eq!(fin[0].text, "Hola.");
        assert_eq!(fin[0].ts.len(), 20);
    }

    #[test]
    fn disabled_markers_go_straight_to_finalized() {
        let dir = TempDir::new().unwrap();
        let markers = Markers::new(dir.path()).unwrap();
        markers.create(&marker("Shh.", Kind::Stop), true).unwrap();
        assert!(markers.next_pending().is_none());
        assert_eq!(markers.finalized()[0].kind, Kind::Stop);
    }

    #[test]
    fn reads_markers_written_by_speak_sh() {
        let dir = TempDir::new().unwrap();
        let markers = Markers::new(dir.path()).unwrap();
        fs::write(
            dir.path().join("00000000000000000001.txt.finalized"),
            r#"{"text":"Viejo.","cwd":"/x","session_id":"s1"}"#,
        )
        .unwrap();
        let fin = markers.finalized();
        assert_eq!(fin[0].kind, Kind::Block);
        assert_eq!(fin[0].ts, "00000000000000000001");
    }

    #[test]
    fn oldest_pending_comes_first() {
        let dir = TempDir::new().unwrap();
        let markers = Markers::new(dir.path()).unwrap();
        fs::write(dir.path().join("00000000000000000002.txt.not_dispatched"), "{}").unwrap();
        fs::write(dir.path().join("00000000000000000001.txt.not_dispatched"), "{}").unwrap();
        assert_eq!(Markers::timestamp(&markers.next_pending().unwrap()), "00000000000000000001");
    }
}
