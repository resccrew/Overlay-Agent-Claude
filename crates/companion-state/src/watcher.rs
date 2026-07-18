//! Finds and tails the live Claude Code JSONL transcript on disk.
//!
//! Pure std, no notify/inotify dependency: this crate's whole point is to
//! build without the webview toolchain, and a polling tailer is simple
//! enough (and the transcript file is small/local) that inotify wouldn't
//! buy much. See `docs/claude-code-transcript.md` for the on-disk layout
//! this was reverse-engineered from.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use serde_json::Value;

/// Walks `projects_root` (normally `~/.claude/projects`) one level of
/// project-dirs deep and returns the most-recently-modified `*.jsonl` file
/// found, if any. That's the transcript of whichever Claude Code session
/// last wrote something, across all projects/windows.
pub fn latest_transcript_file(projects_root: &Path) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;

    let project_dirs = std::fs::read_dir(projects_root).ok()?;
    for project_dir in project_dirs.flatten() {
        let Ok(file_type) = project_dir.file_type() else { continue };
        if !file_type.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(project_dir.path()) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else { continue };
            let Ok(modified) = metadata.modified() else { continue };
            if best.as_ref().is_none_or(|(t, _)| modified > *t) {
                best = Some((modified, path));
            }
        }
    }

    best.map(|(_, path)| path)
}

/// Incrementally reads newly-appended, complete JSON lines from a
/// transcript file. Safe to poll faster than the writer appends: a
/// trailing partial line (the writer mid-`write()`) is held back in
/// `leftover` until the rest of it shows up.
pub struct Tailer {
    file: File,
    pos: u64,
    leftover: String,
}

impl Tailer {
    /// Opens `path` and starts reading from the beginning (replays
    /// everything already in the file, then continues live).
    pub fn open_from_start(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        Ok(Self { file, pos: 0, leftover: String::new() })
    }

    /// Opens `path` and starts reading from the current end of file (only
    /// sees lines appended after this call) -- what the real companion
    /// app wants, so it doesn't replay an entire past session's history
    /// as a burst of state changes on startup.
    pub fn open_at_end(path: &Path) -> std::io::Result<Self> {
        let mut file = File::open(path)?;
        let pos = file.seek(SeekFrom::End(0))?;
        Ok(Self { file, pos, leftover: String::new() })
    }

    /// Reads whatever has been appended since the last call and returns
    /// any newly-complete lines, parsed as JSON. Malformed lines (partial
    /// writes that don't parse yet, or truly corrupt ones) are silently
    /// skipped -- same tolerance as the Node prototype.
    pub fn poll(&mut self) -> std::io::Result<Vec<Value>> {
        self.file.seek(SeekFrom::Start(self.pos))?;
        let mut buf = String::new();
        let read = self.file.read_to_string(&mut buf)?;
        if read == 0 {
            return Ok(Vec::new());
        }
        self.pos += read as u64;

        self.leftover.push_str(&buf);

        let mut lines: Vec<Value> = Vec::new();
        let mut rest = self.leftover.as_str();
        let mut consumed = 0;
        while let Some(idx) = rest.find('\n') {
            let line = &rest[..idx];
            consumed += idx + 1;
            rest = &rest[idx + 1..];
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                if let Ok(value) = serde_json::from_str(trimmed) {
                    lines.push(value);
                }
            }
        }
        self.leftover.drain(..consumed);

        Ok(lines)
    }
}

/// Ties `latest_transcript_file` + `Tailer` together into the actual
/// polling loop the real app runs: periodically re-check whether a
/// different transcript has become the most-recently-modified one (the
/// user switched sessions/projects), and read whatever's newly appended to
/// whichever one is current. GUI-independent by design -- `main.rs` just
/// wraps this in a thread and forwards `poll()`'s output through
/// `StateMachine` + `AppHandle::emit`, but every bit of "what to tail and
/// when to switch" logic lives here where it can be exercised by a plain
/// `cargo test` with a temp directory, no webview required.
pub struct ProjectsWatcher {
    projects_root: PathBuf,
    rescan_interval: std::time::Duration,
    current: Option<(PathBuf, Tailer)>,
    last_rescan: Instant,
}

impl ProjectsWatcher {
    pub fn new(projects_root: PathBuf, rescan_interval: std::time::Duration) -> Self {
        Self {
            projects_root,
            rescan_interval,
            current: None,
            // Due immediately: the first `poll()` should scan right away
            // rather than waiting a full interval before ever looking.
            last_rescan: Instant::now() - rescan_interval,
        }
    }

    /// Call this on a timer (e.g. every 500ms). Internally rate-limits the
    /// expensive directory rescan to `rescan_interval`, but always reads
    /// any new lines from the currently-tailed file. Returns newly-arrived,
    /// successfully-parsed transcript lines in order.
    pub fn poll(&mut self) -> Vec<Value> {
        if self.last_rescan.elapsed() >= self.rescan_interval {
            self.last_rescan = Instant::now();
            if let Some(latest) = latest_transcript_file(&self.projects_root) {
                let already_watching = self.current.as_ref().map(|(p, _)| p) == Some(&latest);
                if !already_watching {
                    if let Ok(tailer) = Tailer::open_at_end(&latest) {
                        self.current = Some((latest, tailer));
                    }
                }
            }
        }

        match self.current.as_mut() {
            Some((_, tailer)) => tailer.poll().unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// The file currently being tailed, if any -- mainly for tests/logging.
    pub fn current_path(&self) -> Option<&Path> {
        self.current.as_ref().map(|(p, _)| p.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_lines(path: &Path, lines: &[&str]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    #[test]
    fn latest_transcript_file_picks_most_recently_modified() {
        let dir = tempdir();
        let proj_a = dir.join("proj-a");
        let proj_b = dir.join("proj-b");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();

        let older = proj_a.join("older.jsonl");
        std::fs::write(&older, "{}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newer = proj_b.join("newer.jsonl");
        std::fs::write(&newer, "{}\n").unwrap();

        assert_eq!(latest_transcript_file(&dir), Some(newer));
    }

    #[test]
    fn latest_transcript_file_ignores_non_jsonl() {
        let dir = tempdir();
        let proj = dir.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("notes.txt"), "hi").unwrap();
        assert_eq!(latest_transcript_file(&dir), None);
    }

    #[test]
    fn tailer_from_start_replays_existing_lines() {
        let dir = tempdir();
        let path = dir.join("session.jsonl");
        write_lines(&path, &[r#"{"type":"mode"}"#, r#"{"type":"ai-title"}"#]);

        let mut tailer = Tailer::open_from_start(&path).unwrap();
        let lines = tailer.poll().unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn tailer_at_end_ignores_existing_lines_but_sees_new_ones() {
        let dir = tempdir();
        let path = dir.join("session.jsonl");
        write_lines(&path, &[r#"{"type":"mode"}"#]);

        let mut tailer = Tailer::open_at_end(&path).unwrap();
        assert_eq!(tailer.poll().unwrap().len(), 0, "pre-existing line should not replay");

        write_lines(&path, &[r#"{"type":"ai-title"}"#]);
        let lines = tailer.poll().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["type"], "ai-title");
    }

    #[test]
    fn tailer_holds_back_partial_trailing_line() {
        let dir = tempdir();
        let path = dir.join("session.jsonl");
        std::fs::write(&path, r#"{"type":"mode"}"#.to_string()).unwrap(); // no trailing newline

        let mut tailer = Tailer::open_from_start(&path).unwrap();
        assert_eq!(tailer.poll().unwrap().len(), 0, "unterminated line shouldn't parse yet");

        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f).unwrap(); // now terminate it
        let lines = tailer.poll().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["type"], "mode");
    }

    #[test]
    fn tailer_tolerates_corrupt_line() {
        let dir = tempdir();
        let path = dir.join("session.jsonl");
        write_lines(&path, &["not json at all", r#"{"type":"mode"}"#]);

        let mut tailer = Tailer::open_from_start(&path).unwrap();
        let lines = tailer.poll().unwrap();
        assert_eq!(lines.len(), 1, "corrupt line skipped, valid one kept");
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "companion-state-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// End-to-end: exercises the exact production pipeline
    /// (`ProjectsWatcher::poll` -> `StateMachine::apply`) the way
    /// `spawn_transcript_watcher` in `src-tauri/src/main.rs` really runs
    /// it, driven by a *live, concurrently-appending* file on another
    /// thread rather than a static fixture read once. This is the
    /// behavior a synthetic unit test on `StateMachine` alone can't catch:
    /// a real session file appearing after startup, a real session
    /// switch, and lines arriving one at a time under real wall-clock
    /// polling.
    #[test]
    fn end_to_end_watcher_tracks_a_live_growing_session_and_switches_sessions() {
        use crate::{CompanionState, StateMachine};
        use std::io::Write;
        use std::time::Duration;

        let root = tempdir();
        let project = root.join("-home-user-some-project");
        std::fs::create_dir_all(&project).unwrap();
        let session_a = project.join("session-a.jsonl");
        std::fs::write(&session_a, "").unwrap();

        let mut watcher = ProjectsWatcher::new(root.clone(), Duration::from_millis(20));
        let mut machine = StateMachine::new();
        let mut observed = Vec::new();

        // Writer thread simulates a real Claude Code process appending to
        // the transcript live, one line at a time with real delays --
        // exactly the scenario the polling design exists for.
        let writer_path = session_a.clone();
        let writer = std::thread::spawn(move || {
            let lines = [
                r#"{"type":"user","message":{"content":[{"type":"text","text":"fix it"}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","is_error":false,"content":"ok"}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"fixed"}]}}"#,
            ];
            for line in lines {
                std::thread::sleep(Duration::from_millis(30));
                let mut f = std::fs::OpenOptions::new().append(true).open(&writer_path).unwrap();
                writeln!(f, "{line}").unwrap();
            }
        });

        let deadline = Instant::now() + Duration::from_secs(3);
        while observed.len() < 4 && Instant::now() < deadline {
            for line in watcher.poll() {
                if let Some(state) = machine.apply(&line) {
                    observed.push(state);
                }
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        writer.join().unwrap();

        assert_eq!(
            observed,
            vec![
                CompanionState::Thinking,
                CompanionState::Typing,
                // tool_result ok while already Typing is a no-op, correctly absent
                CompanionState::WaitingForInput,
            ],
            "watcher should observe the real live-appended lines in order, \
             deduped exactly like StateMachine's unit tests say it should"
        );
        assert_eq!(watcher.current_path(), Some(session_a.as_path()));

        // Now a second, newer session file appears (user opened a new
        // Claude Code window/project) -- the watcher should switch to it
        // on its next rescan instead of staying stuck on session_a.
        std::thread::sleep(Duration::from_millis(25));
        let session_b = project.join("session-b.jsonl");
        std::fs::write(&session_b, "").unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        while watcher.current_path() != Some(session_b.as_path()) && Instant::now() < deadline {
            watcher.poll();
            std::thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(watcher.current_path(), Some(session_b.as_path()), "should switch to the newer session");
    }
}
